//! 收藏与收藏列表：收藏必须归属至少一个自建列表；列表内顺序由 position 决定。

use std::collections::HashMap;

use axum::{
    extract::State,
    routing::{get, patch, put},
    Router,
};
use serde::Deserialize;

use crate::dto::{FavoriteCheck, FavoriteListOut, FavoriteOut, Page};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiJson, ApiPath, ApiQuery, CurrentUser, Pagination};
use crate::hydrate;
use crate::models::{FavoriteListRow, FavoriteRow};
use crate::payload::{
    FavoriteCreate, FavoriteListCreate, FavoriteListOrderUpdate, FavoriteListUpdate,
    FavoriteOrderUpdate, FavoriteUpdate,
};
use crate::services::visible_track;
use crate::state::AppState;
use crate::timeutil::{now_text, normalize_input, to_iso};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/favorites", get(list_favorites).post(add_favorite))
        .route("/favorites/check-batch", get(check_batch))
        .route("/favorites/check/{track_id}", get(check_favorite))
        .route("/favorites/{track_id}", patch(update_favorite).delete(remove_favorite))
}

pub fn lists_router() -> Router<AppState> {
    Router::new()
        .route("/favorite-lists", get(list_favorite_lists).post(create_favorite_list))
        .route("/favorite-lists/order", put(reorder_lists))
        .route(
            "/favorite-lists/{list_id}",
            patch(rename_favorite_list).delete(delete_favorite_list),
        )
        .route("/favorite-lists/{list_id}/order", put(reorder_list_tracks))
}

fn visible_favorite_sql() -> String {
    crate::services::visible_album_of_track_sql("f.track_id")
}

// ------------------------------ 收藏列表 ------------------------------

