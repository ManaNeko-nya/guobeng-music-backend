//! 账户接口：注册、登录、刷新、登出。

use axum::{
    extract::State,
    routing::post,
    Router,
};
use uuid::Uuid;

use crate::dto::{LoginResponse, TokenPair, UserPublic};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiJson, ClientInfo, CurrentUser, MaybeJson};
use crate::models::UserRow;
use crate::payload::{LoginRequest, LogoutRequest, RefreshRequest, RegisterRequest};
use crate::security::{
    create_access_token, dummy_password_hash, generate_refresh_token, hash_password,
    hash_refresh_token, needs_rehash, validate_password_strength, verify_password,
};
use crate::state::AppState;
use crate::timeutil::{now_plus_seconds, now_text};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/logout-all", post(logout_all))
}

async fn issue_token_pair(
    state: &AppState,
    user: &UserRow,
    user_agent: Option<String>,
    ip: &str,
) -> ApiResult<TokenPair> {
    let (access_token, expires_in) =
        create_access_token(&user.uuid, &state.settings.secret_key, state.settings.access_token_ttl);
    let refresh_token = generate_refresh_token();
    sqlx::query(
        "INSERT INTO refresh_tokens (user_id, token_hash, expires_at, user_agent, ip, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user.id)
    .bind(hash_refresh_token(&refresh_token))
    .bind(now_plus_seconds(state.settings.refresh_token_ttl))
    .bind(user_agent)
    .bind(ip)
    .bind(now_text())
    .execute(&state.db)
    .await?;

    Ok(TokenPair {
        access_token,
        refresh_token,
        token_type: "bearer".to_string(),
        expires_in,
        refresh_expires_in: state.settings.refresh_token_ttl,
    })
}

async fn register(
    State(state): State<AppState>,
    client: ClientInfo,
    ApiJson(payload): ApiJson<RegisterRequest>,
) -> ApiResult<crate::error::ApiOk<LoginResponse>> {
    if !state.runtime.get().allow_registration {
        return Err(AppError::forbidden("当前站点已关闭注册"));
    }
    payload.validate()?;
    let username = payload.username.trim().to_string();

    if let Err(message) =
        validate_password_strength(&payload.password, state.settings.password_min_length)
    {
        return Err(AppError::bad_request(message));
    }

    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username = ?")
        .bind(&username)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_some() {
        return Err(AppError::conflict("用户名已被占用"));
    }

    let password = payload.password.clone();
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AppError::internal("密码处理失败"))?;

    let nickname: String = payload
        .nickname
        .as_deref()
        .unwrap_or(&username)
        .trim()
        .chars()
        .take(32)
        .collect();
    let nickname = if nickname.is_empty() { username.clone() } else { nickname };
    let now = now_text();

    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (uuid, username, password_hash, nickname, role, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'user', ?, ?) RETURNING id",
    )
    .bind(Uuid::new_v4().simple().to_string())
    .bind(&username)
    .bind(&password_hash)
    .bind(&nickname)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    let user = fetch_user(&state, user_id).await?;
    let tokens = issue_token_pair(&state, &user, client.user_agent, &client.ip).await?;
    tracing::info!("新用户注册 user={} ({})", user.username, user.uuid);
    Ok(ok(LoginResponse { user: UserPublic::from(&user), tokens }))
}

async fn fetch_user(state: &AppState, id: i64) -> ApiResult<UserRow> {
    sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("用户不存在"))
}

