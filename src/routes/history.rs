//! 播放历史：按 (用户, 歌曲) 记录进度，可续播。

use axum::{
    extract::State,
    routing::{delete, get},
    Router,
};

use crate::dto::{history_out, HistoryOut, Page};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiJson, ApiPath, CurrentUser, Pagination};
use crate::hydrate;
use crate::models::HistoryRow;
use crate::payload::HistoryUpsert;
use crate::services::{can_manage_authored_content, visible_album, visible_track};
use crate::state::AppState;
use crate::timeutil::now_text;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/history", get(list_history).post(upsert_history).delete(clear_history))
        .route("/history/track/{track_id}", get(track_history))
        .route("/history/album/{album_id}/last", get(last_album_history))
        .route("/history/album/{album_id}", delete(delete_album_history))
        .route("/history/{history_id}", delete(delete_history))
}

fn visible_history_sql() -> String {
    crate::services::visible_album_of_track_sql("h.track_id")
}

async fn list_history(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    pagination: Pagination,
) -> ApiResult<crate::error::ApiOk<Page<HistoryOut>>> {
    let admin = if user.is_admin() { 1 } else { 0 };
    let filter = format!("WHERE h.user_id = ? AND {}", visible_history_sql());
    let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM listen_history h {filter}"))
        .bind(user.id)
        .bind(user.id)
        .bind(admin)
        .fetch_one(&state.db)
        .await?;

    let rows = sqlx::query_as::<_, HistoryRow>(&format!(
        "SELECT h.id, h.track_id, h.progress_sec, h.duration_sec, h.completed, h.updated_at
         FROM listen_history h {filter}
         ORDER BY h.updated_at DESC, h.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(user.id)
    .bind(user.id)
    .bind(admin)
    .bind(pagination.page_size)
    .bind(pagination.offset())
    .fetch_all(&state.db)
    .await?;

    let ids: Vec<i64> = rows.iter().map(|row| row.track_id).collect();
    let tracks = hydrate::tracks_with_album(&state.db, &ids).await?;
    let items = rows
        .iter()
        .map(|row| history_out(row, tracks.get(&row.track_id).cloned()))
        .collect();
    Ok(ok(Page::new(items, total, pagination.page, pagination.page_size)))
}

async fn find_history(
    state: &AppState,
    user_id: i64,
    track_id: i64,
) -> ApiResult<Option<HistoryRow>> {
    Ok(sqlx::query_as::<_, HistoryRow>(
        "SELECT id, track_id, progress_sec, duration_sec, completed, updated_at
         FROM listen_history WHERE user_id = ? AND track_id = ?",
    )
    .bind(user_id)
    .bind(track_id)
    .fetch_optional(&state.db)
    .await?)
}

async fn track_history(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<Option<HistoryOut>>> {
    visible_track(&state.db, track_id, Some(&user)).await?;
    let Some(row) = find_history(&state, user.id, track_id).await? else {
        return Ok(ok(None));
    };
    let tracks = hydrate::tracks_with_album(&state.db, &[row.track_id]).await?;
    Ok(ok(Some(history_out(&row, tracks.get(&row.track_id).cloned()))))
}

async fn last_album_history(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<Option<HistoryOut>>> {
    visible_album(&state.db, album_id, Some(&user)).await?;
    let row = sqlx::query_as::<_, HistoryRow>(
        "SELECT h.id, h.track_id, h.progress_sec, h.duration_sec, h.completed, h.updated_at
         FROM listen_history h JOIN tracks t ON t.id = h.track_id
         WHERE h.user_id = ? AND t.album_id = ?
         ORDER BY h.updated_at DESC LIMIT 1",
    )
    .bind(user.id)
    .bind(album_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = row else { return Ok(ok(None)) };
    let tracks = hydrate::tracks_with_album(&state.db, &[row.track_id]).await?;
    Ok(ok(Some(history_out(&row, tracks.get(&row.track_id).cloned()))))
}

async fn upsert_history(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<HistoryUpsert>,
) -> ApiResult<crate::error::ApiOk<HistoryOut>> {
    if payload.progress_sec < 0.0 {
        return Err(AppError::validation("body.progress_sec", "必须大于等于 0"));
    }
    if payload.duration_sec < 0.0 {
        return Err(AppError::validation("body.duration_sec", "必须大于等于 0"));
    }
    let (track, album) = visible_track(&state.db, payload.track_id, Some(&user)).await?;
    let existing = find_history(&state, user.id, track.id).await?;
    let now = now_text();

    let progress = payload.progress_sec.max(0.0);
    let duration = payload.duration_sec.max(existing.as_ref().map(|row| row.duration_sec).unwrap_or(0.0));
    let completed = match payload.completed {
        Some(value) => value,
        None => duration > 0.0 && progress / duration >= 0.95,
    };

    let mut tx = state.db.begin().await?;
    let history_id = match &existing {
        Some(row) => {
            sqlx::query(
                "UPDATE listen_history SET progress_sec = ?, duration_sec = ?, completed = ?, updated_at = ?
                 WHERE id = ?",
            )
            .bind(progress)
            .bind(duration)
            .bind(if completed { 1 } else { 0 })
            .bind(&now)
            .bind(row.id)
            .execute(&mut *tx)
            .await?;
            row.id
        }
        None => sqlx::query_scalar(
            "INSERT INTO listen_history (user_id, track_id, progress_sec, duration_sec, completed,
                                         created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(user.id)
        .bind(track.id)
        .bind(progress)
        .bind(duration)
        .bind(if completed { 1 } else { 0 })
        .bind(&now)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await?,
    };

    // 歌曲时长属于专辑元数据：只有专辑创建者/管理员上报才写回。
    if payload.duration_sec > 0.0
        && track.duration_sec.is_none()
        && can_manage_authored_content(&user, None, album.owner_id)
    {
        sqlx::query("UPDATE tracks SET duration_sec = ? WHERE id = ?")
            .bind(payload.duration_sec.min(24.0 * 3600.0))
            .bind(track.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let row = sqlx::query_as::<_, HistoryRow>(
        "SELECT id, track_id, progress_sec, duration_sec, completed, updated_at
         FROM listen_history WHERE id = ?",
    )
    .bind(history_id)
    .fetch_one(&state.db)
    .await?;
    let tracks = hydrate::tracks_with_album(&state.db, &[row.track_id]).await?;
    Ok(ok(history_out(&row, tracks.get(&row.track_id).cloned())))
}

async fn delete_history(
    State(state): State<AppState>,
    ApiPath(history_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let result = sqlx::query("DELETE FROM listen_history WHERE id = ? AND user_id = ?")
        .bind(history_id)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::not_found("记录不存在"));
    }
    Ok(ok(serde_json::json!({ "deleted": history_id })))
}

async fn delete_album_history(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let result = sqlx::query(
        "DELETE FROM listen_history
         WHERE user_id = ? AND track_id IN (SELECT id FROM tracks WHERE album_id = ?)",
    )
    .bind(user.id)
    .bind(album_id)
    .execute(&state.db)
    .await?;
    Ok(ok(serde_json::json!({ "deleted": result.rows_affected() as i64 })))
}

async fn clear_history(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let result = sqlx::query("DELETE FROM listen_history WHERE user_id = ?")
        .bind(user.id)
        .execute(&state.db)
        .await?;
    Ok(ok(serde_json::json!({ "deleted": result.rows_affected() as i64 })))
}
