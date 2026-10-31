//! 专辑接口：列表/搜索/推荐、详情、创建与维护。

use axum::{
    extract::State,
    routing::get,
    Router,
};
use rand::{rngs::StdRng, seq::SliceRandom, SeedableRng};
use serde::Deserialize;

use crate::dto::{AlbumBrief, AlbumDetail, Page};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiJson, ApiPath, ApiQuery, CurrentUser, OptionalUser, Pagination};
use crate::hydrate;
use crate::media::delete_media_file;
use crate::models::AlbumRow;
use crate::payload::{
    normalize_tags, parse_release_date, validate_media_url, AlbumCreate, AlbumUpdate,
};
use crate::repo::{list_tracks, ALBUM_SELECT};
use crate::services::{
    album_media_urls, album_visible, like_pattern, require_album_owner,
    require_owner_or_admin, set_album_tags, sync_album_artists, visible_album,
};
use crate::sqlutil::{bind_all, Param};
use crate::state::AppState;
use crate::timeutil::now_text;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/albums", get(list_albums).post(create_album))
        .route("/albums/search", get(search_albums))
        .route("/albums/mine", get(my_albums))
        .route("/albums/recommendations", get(recommendations))
        .route("/albums/by-external/{external_id}", get(by_external))
        .route("/albums/{album_id}", get(get_album).patch(update_album).delete(delete_album))
}

#[derive(Debug, Deserialize, Default)]
struct AlbumQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    artist_id: Option<i64>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    year: Option<i64>,
    #[serde(default)]
    album_type: Option<String>,
}

struct AlbumFilter {
    sql: String,
    params: Vec<Param>,
}

fn build_filter(query: &AlbumQuery) -> AlbumFilter {
    let mut sql = String::from("WHERE al.is_public = 1");
    let mut params = Vec::new();

    let keyword = query.q.trim();
    if !keyword.is_empty() {
        let pattern = like_pattern(keyword);
        sql.push_str(
            " AND (al.title LIKE ? ESCAPE '\\' OR al.description LIKE ? ESCAPE '\\' \
             OR EXISTS (SELECT 1 FROM artists a WHERE a.id = al.artist_id AND a.name LIKE ? ESCAPE '\\') \
             OR EXISTS (SELECT 1 FROM album_tags t WHERE t.album_id = al.id AND t.name LIKE ? ESCAPE '\\'))",
        );
        for _ in 0..4 {
            params.push(Param::Text(pattern.clone()));
        }
    }
    if let Some(artist_id) = query.artist_id {
        sql.push_str(" AND al.id IN (SELECT album_id FROM album_artists WHERE artist_id = ?)");
        params.push(Param::Int(artist_id));
    }
    if let Some(tag) = query.tag.as_deref().map(str::trim).filter(|tag| !tag.is_empty()) {
        sql.push_str(" AND EXISTS (SELECT 1 FROM album_tags t WHERE t.album_id = al.id AND t.name = ?)");
        params.push(Param::Text(tag.to_string()));
    }
    if let Some(year) = query.year {
        sql.push_str(" AND al.release_year = ?");
        params.push(Param::Int(year));
    }
    if let Some(album_type) = query
        .album_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        sql.push_str(" AND al.album_type = ?");
        params.push(Param::Text(album_type.to_string()));
    }
    AlbumFilter { sql, params }
}

async fn query_albums(
    state: &AppState,
    pagination: Pagination,
    query: AlbumQuery,
) -> ApiResult<Page<AlbumBrief>> {
    let filter = build_filter(&query);
    let count_sql = format!("SELECT COUNT(*) FROM albums al {}", filter.sql);
    let total = bind_all(sqlx::query_scalar::<_, i64>(&count_sql), &filter.params)
        .fetch_one(&state.db)
        .await?;

    let sort = query.sort.as_deref().unwrap_or("updated_at");
    let order = match sort {
        "created_at" => "al.created_at DESC, al.id DESC",
        "views" => "al.view_count DESC, al.id DESC",
        "score" => "al.score DESC, al.id DESC",
        "release" => "al.release_date DESC, al.id DESC",
        "title" => "al.title DESC, al.id DESC",
        _ => "al.updated_at DESC, al.id DESC",
    };
    let sql = format!("{ALBUM_SELECT} {} ORDER BY {order} LIMIT ? OFFSET ?", filter.sql);
    let mut params = filter.params;
    params.push(Param::Int(pagination.page_size));
    params.push(Param::Int(pagination.offset()));
    let rows = bind_all(sqlx::query_as::<_, AlbumRow>(&sql), &params)
        .fetch_all(&state.db)
        .await?;

    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let albums = hydrate::albums_brief(&state.db, &ids).await?;
    let items = ids.iter().filter_map(|id| albums.get(id).cloned()).collect();
    Ok(Page::new(items, total, pagination.page, pagination.page_size))
}

