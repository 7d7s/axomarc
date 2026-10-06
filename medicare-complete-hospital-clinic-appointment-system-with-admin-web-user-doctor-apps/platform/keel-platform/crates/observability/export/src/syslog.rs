//! Syslog exporter: RFC5424 over TCP (std only, no extra dependencies).

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use keel_telemetry::{Event, ExportError, ExportOk, Exporter, Level};

pub struct SyslogExporter {
    pub addr: String,
    pub app_name: String,
    pub facility: u8,
    timeout: Duration,
    stream: Mutex<Option<TcpStream>>,
}

fn severity_code(level: Level) -> u8 {
    match level {
        Level::Debug => 7,
        Level::Info => 6,
        Level::Notice => 5,
        Level::Warn => 4,
        Level::Error => 3,
        Level::Critical => 2,
    }
}

impl SyslogExporter {
    pub fn new(addr: impl Into<String>, app_name: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            app_name: app_name.into(),
            facility: 4, // auth-ish default; operators tune per deployment
            timeout: Duration::from_secs(3),
            stream: Mutex::new(None),
        }
    }

    fn stream(&self) -> Result<std::sync::MutexGuard<'_, Option<TcpStream>>, ExportError> {
        let mut guard = self.stream.lock().expect("syslog poisoned");
        if guard.is_none() {
            let addrs: Vec<_> = self
                .addr
                .to_socket_addrs()
                .map_err(|e| ExportError::Transport(format!("resolve: {e}")))?
                .collect();
            let stream = TcpStream::connect_timeout(
                addrs
                    .first()
                    .ok_or_else(|| ExportError::Transport("no addr".into()))?,
                self.timeout,
            )
            .map_err(|e| ExportError::Transport(format!("connect: {e}")))?;
            *guard = Some(stream);
        }
        Ok(guard)
    }
}

impl Exporter for SyslogExporter {
    fn name(&self) -> &str {
        "syslog"
    }

    fn export(&self, batch: &[Event]) -> Result<ExportOk, ExportError> {
        let mut guard = self.stream()?;
        let host = "keel";

        let result = {
            let stream = guard.as_mut().expect("connected above");
            (|| -> std::io::Result<usize> {
                let mut written = 0usize;
                for event in batch {
                    let sev = severity_code(event.payload.level_hint().unwrap_or(Level::Info));
                    let pri = self.facility * 8 + sev;
                    let ts = event
                        .at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default();
                    let msg = crate::format::sanitize_text(&crate::format::event_to_pretty(event));
                    let line = format!(
                        "<{pri}>1 {ts} {host} {} - {} {}\n",
                        self.app_name, event.id, msg
                    );
                    stream.write_all(line.as_bytes())?;
                    written += line.len();
                }
                Ok(written)
            })()
        };
        let bytes = match result {
            Ok(w) => w,
            Err(e) => {
                *guard = None;
                return Err(ExportError::Transport(format!("syslog write: {e}")));
            }
        };
        Ok(ExportOk {
            bytes,
            events: batch.len(),
        })
    }
}
