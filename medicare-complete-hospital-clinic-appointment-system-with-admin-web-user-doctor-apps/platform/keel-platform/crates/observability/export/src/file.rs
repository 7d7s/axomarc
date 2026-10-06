//! File exporter (JSONL) with size-based rotation and an injectable
//! filesystem seam for failure tests (ENOSPC etc).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use keel_telemetry::{Event, ExportError, ExportOk, Exporter};

use crate::format::{event_to_jsonl, redact_line};

/// Minimal filesystem seam (tests inject failures).
pub trait FileSink: Send + Sync {
    fn open_append(&self, path: &Path) -> std::io::Result<Box<dyn Write + Send>>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn metadata_len(&self, path: &Path) -> std::io::Result<u64>;
}

pub struct RealFileSink;

impl FileSink for RealFileSink {
    fn open_append(&self, path: &Path) -> std::io::Result<Box<dyn Write + Send>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Box::new(file))
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        fs::rename(from, to)
    }

    fn metadata_len(&self, path: &Path) -> std::io::Result<u64> {
        Ok(fs::metadata(path)?.len())
    }
}

pub struct FileExporterConfig {
    pub path: PathBuf,
    pub max_bytes: u64,
    pub keep_rotations: u32,
}

impl FileExporterConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max_bytes: 50 * 1024 * 1024,
            keep_rotations: 5,
        }
    }
}

pub struct FileExporter {
    cfg: FileExporterConfig,
    sink: Box<dyn FileSink>,
    handle: Mutex<Option<Box<dyn Write + Send>>>,
}

impl FileExporter {
    pub fn new(cfg: FileExporterConfig) -> Self {
        Self::with_sink(cfg, Box::new(RealFileSink))
    }

    pub fn with_sink(cfg: FileExporterConfig, sink: Box<dyn FileSink>) -> Self {
        Self {
            cfg,
            sink,
            handle: Mutex::new(None),
        }
    }

    fn rotate_if_needed(&self) -> std::io::Result<()> {
        let Ok(len) = self.sink.metadata_len(&self.cfg.path) else {
            return Ok(());
        };
        if len < self.cfg.max_bytes {
            return Ok(());
        }
        for i in (1..self.cfg.keep_rotations).rev() {
            let from = rotation_path(&self.cfg.path, i);
            let to = rotation_path(&self.cfg.path, i + 1);
            if self.sink.metadata_len(&from).is_ok() {
                self.sink.rename(&from, &to)?;
            }
        }
        self.sink
            .rename(&self.cfg.path, &rotation_path(&self.cfg.path, 1))?;
        Ok(())
    }
}

fn rotation_path(base: &Path, n: u32) -> PathBuf {
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = base
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    base.with_file_name(format!("{stem}.{n}{ext}"))
}

impl Exporter for FileExporter {
    fn name(&self) -> &str {
        "file"
    }

    fn export(&self, batch: &[Event]) -> Result<ExportOk, ExportError> {
        let mut handle = self.handle.lock().expect("file exporter poisoned");
        if handle.is_none() {
            self.rotate_if_needed()
                .map_err(|e| ExportError::Transport(format!("rotate: {e}")))?;
            *handle = Some(
                self.sink
                    .open_append(&self.cfg.path)
                    .map_err(|e| ExportError::Transport(format!("open: {e}")))?,
            );
        }

        let result = {
            let writer = handle.as_mut().expect("opened above");
            (|| -> std::io::Result<usize> {
                let mut written = 0usize;
                for event in batch {
                    let line = redact_line(&event_to_jsonl(event));
                    writer.write_all(line.as_bytes())?;
                    writer.write_all(b"\n")?;
                    written += line.len() + 1;
                }
                writer.flush()?;
                Ok(written)
            })()
        };
        let bytes = match result {
            Ok(w) => w,
            Err(e) => {
                *handle = None; // force reopen next batch (ENOSPC recovery)
                return Err(ExportError::Transport(format!("write: {e}")));
            }
        };
        Ok(ExportOk {
            bytes,
            events: batch.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_path_shape() {
        let p = PathBuf::from("/var/log/keel/app.log");
        assert_eq!(
            rotation_path(&p, 1),
            PathBuf::from("/var/log/keel/app.1.log")
        );
    }
}
