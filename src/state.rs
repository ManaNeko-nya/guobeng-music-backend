//! 应用状态。

use std::sync::Arc;

use sqlx::sqlite::SqlitePool;

use crate::runtime_config::RuntimeConfig;
use crate::security::LoginRateLimiter;
use crate::settings::Settings;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub settings: Arc<Settings>,
    pub runtime: Arc<RuntimeConfig>,
    pub limiter: Arc<LoginRateLimiter>,
}