async fn login(
    State(state): State<AppState>,
    client: ClientInfo,
    ApiJson(payload): ApiJson<LoginRequest>,
) -> ApiResult<crate::error::ApiOk<LoginResponse>> {
    let account = payload.account.trim().to_string();
    let key = format!("{}|{}", client.ip, account.to_lowercase());

    let locked = state.limiter.locked_seconds(&key);
    if locked > 0 {
        return Err(AppError::rate_limited(format!(
            "登录失败次数过多，请 {locked} 秒后重试"
        )));
    }

    let user = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE username = ?")
        .bind(&account)
        .fetch_optional(&state.db)
        .await?;

    let Some(user) = user else {
        let password = payload.password.clone();
        let _ = tokio::task::spawn_blocking(move || verify_password(&password, dummy_password_hash()))
            .await;
        state.limiter.register_failure(&key);
        tracing::info!("登录失败 account={} ip={}", account, client.ip);
        return Err(AppError::unauthorized("用户名或密码错误"));
    };

    let password = payload.password.clone();
    let stored = user.password_hash.clone();
    let matched = tokio::task::spawn_blocking(move || verify_password(&password, &stored))
        .await
        .unwrap_or(false);
    if !matched {
        state.limiter.register_failure(&key);
        tracing::info!("登录失败 account={} ip={}", account, client.ip);
        return Err(AppError::unauthorized("用户名或密码错误"));
    }
    if !user.active() {
        return Err(AppError::unauthorized("账户已被禁用"));
    }
    if user.is_root() && !client.local {
        tracing::warn!("拒绝 root 远程登录 ip={}", client.ip);
        return Err(AppError::unauthorized("root 账户仅允许从本机登录"));
    }

    state.limiter.register_success(&key);

    let mut password_hash = None;
    if needs_rehash(&user.password_hash) {
        let password = payload.password.clone();
        if let Ok(hash) = tokio::task::spawn_blocking(move || hash_password(&password)).await {
            password_hash = Some(hash);
        }
    }
    let now = now_text();
    sqlx::query(
        "UPDATE users SET last_login_at = ?, last_login_ip = ?,
                          password_hash = COALESCE(?, password_hash), updated_at = ?
         WHERE id = ?",
    )
    .bind(&now)
    .bind(&client.ip)
    .bind(&password_hash)
    .bind(&now)
    .bind(user.id)
    .execute(&state.db)
    .await?;

    let user = fetch_user(&state, user.id).await?;
    let tokens = issue_token_pair(&state, &user, client.user_agent, &client.ip).await?;
    tracing::info!("用户登录成功 user={} ({})", user.username, user.uuid);
    Ok(ok(LoginResponse { user: UserPublic::from(&user), tokens }))
}

async fn refresh(
    State(state): State<AppState>,
    client: ClientInfo,
    ApiJson(payload): ApiJson<RefreshRequest>,
) -> ApiResult<crate::error::ApiOk<TokenPair>> {
    let token_hash = hash_refresh_token(&payload.refresh_token);
    let record = sqlx::query_as::<_, crate::models::RefreshTokenRow>(
        "SELECT id, user_id, expires_at, revoked_at FROM refresh_tokens WHERE token_hash = ?",
    )
    .bind(&token_hash)
    .fetch_optional(&state.db)
    .await?;

    let Some(record) = record.filter(|row| row.valid()) else {
        return Err(AppError::unauthorized("刷新令牌无效或已过期，请重新登录"));
    };

    let user = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = ?")
        .bind(record.user_id)
        .fetch_optional(&state.db)
        .await?
        .filter(|user| user.active())
        .ok_or_else(|| AppError::unauthorized("账户不可用"))?;

    if user.is_root() && !client.local {
        tracing::warn!("拒绝 root 账户的远程令牌刷新 ip={}", client.ip);
        return Err(AppError::unauthorized("root 账户仅允许从本机登录"));
    }

    let tokens = issue_token_pair(&state, &user, client.user_agent, &client.ip).await?;
    sqlx::query("UPDATE refresh_tokens SET revoked_at = ?, replaced_by = ? WHERE id = ?")
        .bind(now_text())
        .bind(hash_refresh_token(&tokens.refresh_token))
        .bind(record.id)
        .execute(&state.db)
        .await?;
    Ok(ok(tokens))
}

async fn revoke_tokens(state: &AppState, user_id: i64, token: Option<&str>) -> ApiResult<i64> {
    let now = now_text();
    let result = match token {
        Some(token) => {
            sqlx::query(
                "UPDATE refresh_tokens SET revoked_at = ?
                 WHERE user_id = ? AND revoked_at IS NULL AND token_hash = ?",
            )
            .bind(&now)
            .bind(user_id)
            .bind(hash_refresh_token(token))
            .execute(&state.db)
            .await?
        }
        None => {
            sqlx::query(
                "UPDATE refresh_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
            )
            .bind(&now)
            .bind(user_id)
            .execute(&state.db)
            .await?
        }
    };
    Ok(result.rows_affected() as i64)
}

async fn logout(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    MaybeJson(payload): MaybeJson<LogoutRequest>,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let token = payload.and_then(|body| body.refresh_token);
    let revoked = revoke_tokens(&state, user.id, token.as_deref()).await?;
    Ok(ok(serde_json::json!({ "revoked": revoked })))
}

async fn logout_all(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let revoked = revoke_tokens(&state, user.id, None).await?;
    Ok(ok(serde_json::json!({ "revoked": revoked })))
}