async fn list_favorite_lists(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<Vec<FavoriteListOut>>> {
    let rows = sqlx::query_as::<_, FavoriteListRow>(
        "SELECT id, name, position, created_at, updated_at FROM favorite_lists
         WHERE user_id = ? ORDER BY position, id",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let list_ids: Vec<i64> = rows.iter().map(|row| row.id).collect();

    let mut counts: HashMap<i64, i64> = HashMap::new();
    let mut covers: HashMap<i64, Option<String>> = HashMap::new();
    if !list_ids.is_empty() {
        let marks = crate::sqlutil::placeholders(list_ids.len());
        let sql = format!(
            "SELECT fi.list_id, COUNT(fi.id)
             FROM favorite_items fi JOIN favorites f ON f.id = fi.favorite_id
             WHERE fi.list_id IN ({marks}) AND f.user_id = ? AND {}
             GROUP BY fi.list_id",
            visible_favorite_sql()
        );
        let mut query = sqlx::query_as::<_, (i64, i64)>(&sql);
        for id in &list_ids {
            query = query.bind(id);
        }
        let (uid, admin) = crate::services::viewer_params(Some(&user));
        for (list_id, count) in query.bind(uid).bind(admin).fetch_all(&state.db).await? {
            counts.insert(list_id, count);
        }

        let sql = format!(
            "SELECT fi.list_id, al.cover_url
             FROM favorite_items fi
             JOIN favorites f ON f.id = fi.favorite_id
             JOIN tracks t ON t.id = f.track_id
             JOIN albums al ON al.id = t.album_id
             WHERE fi.list_id IN ({marks}) AND f.user_id = ? AND {}
             ORDER BY fi.list_id, fi.position, fi.id",
            visible_favorite_sql()
        );
        let mut query = sqlx::query_as::<_, (i64, Option<String>)>(&sql);
        for id in &list_ids {
            query = query.bind(id);
        }
        for (list_id, cover) in query.bind(uid).bind(admin).fetch_all(&state.db).await? {
            covers.entry(list_id).or_insert(cover);
        }
    }

    Ok(ok(rows
        .iter()
        .map(|row| FavoriteListOut {
            id: row.id,
            name: row.name.clone(),
            count: counts.get(&row.id).copied().unwrap_or(0),
            cover_url: covers.get(&row.id).cloned().flatten(),
            created_at: to_iso(&row.created_at),
            updated_at: to_iso(&row.updated_at),
        })
        .collect()))
}

async fn create_favorite_list(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<FavoriteListCreate>,
) -> ApiResult<crate::error::ApiOk<FavoriteListOut>> {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad_request("列表名称不能为空"));
    }
    if name.chars().count() > 64 {
        return Err(AppError::validation("body.name", "长度不能超过 64 个字符"));
    }
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM favorite_lists WHERE user_id = ? AND name = ?")
            .bind(user.id)
            .bind(&name)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_some() {
        return Err(AppError::conflict("已有同名列表"));
    }
    let last: Option<i64> =
        sqlx::query_scalar("SELECT MAX(position) FROM favorite_lists WHERE user_id = ?")
            .bind(user.id)
            .fetch_one(&state.db)
            .await?;
    let now = now_text();
    let list_id: i64 = sqlx::query_scalar(
        "INSERT INTO favorite_lists (user_id, name, position, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user.id)
    .bind(name.chars().take(64).collect::<String>())
    .bind(last.map(|value| value + 1).unwrap_or(0))
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    tracing::info!("创建收藏列表 id={} by={}", list_id, user.uuid);
    Ok(ok(FavoriteListOut {
        id: list_id,
        name: name.chars().take(64).collect(),
        count: 0,
        cover_url: None,
        created_at: to_iso(&now),
        updated_at: to_iso(&now),
    }))
}

async fn get_list_or_404(state: &AppState, user_id: i64, list_id: i64) -> ApiResult<FavoriteListRow> {
    sqlx::query_as::<_, FavoriteListRow>(
        "SELECT id, name, position, created_at, updated_at FROM favorite_lists
         WHERE id = ? AND user_id = ?",
    )
    .bind(list_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::not_found("收藏列表不存在"))
}

async fn reorder_lists(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<FavoriteListOrderUpdate>,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let ids = payload.list_ids;
    if ids.is_empty() {
        return Err(AppError::validation("body.list_ids", "至少需要一个列表 id"));
    }
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != ids.len() {
        return Err(AppError::bad_request("歌单顺序不能包含重复 id"));
    }
    let rows = sqlx::query_as::<_, FavoriteListRow>(
        "SELECT id, name, position, created_at, updated_at FROM favorite_lists WHERE user_id = ?",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let mut existing: Vec<i64> = rows.iter().map(|row| row.id).collect();
    existing.sort_unstable();
    let mut wanted = sorted.clone();
    wanted.sort_unstable();
    if existing != wanted {
        return Err(AppError::bad_request("排序必须包含当前账号的全部歌单"));
    }

    let mut tx = state.db.begin().await?;
    for (index, list_id) in ids.iter().enumerate() {
        sqlx::query("UPDATE favorite_lists SET position = ?, updated_at = ? WHERE id = ?")
            .bind(index as i64)
            .bind(now_text())
            .bind(list_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    tracing::info!("调整歌单顺序 count={} by={}", ids.len(), user.uuid);
    Ok(ok(serde_json::json!({ "count": ids.len() })))
}

async fn rename_favorite_list(
    State(state): State<AppState>,
    ApiPath(list_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<FavoriteListUpdate>,
) -> ApiResult<crate::error::ApiOk<FavoriteListOut>> {
    let item = get_list_or_404(&state, user.id, list_id).await?;
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad_request("列表名称不能为空"));
    }
    if name.chars().count() > 64 {
        return Err(AppError::validation("body.name", "长度不能超过 64 个字符"));
    }
    let clash: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM favorite_lists WHERE user_id = ? AND name = ? AND id != ?",
    )
    .bind(user.id)
    .bind(&name)
    .bind(item.id)
    .fetch_optional(&state.db)
    .await?;
    if clash.is_some() {
        return Err(AppError::conflict("已有同名列表"));
    }
    let now = now_text();
    sqlx::query("UPDATE favorite_lists SET name = ?, updated_at = ? WHERE id = ?")
        .bind(name.chars().take(64).collect::<String>())
        .bind(&now)
        .bind(item.id)
        .execute(&state.db)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM favorite_items WHERE list_id = ?")
        .bind(item.id)
        .fetch_one(&state.db)
        .await?;
    Ok(ok(FavoriteListOut {
        id: item.id,
        name: name.chars().take(64).collect(),
        count,
        cover_url: None,
        created_at: to_iso(&item.created_at),
        updated_at: to_iso(&now),
    }))
}

async fn delete_favorite_list(
    State(state): State<AppState>,
    ApiPath(list_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let item = get_list_or_404(&state, user.id, list_id).await?;
    let affected: Vec<i64> = sqlx::query_scalar("SELECT favorite_id FROM favorite_items WHERE list_id = ?")
        .bind(item.id)
        .fetch_all(&state.db)
        .await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM favorite_lists WHERE id = ?")
        .bind(item.id)
        .execute(&mut *tx)
        .await?;
    purge_orphans(&mut tx, &affected).await?;
    tx.commit().await?;
    tracing::info!("删除收藏列表 id={} by={}", list_id, user.uuid);
    Ok(ok(serde_json::json!({ "deleted": list_id })))
}

async fn purge_orphans(
    exec: &mut sqlx::SqliteConnection,
    favorite_ids: &[i64],
) -> Result<(), sqlx::Error> {
    if favorite_ids.is_empty() {
        return Ok(());
    }
    let marks = crate::sqlutil::placeholders(favorite_ids.len());
    let sql = format!(
        "DELETE FROM favorites WHERE id IN ({marks})
         AND NOT EXISTS (SELECT 1 FROM favorite_items fi WHERE fi.favorite_id = favorites.id)"
    );
    let mut query = sqlx::query(&sql);
    for id in favorite_ids {
        query = query.bind(id);
    }
    query.execute(&mut *exec).await?;
    Ok(())
}

async fn reorder_list_tracks(
    State(state): State<AppState>,
    ApiPath(list_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<FavoriteOrderUpdate>,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let item = get_list_or_404(&state, user.id, list_id).await?;
    let track_ids = payload.track_ids;
    if track_ids.is_empty() {
        return Err(AppError::validation("body.track_ids", "至少需要一首歌曲"));
    }
    let mut sorted = track_ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() != track_ids.len() {
        return Err(AppError::bad_request("歌曲顺序不能包含重复的歌曲"));
    }

    let (uid, admin) = crate::services::viewer_params(Some(&user));
    let rows = sqlx::query_as::<_, (i64, i64)>(
        "SELECT fi.id, f.track_id
         FROM favorite_items fi JOIN favorites f ON f.id = fi.favorite_id
         WHERE fi.list_id = ? AND EXISTS (
             SELECT 1 FROM tracks vt JOIN albums va ON va.id = vt.album_id
             WHERE vt.id = f.track_id AND (va.is_public = 1 OR va.owner_id = ? OR ? = 1))",
    )
    .bind(item.id)
    .bind(uid)
    .bind(admin)
    .fetch_all(&state.db)
    .await?;
    let mut by_track: HashMap<i64, i64> = rows.into_iter().map(|(id, track)| (track, id)).collect();
    let mut wanted = track_ids.clone();
    wanted.sort_unstable();
    let mut current: Vec<i64> = by_track.keys().copied().collect();
    current.sort_unstable();
    if current != wanted {
        return Err(AppError::bad_request("排序必须包含该歌单内的全部歌曲"));
    }

    let mut tx = state.db.begin().await?;
    for (index, track_id) in track_ids.iter().enumerate() {
        if let Some(item_id) = by_track.remove(track_id) {
            sqlx::query("UPDATE favorite_items SET position = ? WHERE id = ?")
                .bind(index as i64)
                .bind(item_id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    tracing::info!("调整歌单内顺序 list={} count={} by={}", list_id, track_ids.len(), user.uuid);
    Ok(ok(serde_json::json!({ "count": track_ids.len() })))
}

// -------------------------------- 收藏 --------------------------------

async fn find_favorite(
    state: &AppState,
    user_id: i64,
    track_id: i64,
) -> ApiResult<Option<FavoriteRow>> {
    Ok(sqlx::query_as::<_, FavoriteRow>(
        "SELECT f.id, f.track_id, f.collected_at, f.created_at
         FROM favorites f
         WHERE f.user_id = ? AND f.track_id = ?",
    )
    .bind(user_id)
    .bind(track_id)
    .fetch_optional(&state.db)
    .await?)
}

async fn list_ids_of(state: &AppState, favorite_id: i64) -> ApiResult<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT list_id FROM favorite_items WHERE favorite_id = ? ORDER BY list_id")
        .bind(favorite_id)
        .fetch_all(&state.db)
        .await?)
}

#[derive(Debug, Deserialize, Default)]
struct FavoriteQuery {
    #[serde(default)]
    list_id: Option<i64>,
}

async fn list_favorites(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    pagination: Pagination,
    ApiQuery(query): ApiQuery<FavoriteQuery>,
) -> ApiResult<crate::error::ApiOk<Page<FavoriteOut>>> {
    let (uid, admin) = crate::services::viewer_params(Some(&user));
    let mut filter = format!("WHERE f.user_id = ? AND {}", visible_favorite_sql());
    let mut params: Vec<i64> = vec![user.id, uid, admin];
    let mut order = "ORDER BY COALESCE(f.collected_at, f.created_at) DESC, f.id DESC".to_string();

    if let Some(list_id) = query.list_id {
        get_list_or_404(&state, user.id, list_id).await?;
        filter.push_str(" AND f.id IN (SELECT favorite_id FROM favorite_items WHERE list_id = ?)");
        params.push(list_id);
        order = "ORDER BY (SELECT position FROM favorite_items fi WHERE fi.favorite_id = f.id AND fi.list_id = ?), \
                 (SELECT id FROM favorite_items fi WHERE fi.favorite_id = f.id AND fi.list_id = ?)"
            .to_string();
    }

    let count_sql = format!("SELECT COUNT(*) FROM favorites f {filter}");
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    for value in &params {
        count_query = count_query.bind(value);
    }
    let total = count_query.fetch_one(&state.db).await?;

    let mut page_params = params.clone();
    if let Some(list_id) = query.list_id {
        page_params.push(list_id);
        page_params.push(list_id);
    }
    page_params.push(pagination.page_size);
    page_params.push(pagination.offset());

    let sql = format!(
        "SELECT f.id, f.track_id, f.collected_at, f.created_at FROM favorites f {filter} {order} LIMIT ? OFFSET ?"
    );
    let mut page_query = sqlx::query_as::<_, FavoriteRow>(&sql);
    for value in &page_params {
        page_query = page_query.bind(value);
    }
    let rows = page_query.fetch_all(&state.db).await?;

    let favorite_ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let mut memberships: HashMap<i64, Vec<i64>> = HashMap::new();
    if !favorite_ids.is_empty() {
        let marks = crate::sqlutil::placeholders(favorite_ids.len());
        let sql = format!(
            "SELECT favorite_id, list_id FROM favorite_items WHERE favorite_id IN ({marks}) ORDER BY list_id"
        );
        let mut query = sqlx::query_as::<_, (i64, i64)>(&sql);
        for id in &favorite_ids {
            query = query.bind(id);
        }
        for (favorite_id, list_id) in query.fetch_all(&state.db).await? {
            memberships.entry(favorite_id).or_default().push(list_id);
        }
    }

    let track_ids: Vec<i64> = rows.iter().map(|row| row.track_id).collect();
    let tracks = hydrate::tracks_with_album(&state.db, &track_ids).await?;

    let items = rows
        .iter()
        .map(|row| FavoriteOut {
            id: row.id,
            track_id: row.track_id,
            created_at: to_iso(&row.created_at),
            collected_at: row.collected_at.as_deref().map(to_iso),
            list_ids: memberships.get(&row.id).cloned().unwrap_or_default(),
            track: tracks.get(&row.track_id).cloned(),
        })
        .collect();

    Ok(ok(Page::new(items, total, pagination.page, pagination.page_size)))
}

async fn prepend_position(exec: &mut sqlx::SqliteConnection, list_id: i64) -> Result<i64, sqlx::Error> {
    let lowest: Option<i64> =
        sqlx::query_scalar("SELECT MIN(position) FROM favorite_items WHERE list_id = ?")
            .bind(list_id)
            .fetch_one(&mut *exec)
            .await?;
    Ok(lowest.unwrap_or(0) - 1)
}

async fn set_memberships(
    exec: &mut sqlx::SqliteConnection,
    user_id: i64,
    favorite_id: i64,
    list_ids: &[i64],
) -> ApiResult<()> {
    let mut ordered: Vec<i64> = Vec::new();
    for id in list_ids {
        if !ordered.contains(id) {
            ordered.push(*id);
        }
    }
    if ordered.is_empty() {
        return Err(AppError::bad_request("收藏至少需要归属一个列表"));
    }

    let marks = crate::sqlutil::placeholders(ordered.len());
    let sql = format!("SELECT id FROM favorite_lists WHERE user_id = ? AND id IN ({marks})");
    let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(user_id);
    for id in &ordered {
        query = query.bind(id);
    }
    let found = query.fetch_all(&mut *exec).await?;
    if let Some(missing) = ordered.iter().find(|id| !found.contains(id)) {
        return Err(AppError::not_found(format!("收藏列表不存在：{missing}")));
    }

    let existing: Vec<i64> = sqlx::query_scalar("SELECT list_id FROM favorite_items WHERE favorite_id = ?")
        .bind(favorite_id)
        .fetch_all(&mut *exec)
        .await?;

    for list_id in &existing {
        if !ordered.contains(list_id) {
            sqlx::query("DELETE FROM favorite_items WHERE favorite_id = ? AND list_id = ?")
                .bind(favorite_id)
                .bind(list_id)
                .execute(&mut *exec)
                .await?;
        }
    }
    for list_id in &ordered {
        if existing.contains(list_id) {
            continue;
        }
        let position = prepend_position(exec, *list_id).await?;
        sqlx::query(
            "INSERT INTO favorite_items (list_id, favorite_id, position, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(list_id)
        .bind(favorite_id)
        .bind(position)
        .bind(now_text())
        .execute(&mut *exec)
        .await?;
    }
    Ok(())
}

async fn add_favorite(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<FavoriteCreate>,
) -> ApiResult<crate::error::ApiOk<FavoriteOut>> {
    let (track, _) = visible_track(&state.db, payload.track_id, Some(&user)).await?;
    if find_favorite(&state, user.id, track.id).await?.is_some() {
        return Err(AppError::conflict("已经收藏过了"));
    }
    if payload.list_ids.is_empty() {
        return Err(AppError::validation("body.list_ids", "至少选择一个列表"));
    }
    let collected_at = match payload.collected_at.as_deref() {
        Some(raw) => Some(
            normalize_input(raw)
                .ok_or_else(|| AppError::validation("body.collected_at", "时间格式不正确"))?,
        ),
        None => Some(now_text()),
    };

    let now = now_text();
    let mut tx = state.db.begin().await?;
    let favorite_id: i64 = sqlx::query_scalar(
        "INSERT INTO favorites (user_id, track_id, collected_at, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user.id)
    .bind(track.id)
    .bind(&collected_at)
    .bind(&now)
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;
    set_memberships(&mut tx, user.id, favorite_id, &payload.list_ids).await?;
    tx.commit().await?;

    tracing::info!("添加收藏 track={} by={}", track.id, user.uuid);
    let row = find_favorite(&state, user.id, track.id)
        .await?
        .ok_or_else(|| AppError::not_found("尚未收藏该歌曲"))?;
    let tracks = hydrate::tracks_with_album(&state.db, &[row.track_id]).await?;
    Ok(ok(FavoriteOut {
        id: row.id,
        track_id: row.track_id,
        created_at: to_iso(&row.created_at),
        collected_at: row.collected_at.as_deref().map(to_iso),
        list_ids: list_ids_of(&state, row.id).await?,
        track: tracks.get(&row.track_id).cloned(),
    }))
}

async fn update_favorite(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<FavoriteUpdate>,
) -> ApiResult<crate::error::ApiOk<FavoriteOut>> {
    let favorite = find_favorite(&state, user.id, track_id)
        .await?
        .ok_or_else(|| AppError::not_found("尚未收藏该歌曲"))?;
    // 私有专辑中的歌曲对非所有者表现为不存在
    if visible_track(&state.db, track_id, Some(&user)).await.is_err() {
        return Err(AppError::not_found("尚未收藏该歌曲"));
    }

    let collected_at = match payload.collected_at.as_deref() {
        Some(raw) => Some(
            normalize_input(raw)
                .ok_or_else(|| AppError::validation("body.collected_at", "时间格式不正确"))?,
        ),
        None => favorite.collected_at.clone(),
    };

    let mut tx = state.db.begin().await?;
    if let Some(list_ids) = &payload.list_ids {
        if list_ids.is_empty() {
            return Err(AppError::validation("body.list_ids", "至少选择一个列表"));
        }
        set_memberships(&mut tx, user.id, favorite.id, list_ids).await?;
    }
    sqlx::query("UPDATE favorites SET collected_at = ?, updated_at = ? WHERE id = ?")
        .bind(&collected_at)
        .bind(now_text())
        .bind(favorite.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let row = find_favorite(&state, user.id, track_id)
        .await?
        .ok_or_else(|| AppError::not_found("尚未收藏该歌曲"))?;
    let tracks = hydrate::tracks_with_album(&state.db, &[row.track_id]).await?;
    Ok(ok(FavoriteOut {
        id: row.id,
        track_id: row.track_id,
        created_at: to_iso(&row.created_at),
        collected_at: row.collected_at.as_deref().map(to_iso),
        list_ids: list_ids_of(&state, row.id).await?,
        track: tracks.get(&row.track_id).cloned(),
    }))
}

async fn remove_favorite(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let favorite = find_favorite(&state, user.id, track_id)
        .await?
        .ok_or_else(|| AppError::not_found("尚未收藏该歌曲"))?;
    sqlx::query("DELETE FROM favorites WHERE id = ?")
        .bind(favorite.id)
        .execute(&state.db)
        .await?;
    Ok(ok(serde_json::json!({ "deleted": track_id })))
}

async fn check_favorite(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<FavoriteCheck>> {
    let favorite = find_favorite(&state, user.id, track_id).await?;
    let Some(favorite) = favorite else {
        return Ok(ok(FavoriteCheck {
            track_id,
            favorited: false,
            list_ids: Vec::new(),
            collected_at: None,
        }));
    };
    if visible_track(&state.db, track_id, Some(&user)).await.is_err() {
        return Ok(ok(FavoriteCheck {
            track_id,
            favorited: false,
            list_ids: Vec::new(),
            collected_at: None,
        }));
    }
    Ok(ok(FavoriteCheck {
        track_id,
        favorited: true,
        list_ids: list_ids_of(&state, favorite.id).await?,
        collected_at: favorite.collected_at.as_deref().map(to_iso),
    }))
}

#[derive(Debug, Deserialize)]
struct BatchQuery {
    #[serde(default)]
    track_ids: String,
}

async fn check_batch(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiQuery(query): ApiQuery<BatchQuery>,
) -> ApiResult<crate::error::ApiOk<Vec<FavoriteCheck>>> {
    let ids: Vec<i64> = query
        .track_ids
        .split(',')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .take(200)
        .collect();
    if ids.is_empty() {
        return Ok(ok(Vec::new()));
    }

    let (uid, admin) = crate::services::viewer_params(Some(&user));
    let marks = crate::sqlutil::placeholders(ids.len());
    let sql = format!(
        "SELECT f.id, f.track_id, f.collected_at, f.created_at
         FROM favorites f
         WHERE f.user_id = ? AND f.track_id IN ({marks}) AND {}",
        visible_favorite_sql()
    );
    let mut db_query = sqlx::query_as::<_, FavoriteRow>(&sql).bind(user.id);
    for id in &ids {
        db_query = db_query.bind(id);
    }
    let rows = db_query.bind(uid).bind(admin).fetch_all(&state.db).await?;

    let mut by_track: HashMap<i64, FavoriteRow> =
        rows.into_iter().map(|row| (row.track_id, row)).collect();
    let mut result = Vec::with_capacity(ids.len());
    for track_id in ids {
        let Some(favorite) = by_track.remove(&track_id) else {
            result.push(FavoriteCheck { track_id, favorited: false, list_ids: Vec::new(), collected_at: None });
            continue;
        };
        result.push(FavoriteCheck {
            track_id,
            favorited: true,
            list_ids: list_ids_of(&state, favorite.id).await?,
            collected_at: favorite.collected_at.as_deref().map(to_iso),
        });
    }
    Ok(ok(result))
}