async fn list_albums(
    State(state): State<AppState>,
    pagination: Pagination,
    ApiQuery(query): ApiQuery<AlbumQuery>,
) -> ApiResult<crate::error::ApiOk<Page<AlbumBrief>>> {
    Ok(ok(query_albums(&state, pagination, query).await?))
}

async fn search_albums(
    State(state): State<AppState>,
    pagination: Pagination,
    ApiQuery(query): ApiQuery<AlbumQuery>,
) -> ApiResult<crate::error::ApiOk<Page<AlbumBrief>>> {
    Ok(ok(query_albums(&state, pagination, query).await?))
}

async fn my_albums(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    pagination: Pagination,
) -> ApiResult<crate::error::ApiOk<Page<AlbumBrief>>> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM albums WHERE owner_id = ?")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    let rows = sqlx::query_as::<_, AlbumRow>(&format!(
        "{ALBUM_SELECT} WHERE al.owner_id = ? ORDER BY al.updated_at DESC, al.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(user.id)
    .bind(pagination.page_size)
    .bind(pagination.offset())
    .fetch_all(&state.db)
    .await?;

    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let albums = hydrate::albums_brief(&state.db, &ids).await?;
    let items = ids.iter().filter_map(|id| albums.get(id).cloned()).collect();
    Ok(ok(Page::new(items, total, pagination.page, pagination.page_size)))
}

#[derive(Debug, Deserialize, Default)]
struct RecommendationQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    seed: Option<i64>,
    #[serde(default)]
    exclude: Option<String>,
}

async fn recommendations(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<RecommendationQuery>,
) -> ApiResult<crate::error::ApiOk<Vec<AlbumBrief>>> {
    let limit = query.limit.unwrap_or(12).clamp(1, 50);
    let exclude: Vec<i64> = query
        .exclude
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect();

    let mut sql = String::from("SELECT id FROM albums WHERE is_public = 1");
    let mut params = Vec::new();
    if !exclude.is_empty() {
        let marks = crate::sqlutil::placeholders(exclude.len());
        sql.push_str(&format!(" AND id NOT IN ({marks})"));
        params.extend(exclude.iter().copied().map(Param::Int));
    }
    let mut ids: Vec<i64> = bind_all(sqlx::query_scalar::<_, i64>(&sql), &params)
        .fetch_all(&state.db)
        .await?;
    if ids.is_empty() {
        return Ok(ok(Vec::new()));
    }

    let take = (limit as usize).min(ids.len());
    match query.seed {
        Some(seed) => {
            let mut rng = StdRng::seed_from_u64(seed as u64);
            ids.shuffle(&mut rng);
        }
        None => {
            let mut rng = rand::thread_rng();
            ids.shuffle(&mut rng);
        }
    }
    ids.truncate(take);

    let albums = hydrate::albums_brief(&state.db, &ids).await?;
    Ok(ok(ids.iter().filter_map(|id| albums.get(id).cloned()).collect()))
}

async fn album_detail(state: &AppState, album: &AlbumRow) -> ApiResult<AlbumDetail> {
    let brief = hydrate::albums_brief(&state.db, &[album.id])
        .await?
        .get(&album.id)
        .cloned()
        .ok_or_else(|| AppError::not_found("专辑不存在"))?;
    let rows = list_tracks(&state.db, album.id).await?;
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let tracks_map = hydrate::tracks_out(&state.db, &ids).await?;
    let tracks = ids.iter().filter_map(|id| tracks_map.get(id).cloned()).collect();

    Ok(AlbumDetail {
        base: brief,
        owner_id: hydrate::owner_uuid(&state.db, album.owner_id).await,
        tracks,
    })
}

#[derive(Debug, Deserialize, Default)]
struct DetailQuery {
    #[serde(default)]
    count_view: Option<bool>,
}

