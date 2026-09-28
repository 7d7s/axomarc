//! Typed boot-time config (02-SYSTEM-DESIGN §9): fail fast, secrets via ENV only.
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerCfg,
    pub database: DatabaseCfg,
    pub deploy: DeployCfg,
    pub jwt: JwtCfg,
    pub otp: OtpCfg,
    pub storage: StorageCfg,
    pub demo: DemoCfg,
}

#[derive(Debug, Deserialize)]
pub struct ServerCfg { pub bind: String }

#[derive(Debug, Deserialize)]
pub struct DatabaseCfg { pub url: String, #[serde(default = "default_max_conn")] pub max_connections: u32 }

fn default_max_conn() -> u32 { 10 }

#[derive(Debug, Deserialize)]
pub struct DeployCfg { pub mode: String } // standalone | saas (D-11)

#[derive(Debug, Deserialize)]
pub struct JwtCfg {
    pub mode: String,
    pub dev_secret: String,
    #[serde(default = "default_access_ttl")] pub access_ttl_secs: i64,
    #[serde(default = "default_refresh_ttl")] pub refresh_ttl_secs: i64,
}

fn default_access_ttl() -> i64 { 900 }
fn default_refresh_ttl() -> i64 { 2_592_000 }

#[derive(Debug, Deserialize)]
pub struct OtpCfg { pub provider: String, #[serde(default = "default_otp_ttl")] pub code_ttl_secs: i64, #[serde(default = "default_otp_attempts")] pub max_attempts: i32 }

fn default_otp_ttl() -> i64 { 300 }
fn default_otp_attempts() -> i32 { 5 }

#[derive(Debug, Deserialize)]
pub struct StorageCfg { pub endpoint: String, pub bucket: String }

#[derive(Debug, Deserialize)]
pub struct DemoCfg { pub tenant_id: String }

pub fn load() -> Result<Config, Box<dyn std::error::Error>> {
    let path = std::env::var("MEDICARE_CONFIG").unwrap_or_else(|_| "config/dev.toml".into());
    let raw = std::fs::read_to_string(&path)?;
    let cfg: Config = toml::from_str(&raw)?;
    if cfg.server.bind != "127.0.0.1:8080" && cfg.jwt.mode == "hs256-dev" {
        tracing::error!("refusing non-loopback bind with jwt.mode=hs256-dev (NFR-S1)");
        return Err("insecure config".into());
    }
    Ok(cfg)
}