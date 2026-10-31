//! 用户资料接口：本人资料、修改资料、修改密码、查看公开资料。

use axum::{
    extract::State,
    routing::{get, post},
    Router,
};

use crate::dto::{UserPrivate, UserPublic};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiJson, ApiPath, CurrentUser};
use crate::media::delete_media_file;
use crate::payload::{validate_media_url, PasswordChangeRequest, ProfileUpdateRequest};
use crate::security::{hash_password, validate_password_strength, verify_password};
use crate::state::AppState;
use crate::timeutil::{now_text, opt_iso};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/me", get(read_me).patch(update_me))
        .route("/users/me/password", post(change_password))
        .route("/users/{user_id}", get(read_user))
}

async fn read_me(CurrentUser(user): CurrentUser) -> ApiResult<crate::error::ApiOk<UserPrivate>> {
    Ok(ok(UserPrivate {
        base: UserPublic::from(&user),
        last_login_at: opt_iso(&user.last_login_at),
    }))
}

async fn update_me(
    State(state): State<AppState>,
    CurrentUser(mut user): CurrentUser,
    ApiJson(payload): ApiJson<ProfileUpdateRequest>,
) -> ApiResult<crate::error::ApiOk<UserPublic>> {
    let old_avatar = user.avatar_url.clone();
    let mut avatar_changed = false;

    if let Some(nickname) = payload.nickname.as_deref() {
        let nickname = nickname.trim();
        if nickname.is_empty() {
            return Err(AppError::bad_request("昵称不能为空"));
        }
        if nickname.chars().count() > 32 {
            return Err(AppError::validation("body.nickname", "长度不能超过 32 个字符"));
        }
        user.nickname = nickname.to_string();
    }
    if let Some(bio) = payload.bio {
        if let Some(raw) = bio.as_deref() {
            if raw.chars().count() > 500 {
                return Err(AppError::validation("body.bio", "长度不能超过 500 个字符"));
            }
        }
        user.bio = bio
            .as_deref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    if let Some(avatar) = payload.avatar_url {
        let next = match avatar.as_deref() {
            Some(value) => validate_media_url("body.avatar_url", value)?,
            None => None,
        };
        if next != user.avatar_url {
            avatar_changed = true;
        }
        user.avatar_url = next;
    }

    sqlx::query("UPDATE users SET nickname = ?, bio = ?, avatar_url = ?, updated_at = ? WHERE id = ?")
        .bind(&user.nickname)
        .bind(&user.bio)
        .bind(&user.avatar_url)
        .bind(now_text())
        .bind(user.id)
        .execute(&state.db)
        .await?;

    if avatar_changed {
        delete_media_file(&state.settings, &state.db, old_avatar.as_ref(), &user, false).await;
    }
    Ok(ok(UserPublic::from(&user)))
}

async fn change_password(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<PasswordChangeRequest>,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    payload.validate()?;

    let stored = user.password_hash.clone();
    let old = payload.old_password.clone();
    let matched = tokio::task::spawn_blocking(move || verify_password(&old, &stored))
        .await
        .unwrap_or(false);
    if !matched {
        return Err(AppError::unauthorized("原密码不正确"));
    }
    if let Err(message) =
        validate_password_strength(&payload.new_password, state.settings.password_min_length)
    {
        return Err(AppError::bad_request(message));
    }
    if payload.old_password == payload.new_password {
        return Err(AppError::bad_request("新密码不能与旧密码相同"));
    }

    let password = payload.new_password.clone();
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AppError::internal("密码处理失败"))?;
    let now = now_text();

    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
        .bind(&password_hash)
        .bind(&now)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE refresh_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
        .bind(&now)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    tracing::info!("用户修改密码成功 user={}，刷新令牌已全部吊销", user.uuid);
    Ok(ok(serde_json::json!({ "changed": true })))
}

async fn read_user(
    State(state): State<AppState>,
    ApiPath(user_id): ApiPath<String>,
) -> ApiResult<crate::error::ApiOk<UserPublic>> {
    let user = sqlx::query_as::<_, crate::models::UserRow>("SELECT * FROM users WHERE uuid = ?")
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::not_found("用户不存在"))?;
    Ok(ok(UserPublic::from(&user)))
}