async fn get_album(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    OptionalUser(user): OptionalUser,
    ApiQuery(query): ApiQuery<DetailQuery>,
) -> ApiResult<crate::error::ApiOk<AlbumDetail>> {
    let album = visible_album(&state.db, album_id, user.as_ref()).await?;
    if query.count_view.unwrap_or(true) {
        sqlx::query("UPDATE albums SET view_count = view_count + 1 WHERE id = ?")
            .bind(album.id)
            .execute(&state.db)
            .await?;
    }
    let album = crate::repo::fetch_album(&state.db, album.id)
        .await?
        .ok_or_else(|| AppError::not_found("专辑不存在"))?;
    Ok(ok(album_detail(&state, &album).await?))
}

async fn by_external(
    State(state): State<AppState>,
    ApiPath(external_id): ApiPath<String>,
    OptionalUser(user): OptionalUser,
) -> ApiResult<crate::error::ApiOk<AlbumDetail>> {
    let key = external_id.trim().to_string();
    if key.is_empty() {
        return Err(AppError::not_found("专辑不存在"));
    }
    let album = sqlx::query_as::<_, AlbumRow>(&format!("{ALBUM_SELECT} WHERE al.external_id = ?"))
        .bind(&key)
        .fetch_optional(&state.db)
        .await?;
    let album = album.filter(|row| album_visible(row, user.as_ref()));
    let Some(album) = album else {
        return Err(AppError::not_found("专辑不存在"));
    };
    Ok(ok(album_detail(&state, &album).await?))
}

async fn create_album(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<AlbumCreate>,
) -> ApiResult<crate::error::ApiOk<AlbumDetail>> {
    crate::extract::require_admin_only_create(&state, &user, "专辑")?;

    let artist = crate::repo::fetch_artist(&state.db, payload.artist_id)
        .await?
        .ok_or_else(|| AppError::not_found("艺人不存在"))?;
    require_owner_or_admin(&user, artist.owner_id)?;

    let title = payload.title.trim().to_string();
    if title.is_empty() {
        return Err(AppError::bad_request("专辑标题不能为空"));
    }
    if title.chars().count() > 255 {
        return Err(AppError::validation("body.title", "长度不能超过 255 个字符"));
    }
    let cover = match payload.cover_url.as_deref() {
        Some(value) => validate_media_url("body.cover_url", value)?,
        None => None,
    };
    if let Some(score) = payload.score {
        if !(0.0..=10.0).contains(&score) {
            return Err(AppError::validation("body.score", "必须在 0-10 之间"));
        }
    }
    let external_id = payload
        .external_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    if let Some(external) = &external_id {
        if external.chars().count() > 64 {
            return Err(AppError::validation("body.external_id", "长度不能超过 64 个字符"));
        }
        let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM albums WHERE external_id = ?")
            .bind(external)
            .fetch_optional(&state.db)
            .await?;
        if exists.is_some() {
            return Err(AppError::conflict(format!("外部专辑标识已存在：{external}")));
        }
    }

    let release_date = match payload.release_date.as_deref() {
        Some(raw) => parse_release_date("body.release_date", raw)?,
        None => None,
    };
    let release_year = match &release_date {
        Some(date) => date[..4].parse::<i64>().ok(),
        None => payload.release_year,
    };
    let description = payload
        .description
        .as_deref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let tags = normalize_tags(payload.tags.as_ref());
    let now = now_text();

    let mut tx = state.db.begin().await?;
    let album_id: i64 = sqlx::query_scalar(
        "INSERT INTO albums (artist_id, title, external_id, cover_url, description, album_type,
                             release_year, release_date, score, owner_id, view_count, is_public,
                             created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?) RETURNING id",
    )
    .bind(artist.id)
    .bind(&title)
    .bind(&external_id)
    .bind(&cover)
    .bind(&description)
    .bind(payload.album_type.as_str())
    .bind(release_year)
    .bind(&release_date)
    .bind(payload.score)
    .bind(user.id)
    .bind(if payload.is_public { 1 } else { 0 })
    .bind(&now)
    .bind(&now)
    .fetch_one(&mut *tx)
    .await?;

    set_album_tags(&mut tx, album_id, &tags).await?;
    let artist_ids = if payload.artist_ids.is_empty() {
        vec![artist.id]
    } else {
        payload.artist_ids.clone()
    };
    sync_album_artists(&mut tx, album_id, artist.id, &artist_ids).await?;
    tx.commit().await?;

    let album = crate::repo::fetch_album(&state.db, album_id)
        .await?
        .ok_or_else(|| AppError::not_found("专辑不存在"))?;
    tracing::info!("创建专辑 id={} title={} owner={}", album.id, album.title, user.uuid);
    Ok(ok(album_detail(&state, &album).await?))
}

