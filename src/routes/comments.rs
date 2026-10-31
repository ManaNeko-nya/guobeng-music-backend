//! 歌曲评论：主楼分页、一层回复、作者/专辑主/管理员删除。

use axum::{
    extract::State,
    routing::{delete, get},
    Router,
};
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use serde::Deserialize;

use crate::dto::{CommentOut, Page};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiJson, ApiPath, ApiQuery, CurrentUser, OptionalUser, Pagination};
use crate::models::CommentRow;
use crate::payload::CommentCreate;
use crate::repo::{COMMENT_SELECT, fetch_comment};
use crate::services::{
    can_manage_authored_content, require_track_manager, visible_track,
};
use crate::state::AppState;
use crate::timeutil::now_text;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/tracks/{track_id}/comments",
            get(list_comments).post(create_comment).delete(clear_comments),
        )
        .route("/comments/{comment_id}", delete(delete_comment))
}

#[derive(Debug, Deserialize, Default)]
struct CommentQuery {
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    seed: Option<i64>,
}

async fn fetch_comments(state: &AppState, sql: &str, binds: &[i64]) -> ApiResult<Vec<CommentRow>> {
    let mut query = sqlx::query_as::<_, CommentRow>(sql);
    for value in binds {
        query = query.bind(value);
    }
    Ok(query.fetch_all(&state.db).await?)
}

async fn list_comments(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    OptionalUser(user): OptionalUser,
    pagination: Pagination,
    ApiQuery(query): ApiQuery<CommentQuery>,
) -> ApiResult<crate::error::ApiOk<Page<CommentOut>>> {
    visible_track(&state.db, track_id, user.as_ref()).await?;

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM comments WHERE track_id = ? AND parent_id IS NULL",
    )
    .bind(track_id)
    .fetch_one(&state.db)
    .await?;

    let roots = if query.sort.as_deref() == Some("time") {
        fetch_comments(
            &state,
            &format!(
                "{COMMENT_SELECT} WHERE c.track_id = ? AND c.parent_id IS NULL
                 ORDER BY c.created_at DESC, c.id DESC LIMIT ? OFFSET ?"
            ),
            &[track_id, pagination.page_size, pagination.offset()],
        )
        .await?
    } else {
        let mut ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM comments WHERE track_id = ? AND parent_id IS NULL",
        )
        .bind(track_id)
        .fetch_all(&state.db)
        .await?;
        match query.seed {
            Some(seed) => ids.shuffle(&mut StdRng::seed_from_u64(seed as u64)),
            None => ids.shuffle(&mut rand::thread_rng()),
        }
        let page_ids: Vec<i64> = ids
            .into_iter()
            .skip(pagination.offset().max(0) as usize)
            .take(pagination.page_size as usize)
            .collect();
        if page_ids.is_empty() {
            Vec::new()
        } else {
            let marks = crate::sqlutil::placeholders(page_ids.len());
            let rows = fetch_comments(
                &state,
                &format!("{COMMENT_SELECT} WHERE c.id IN ({marks})"),
                &page_ids,
            )
            .await?;
            let mut by_id: std::collections::HashMap<i64, CommentRow> =
                rows.into_iter().map(|row| (row.id, row)).collect();
            page_ids
                .iter()
                .filter_map(|id| by_id.remove(id))
                .collect()
        }
    };

    let mut replies: std::collections::HashMap<i64, Vec<CommentOut>> =
        roots.iter().map(|row| (row.id, Vec::new())).collect();
    if !roots.is_empty() {
        let root_ids: Vec<i64> = roots.iter().map(|row| row.id).collect();
        let marks = crate::sqlutil::placeholders(root_ids.len());
        let children = fetch_comments(
            &state,
            &format!(
                "{COMMENT_SELECT} WHERE c.parent_id IN ({marks})
                 ORDER BY c.created_at, c.id"
            ),
            &root_ids,
        )
        .await?;
        for child in children {
            if let Some(parent) = child.parent_id {
                if let Some(list) = replies.get_mut(&parent) {
                    list.push(CommentOut::from_row(&child));
                }
            }
        }
    }

    let items: Vec<CommentOut> = roots
        .iter()
        .map(|row| {
            let mut out = CommentOut::from_row(row);
            out.replies = replies.remove(&row.id).unwrap_or_default();
            out
        })
        .collect();

    Ok(ok(Page::new(items, total, pagination.page, pagination.page_size)))
}

async fn create_comment(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<CommentCreate>,
) -> ApiResult<crate::error::ApiOk<CommentOut>> {
    visible_track(&state.db, track_id, Some(&user)).await?;

    let content = payload.content.trim().to_string();
    if content.is_empty() {
        return Err(AppError::bad_request("评论内容不能为空"));
    }
    if content.chars().count() > 1000 {
        return Err(AppError::validation("body.content", "长度不能超过 1000 个字符"));
    }

    let mut parent_id = payload.parent_id;
    if let Some(id) = parent_id {
        let parent = fetch_comment(&state.db, id)
            .await?
            .filter(|row| row.track_id == track_id)
            .ok_or_else(|| AppError::not_found("要回复的评论不存在"))?;
        // 回复统一挂到主楼，避免无限层级把列表压扁
        parent_id = parent.parent_id.or(Some(parent.id));
    }

    let now = now_text();
    let comment_id: i64 = sqlx::query_scalar(
        "INSERT INTO comments (track_id, user_id, parent_id, content, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(track_id)
    .bind(user.id)
    .bind(parent_id)
    .bind(content.chars().take(1000).collect::<String>())
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    tracing::info!("新增评论 id={} track={} user={}", comment_id, track_id, user.uuid);
    let row = fetch_comment(&state.db, comment_id)
        .await?
        .ok_or_else(|| AppError::not_found("评论不存在"))?;
    Ok(ok(CommentOut::from_row(&row)))
}

async fn delete_comment(
    State(state): State<AppState>,
    ApiPath(comment_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let comment = fetch_comment(&state.db, comment_id)
        .await?
        .ok_or_else(|| AppError::not_found("评论不存在"))?;
    let (_, album) = visible_track(&state.db, comment.track_id, Some(&user)).await?;
    if !can_manage_authored_content(&user, comment.user_id, album.owner_id) {
        return Err(AppError::forbidden("只能删除自己发表的评论"));
    }

    sqlx::query("DELETE FROM comments WHERE id = ?")
        .bind(comment_id)
        .execute(&state.db)
        .await?;
    tracing::info!("删除评论 id={} by={}", comment_id, user.uuid);
    Ok(ok(serde_json::json!({ "deleted": comment_id })))
}

async fn clear_comments(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let (track, album) = visible_track(&state.db, track_id, Some(&user)).await?;
    require_track_manager(&user, &album)?;

    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM comments WHERE track_id = ?")
            .bind(track.id)
            .fetch_one(&state.db)
            .await?;
    sqlx::query("DELETE FROM comments WHERE track_id = ?")
        .bind(track.id)
        .execute(&state.db)
        .await?;
    tracing::info!("清空评论 track={} count={} by={}", track.id, total, user.uuid);
    Ok(ok(serde_json::json!({ "deleted": total })))
}
