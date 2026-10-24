//! Disk-backed write-ahead log for durable telemetry (Sentinel S5 §18).
//!
//! Layout: `wal/segment-<first-LSN>.wal` files of length-prefixed JSON
//! records; each record carries its LSN and a truncated sha2-256 checksum.
//! A `watermark.json` records the highest LSN durably exported. Replay skips
//! corrupted/partial tail records and reports them (never panics).
//!
//! Recovery semantics: on restart, records with `lsn > watermark` are
//! re-exported. Data after the last successfully written+flushed record in a
//! torn segment is lost by design (documented SIGKILL gap, S5 spec).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const MAGIC: &[u8; 2] = b"K5";
const DEFAULT_MAX_SEGMENT_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct WalConfig {
    pub dir: PathBuf,
    pub max_segment_bytes: u64,
    /// fsync after every durable append (stronger durability, slower).
    pub fsync_durable: bool,
}

impl WalConfig {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_segment_bytes: DEFAULT_MAX_SEGMENT_BYTES,
            fsync_durable: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WalStats {
    pub segments: usize,
    pub bytes_on_disk: u64,
    pub watermark: u64,
    pub corrupted_records_skipped: u64,
}

/// One replayed record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRecord {
    pub lsn: u64,
    pub bytes: Vec<u8>,
}

fn checksum(lsn: u64, body: &[u8]) -> u32 {
    let mut h = Sha256::new();
    h.update(lsn.to_le_bytes());
    h.update(body);
    let digest = h.finalize();
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

fn segment_name(first_lsn: u64) -> String {
    format!("segment-{first_lsn:020}.wal")
}

fn first_lsn_of(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let digits = name
        .strip_prefix("segment-")
        .and_then(|rest| rest.strip_suffix(".wal"))?;
    digits.parse::<u64>().ok()
}

pub struct Wal {
    cfg: WalConfig,
    current: Option<File>,
    current_first_lsn: u64,
    current_size: u64,
    watermark: u64,
}

impl Wal {
    /// Open (or create) the WAL directory and load the watermark.
    pub fn open(cfg: WalConfig) -> io::Result<Self> {
        fs::create_dir_all(&cfg.dir)?;
        let watermark = read_watermark(&cfg.dir).unwrap_or(0);
        Ok(Self {
            cfg,
            current: None,
            current_first_lsn: 0,
            current_size: 0,
            watermark,
        })
    }

    pub fn watermark(&self) -> u64 {
        self.watermark
    }

    /// Append one record. `durable` triggers fsync when configured.
    pub fn append(&mut self, lsn: u64, body: &[u8], durable: bool) -> io::Result<()> {
        if lsn <= self.watermark {
            return Ok(()); // already exported; replay artifact, ignore
        }
        if self.current.is_none() || self.current_size >= self.cfg.max_segment_bytes {
            self.rotate(lsn)?;
        }
        let file = self.current.as_mut().expect("segment open after rotate");
        let record: Vec<u8> = {
            let len = (8 + body.len() + 4) as u32;
            let mut buf = Vec::with_capacity(2 + 4 + len as usize);
            buf.extend_from_slice(MAGIC);
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&lsn.to_le_bytes());
            buf.extend_from_slice(body);
            buf.extend_from_slice(&checksum(lsn, body).to_le_bytes());
            buf
        };
        file.write_all(&record)?;
        self.current_size += record.len() as u64;
        if durable && self.cfg.fsync_durable {
            file.sync_all()?;
        }
        Ok(())
    }

    fn rotate(&mut self, next_lsn: u64) -> io::Result<()> {
        if let Some(mut f) = self.current.take() {
            let _ = f.flush();
        }
        let path = self.cfg.dir.join(segment_name(next_lsn));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        self.current = Some(file);
        self.current_first_lsn = next_lsn;
        self.current_size = 0;
        Ok(())
    }

    /// Ordered list of segment paths by first LSN.
    pub fn segments(&self) -> io::Result<Vec<PathBuf>> {
        let mut out: Vec<(u64, PathBuf)> = Vec::new();
        for entry in fs::read_dir(&self.cfg.dir)? {
            let path = entry?.path();
            if let Some(lsn) = first_lsn_of(&path) {
                out.push((lsn, path));
            }
        }
        out.sort_by_key(|(lsn, _)| *lsn);
        Ok(out.into_iter().map(|(_, p)| p).collect())
    }

    /// Replay all records with `lsn > since` in LSN order. Stops cleanly at
    /// the first corrupted record in each segment tail (torn write), counting
    /// skips.
    pub fn replay(&self, since: u64) -> io::Result<(Vec<WalRecord>, u64)> {
        let mut records = Vec::new();
        let mut skipped = 0u64;
        for path in self.segments()? {
            let mut file = File::open(&path)?;
            let mut buf = Vec::new();
            file.seek(SeekFrom::Start(0))?;
            file.read_to_end(&mut buf)?;
            let mut pos = 0usize;
            while pos + 6 <= buf.len() {
                if &buf[pos..pos + 2] != MAGIC {
                    skipped += 1;
                    break;
                }
                let len =
                    u32::from_le_bytes([buf[pos + 2], buf[pos + 3], buf[pos + 4], buf[pos + 5]])
                        as usize;
                let record_end = pos + 6 + len;
                if len < 12 || record_end > buf.len() {
                    skipped += 1; // partial tail record
                    break;
                }
                let lsn = u64::from_le_bytes(buf[pos + 6..pos + 14].try_into().unwrap());
                let body = &buf[pos + 14..record_end - 4];
                let expect =
                    u32::from_le_bytes(buf[record_end - 4..record_end].try_into().unwrap());
                if checksum(lsn, body) != expect {
                    skipped += 1;
                    break; // corrupted: trust nothing after it in this segment
                }
                if lsn > since {
                    records.push(WalRecord {
                        lsn,
                        bytes: body.to_vec(),
                    });
                }
                pos = record_end;
            }
            if pos < buf.len() {
                skipped += 1; // short torn tail (partial header/record)
            }
        }
        records.sort_by_key(|r| r.lsn);
        Ok((records, skipped))
    }

    /// Advance the exported watermark; segments fully below it are removed.
    pub fn ack(&mut self, lsn: u64) -> io::Result<()> {
        if lsn <= self.watermark {
            return Ok(());
        }
        self.watermark = lsn;
        write_watermark(&self.cfg.dir, lsn)?;
        // A segment is fully exported only when the next segment already
        // starts at or below watermark+1 (all its records are <= watermark).
        let mut segs: Vec<(u64, PathBuf)> = Vec::new();
        for path in self.segments()? {
            if let Some(first) = first_lsn_of(&path) {
                segs.push((first, path));
            }
        }
        let current = self.current_segment_path();
        for i in 0..segs.len().saturating_sub(1) {
            let next_first = segs[i + 1].0;
            if next_first <= self.watermark.saturating_add(1)
                && Some(&segs[i].1) != current.as_ref()
            {
                fs::remove_file(&segs[i].1).ok();
            }
        }
        Ok(())
    }

    fn current_segment_path(&self) -> Option<PathBuf> {
        self.current
            .as_ref()
            .map(|_| self.cfg.dir.join(segment_name(self.current_first_lsn)))
    }

    pub fn stats(&self) -> io::Result<WalStats> {
        let segs = self.segments()?;
        let mut bytes = 0u64;
        for p in &segs {
            bytes += fs::metadata(p)?.len();
        }
        Ok(WalStats {
            segments: segs.len(),
            bytes_on_disk: bytes,
            watermark: self.watermark,
            corrupted_records_skipped: 0,
        })
    }

    /// Flush pending bytes (graceful shutdown).
    pub fn flush(&mut self) -> io::Result<()> {
        if let Some(f) = self.current.as_mut() {
            f.flush()?;
            if self.cfg.fsync_durable {
                f.sync_all()?;
            }
        }
        Ok(())
    }
}

