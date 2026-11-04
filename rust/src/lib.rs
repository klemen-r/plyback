pub mod analysis;
pub mod config;
pub mod db;
pub mod engine;
pub mod http;
pub mod insights;
pub mod pgn;
pub mod security;
pub mod services;

use serde_json::Value;
use std::collections::HashMap;

pub type Query = HashMap<String, String>;
pub type ApiResult = anyhow::Result<(u16, Value)>;
pub const ANALYSIS_VERSION: i64 = 7;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug)]
pub struct ApiError(pub u16, pub String);
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.1)
    }
}
impl std::error::Error for ApiError {}
pub fn api_error(status: u16, message: impl Into<String>) -> anyhow::Error {
    ApiError(status, message.into()).into()
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
