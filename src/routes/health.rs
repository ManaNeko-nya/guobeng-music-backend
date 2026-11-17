//! 健康检查：公开探活，版本与曲库规模仅管理员可见。

use axum::{extract::State, routing::get, Router};
use serde_json::{json, Value};

use crate::error::{ok, ApiResult};
use crate::extract::OptionalUser;
use crate::settings::API_VERSION;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

async fn health(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
) -> ApiResult<crate::error::ApiOk<Value>> {
    let privileged = state.settings.debug
        || user.map(|value| value.is_admin()).unwrap_or(false);
    if !privileged {
        return Ok(ok(json!({ "status": "running" })));
    }

    let mut stats = serde_json::Map::new();
    let mut healthy = true;
    for (key, table) in [
        ("artists", "artists"),
        ("albums", "albums"),
        ("tracks", "tracks"),
        ("users", "users"),
    ] {
        match crate::repo::count(&state.db, table).await {
            Ok(value) => {
                stats.insert(key.to_string(), Value::from(value));
            }
            Err(err) => {
                tracing::error!("统计信息获取失败：{err}");
                healthy = false;
                break;
            }
        }
    }

    Ok(ok(json!({
        "status": "running",
        "version": API_VERSION,
        "debug": state.settings.debug,
        "stats": if healthy { Value::Object(stats) } else { json!({}) },
    })))
}