fn watermark_path(dir: &Path) -> PathBuf {
    dir.join("watermark.json")
}

fn read_watermark(dir: &Path) -> Option<u64> {
    let raw = fs::read_to_string(watermark_path(dir)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("watermark")?.as_u64()
}

fn write_watermark(dir: &Path, watermark: u64) -> io::Result<()> {
    let tmp = dir.join("watermark.json.tmp");
    let json = serde_json::json!({ "watermark": watermark });
    fs::write(&tmp, serde_json::to_vec_pretty(&json).unwrap_or_default())?;
    fs::rename(&tmp, watermark_path(dir))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kelson-buffer-test-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn append_and_replay_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let mut wal = Wal::open(WalConfig::new(&dir)).unwrap();
        wal.append(1, b"one", true).unwrap();
        wal.append(2, b"two", false).unwrap();
        wal.flush().unwrap();
        let (records, skipped) = Wal::open(WalConfig::new(&dir)).unwrap().replay(0).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].lsn, 1);
        assert_eq!(records[0].bytes, b"one");
        assert_eq!(records[1].bytes, b"two");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replay_honours_watermark_filter() {
        let dir = tmp_dir("watermark-filter");
        let mut wal = Wal::open(WalConfig::new(&dir)).unwrap();
        for lsn in 1..=5 {
            wal.append(lsn, format!("e{lsn}").as_bytes(), false)
                .unwrap();
        }
        let (records, _) = wal.replay(3).unwrap();
        assert_eq!(
            records.iter().map(|r| r.lsn).collect::<Vec<_>>(),
            vec![4, 5]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_tail_is_skipped_not_fatal() {
        let dir = tmp_dir("corrupt-tail");
        {
            let mut wal = Wal::open(WalConfig::new(&dir)).unwrap();
            wal.append(1, b"good", true).unwrap();
            wal.flush().unwrap();
        }
        // Append garbage to the segment tail.
        let seg = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "wal"))
            .unwrap();
        let mut f = OpenOptions::new().append(true).open(&seg).unwrap();
        f.write_all(b"K5\xff\xff").unwrap();
        let wal = Wal::open(WalConfig::new(&dir)).unwrap();
        let (records, skipped) = wal.replay(0).unwrap();
        assert_eq!(records.len(), 1);
        assert!(skipped >= 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ack_advances_watermark_and_truncates() {
        let dir = tmp_dir("ack");
        let mut cfg = WalConfig::new(&dir);
        cfg.max_segment_bytes = 64; // force rotation
        {
            let mut wal = Wal::open(cfg.clone()).unwrap();
            for lsn in 1..=6 {
                wal.append(lsn, b"0123456789", true).unwrap();
            }
            wal.flush().unwrap();
            assert!(wal.segments().unwrap().len() >= 2);
        }
        let mut wal = Wal::open(cfg.clone()).unwrap();
        wal.ack(4).unwrap();
        assert_eq!(wal.watermark(), 4);
        let (records, _) = wal.replay(wal.watermark()).unwrap();
        let lsns: Vec<u64> = records.iter().map(|r| r.lsn).collect();
        assert!(lsns.iter().all(|l| *l > 4), "got {lsns:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