async fn update_album(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<AlbumUpdate>,
) -> ApiResult<crate::error::ApiOk<AlbumDetail>> {
    let album = crate::repo::fetch_album(&state.db, album_id)
        .await?
        .ok_or_else(|| AppError::not_found("专辑不存在"))?;
    require_album_owner(&user, &album)?;

    let old_cover = album.cover_url.clone();
    let mut title = album.title.clone();
    let mut description = album.description.clone();
    let mut cover = album.cover_url.clone();
    let mut album_type = album.album_type.clone();
    let mut release_year = album.release_year;
    let mut release_date = album.release_date.clone();
    let mut score = album.score;
    let mut is_public = album.is_public;
    let mut tags: Option<Vec<String>> = None;
    let mut cover_changed = false;

    if let Some(value) = payload.title {
        let value = value.ok_or_else(|| AppError::bad_request("专辑标题不能为空"))?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AppError::bad_request("专辑标题不能为空"));
        }
        if trimmed.chars().count() > 255 {
            return Err(AppError::validation("body.title", "长度不能超过 255 个字符"));
        }
        title = trimmed.to_string();
    }
    if let Some(value) = payload.description {
        description = value
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty());
    }
    if let Some(value) = payload.cover_url {
        cover = match value.as_deref() {
            Some(raw) => validate_media_url("body.cover_url", raw)?,
            None => None,
        };
        if cover != old_cover {
            cover_changed = true;
        }
    }
    if let Some(Some(value)) = payload.album_type {
        album_type = value.as_str().to_string();
    }
    if let Some(value) = payload.release_year {
        release_year = value;
    }
    if let Some(value) = payload.release_date {
        match value {
            Some(raw) => {
                let parsed = parse_release_date("body.release_date", &raw)?;
                release_year = parsed.as_deref().and_then(|date| date[..4].parse::<i64>().ok());
                release_date = parsed;
            }
            None => {
                release_date = None;
                if payload.release_year.is_none() {
                    release_year = None;
                }
            }
        }
    }
    if let Some(value) = payload.score {
        if let Some(value) = value {
            if !(0.0..=10.0).contains(&value) {
                return Err(AppError::validation("body.score", "必须在 0-10 之间"));
            }
        }
        score = value;
    }
    if let Some(Some(value)) = payload.is_public {
        is_public = if value { 1 } else { 0 };
    }
    if let Some(value) = payload.tags {
        tags = Some(normalize_tags(value.as_ref()));
    }

    let now = now_text();
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE albums SET title = ?, description = ?, cover_url = ?, album_type = ?,
                           release_year = ?, release_date = ?, score = ?, is_public = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(&title)
    .bind(&description)
    .bind(&cover)
    .bind(&album_type)
    .bind(release_year)
    .bind(&release_date)
    .bind(score)
    .bind(is_public)
    .bind(&now)
    .bind(album.id)
    .execute(&mut *tx)
    .await?;
    if let Some(tags) = &tags {
        set_album_tags(&mut tx, album.id, tags).await?;
    }
    tx.commit().await?;

    if cover_changed {
        delete_media_file(&state.settings, &state.db, old_cover.as_ref(), &user, false).await;
    }

    let album = crate::repo::fetch_album(&state.db, album.id)
        .await?
        .ok_or_else(|| AppError::not_found("专辑不存在"))?;
    Ok(ok(album_detail(&state, &album).await?))
}

async fn delete_album(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let album = crate::repo::fetch_album(&state.db, album_id)
        .await?
        .ok_or_else(|| AppError::not_found("专辑不存在"))?;
    require_album_owner(&user, &album)?;

    let media = album_media_urls(&state.db, &album).await;
    sqlx::query("DELETE FROM albums WHERE id = ?")
        .bind(album.id)
        .execute(&state.db)
        .await?;
    for url in &media {
        delete_media_file(&state.settings, &state.db, Some(url), &user, false).await;
    }
    tracing::info!("删除专辑 id={} by={}", album_id, user.uuid);
    Ok(ok(serde_json::json!({ "deleted": album_id })))
}
