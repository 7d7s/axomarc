//! Generic HTTP JSON exporter: POST batches as a JSON array to a configurable
//! endpoint (webhooks, SIEM collectors, custom backends).

use std::sync::OnceLock;
use std::time::Duration;

use kelson_telemetry::{Event, ExportError, ExportOk, Exporter};

pub struct HttpExporterConfig {
    pub url: String,
    pub timeout: Duration,
    pub bearer_token: Option<String>,
}

pub struct HttpExporter {
    cfg: HttpExporterConfig,
    client: OnceLock<reqwest::blocking::Client>,
}

impl HttpExporter {
    pub fn new(cfg: HttpExporterConfig) -> Self {
        Self {
            cfg,
            client: OnceLock::new(),
        }
    }

    fn client(&self) -> &reqwest::blocking::Client {
        self.client.get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(self.cfg.timeout)
                .build()
                .expect("reqwest client builds (rustls, no TLS init failure)")
        })
    }
}

impl Exporter for HttpExporter {
    fn name(&self) -> &str {
        "http"
    }

    fn export(&self, batch: &[Event]) -> Result<ExportOk, ExportError> {
        let body =
            serde_json::to_vec(batch).map_err(|e| ExportError::Rejected(format!("encode: {e}")))?;
        let len = body.len();
        let mut req = self.client().post(&self.cfg.url).body(body);
        if let Some(token) = &self.cfg.bearer_token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .map_err(|e| ExportError::Transport(format!("send: {e}")))?;
        let status = resp.status();
        if status.is_server_error() {
            return Err(ExportError::Transport(format!("status {status}")));
        }
        if !status.is_success() {
            return Err(ExportError::Rejected(format!("status {status}")));
        }
        Ok(ExportOk {
            bytes: len,
            events: batch.len(),
        })
    }
}
