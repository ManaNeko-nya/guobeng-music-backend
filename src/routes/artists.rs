//! 艺人接口：列表/搜索、详情、参与歌曲、创建与维护。

use axum::{
    extract::State,
    routing::get,
    Router,
};
use serde::Deserialize;

use crate::dto::{ArtistBrief, ArtistDetail, Page, TrackWithAlbum};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiJson, ApiPath, ApiQuery, CurrentUser, OptionalUser, Pagination};
use crate::hydrate;
use crate::media::delete_media_file;
use crate::models::ArtistRow;
use crate::payload::{validate_media_url, ArtistCreate, ArtistUpdate};
use crate::repo::{ARTIST_SELECT, ALBUM_SELECT, TRACK_SELECT};
use crate::services::{
    artist_or_404, like_pattern, require_owner_or_admin, visible_album_clause,
};
use crate::state::AppState;
use crate::timeutil::now_text;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/artists", get(list_artists).post(create_artist))
        .route("/artists/mine", get(my_artists))
        .route("/artists/{artist_id}", get(get_artist).patch(update_artist).delete(delete_artist))
        .route("/artists/{artist_id}/tracks", get(artist_tracks))
}

#[derive(Debug, Deserialize, Default)]
struct ArtistQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    sort: Option<String>,
}

fn briefs(rows: &[ArtistRow]) -> Vec<ArtistBrief> {
    rows.iter().map(ArtistBrief::from).collect()
}

async fn list_artists(
    State(state): State<AppState>,
    pagination: Pagination,
    ApiQuery(query): ApiQuery<ArtistQuery>,
) -> ApiResult<crate::error::ApiOk<Page<ArtistBrief>>> {
    let keyword = query.q.trim();
    let sort = query.sort.as_deref().unwrap_or("name");

    let (filter, pattern) = if keyword.is_empty() {
        (String::new(), None)
    } else {
        (
            "WHERE (ar.name LIKE ? ESCAPE '\\' OR ar.bio LIKE ? ESCAPE '\\')".to_string(),
            Some(like_pattern(keyword)),
        )
    };

    let count_sql = format!("SELECT COUNT(*) FROM artists ar {filter}");
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(pattern) = &pattern {
        count_query = count_query.bind(pattern).bind(pattern);
    }
    let total = count_query.fetch_one(&state.db).await?;

    let order = match sort {
        "name" => "ar.name ASC, ar.id DESC",
        "created_at" => "ar.created_at DESC, ar.id DESC",
        "albums" => "album_count DESC, ar.id DESC",
        _ => "ar.name ASC, ar.id DESC",
    };
    let sql = format!("{ARTIST_SELECT} {filter} ORDER BY {order} LIMIT ? OFFSET ?");
    let mut page_query = sqlx::query_as::<_, ArtistRow>(&sql);
    if let Some(pattern) = &pattern {
        page_query = page_query.bind(pattern).bind(pattern);
    }
    let rows = page_query
        .bind(pagination.page_size)
        .bind(pagination.offset())
        .fetch_all(&state.db)
        .await?;

    Ok(ok(Page::new(
        briefs(&rows),
        total,
        pagination.page,
        pagination.page_size,
    )))
}

async fn my_artists(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    pagination: Pagination,
) -> ApiResult<crate::error::ApiOk<Page<ArtistBrief>>> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artists WHERE owner_id = ?")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    let rows = sqlx::query_as::<_, ArtistRow>(&format!(
        "{ARTIST_SELECT} WHERE ar.owner_id = ? ORDER BY ar.created_at DESC, ar.id DESC LIMIT ? OFFSET ?"
    ))
    .bind(user.id)
    .bind(pagination.page_size)
    .bind(pagination.offset())
    .fetch_all(&state.db)
    .await?;
    Ok(ok(Page::new(
        briefs(&rows),
        total,
        pagination.page,
        pagination.page_size,
    )))
}

async fn artist_albums(
    state: &AppState,
    artist_id: i64,
    user: Option<&crate::models::UserRow>,
) -> ApiResult<Vec<crate::dto::AlbumBrief>> {
    let clause = visible_album_clause("al", user);
    let sql = format!(
        "{ALBUM_SELECT}
         WHERE al.id IN (SELECT album_id FROM album_artists WHERE artist_id = ?)
           AND {}
         ORDER BY al.release_date DESC, al.id DESC",
        clause.sql
    );
    let mut query = sqlx::query_as::<_, crate::models::AlbumRow>(&sql).bind(artist_id);
    for value in clause.binds {
        query = query.bind(value);
    }
    let rows = query.fetch_all(&state.db).await?;
    let albums = hydrate::albums_brief(&state.db, &rows.iter().map(|row| row.id).collect::<Vec<_>>()).await?;
    Ok(rows.iter().filter_map(|row| albums.get(&row.id).cloned()).collect())
}

async fn get_artist(
    State(state): State<AppState>,
    ApiPath(artist_id): ApiPath<i64>,
    OptionalUser(user): OptionalUser,
) -> ApiResult<crate::error::ApiOk<ArtistDetail>> {
    let artist = artist_or_404(&state.db, artist_id).await?;
    let albums = artist_albums(&state, artist.id, user.as_ref()).await?;
    Ok(ok(ArtistDetail {
        base: ArtistBrief::from(&artist),
        owner_id: hydrate::owner_uuid(&state.db, artist.owner_id).await,
        albums,
    }))
}

async fn artist_tracks(
    State(state): State<AppState>,
    ApiPath(artist_id): ApiPath<i64>,
    OptionalUser(user): OptionalUser,
    pagination: Pagination,
    ApiQuery(query): ApiQuery<TrackQuery>,
) -> ApiResult<crate::error::ApiOk<Page<TrackWithAlbum>>> {
    artist_or_404(&state.db, artist_id).await?;
    let clause = visible_album_clause("al", user.as_ref());

    let filter = format!(
        "WHERE {} AND t.id IN (SELECT track_id FROM track_artists WHERE artist_id = ?)",
        clause.sql
    );
    let count_sql = format!("SELECT COUNT(*) FROM tracks t JOIN albums al ON al.id = t.album_id {filter}");
    let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);
    for value in &clause.binds {
        count_query = count_query.bind(value);
    }
    let total = count_query.bind(artist_id).fetch_one(&state.db).await?;

    let order = if query.sort.as_deref() == Some("title") {
        "t.title ASC, t.id ASC"
    } else {
        "al.release_date DESC, t.order_index ASC, t.id ASC"
    };
    let sql = format!(
        "{TRACK_SELECT} JOIN albums al ON al.id = t.album_id {filter} ORDER BY {order} LIMIT ? OFFSET ?"
    );
    let mut page_query = sqlx::query_as::<_, crate::models::TrackRow>(&sql);
    for value in &clause.binds {
        page_query = page_query.bind(value);
    }
    let rows = page_query
        .bind(artist_id)
        .bind(pagination.page_size)
        .bind(pagination.offset())
        .fetch_all(&state.db)
        .await?;

    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let tracks = hydrate::tracks_with_album(&state.db, &ids).await?;
    let items = ids.iter().filter_map(|id| tracks.get(id).cloned()).collect();
    Ok(ok(Page::new(items, total, pagination.page, pagination.page_size)))
}

#[derive(Debug, Deserialize, Default)]
struct TrackQuery {
    #[serde(default)]
    sort: Option<String>,
}

async fn create_artist(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<ArtistCreate>,
) -> ApiResult<crate::error::ApiOk<ArtistDetail>> {
    crate::extract::require_admin_only_create(&state, &user, "艺人")?;

    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::bad_request("艺人名称不能为空"));
    }
    if name.chars().count() > 255 {
        return Err(AppError::validation("body.name", "长度不能超过 255 个字符"));
    }
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM artists WHERE name = ?")
        .bind(&name)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_some() {
        return Err(AppError::conflict("已存在同名艺人"));
    }

    let cover = match payload.cover_url.as_deref() {
        Some(value) => validate_media_url("body.cover_url", value)?,
        None => None,
    };
    let bio = payload
        .bio
        .as_deref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let now = now_text();

    let artist_id: i64 = sqlx::query_scalar(
        "INSERT INTO artists (name, cover_url, bio, owner_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&name)
    .bind(&cover)
    .bind(&bio)
    .bind(user.id)
    .bind(&now)
    .bind(&now)
    .fetch_one(&state.db)
    .await?;

    let artist = artist_or_404(&state.db, artist_id).await?;
    tracing::info!("创建艺人 id={} name={} owner={}", artist.id, artist.name, user.uuid);
    Ok(ok(ArtistDetail {
        base: ArtistBrief::from(&artist),
        owner_id: Some(user.uuid.clone()),
        albums: Vec::new(),
    }))
}

async fn update_artist(
    State(state): State<AppState>,
    ApiPath(artist_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<ArtistUpdate>,
) -> ApiResult<crate::error::ApiOk<ArtistDetail>> {
    let artist = artist_or_404(&state.db, artist_id).await?;
    require_owner_or_admin(&user, artist.owner_id)?;
    let old_cover = artist.cover_url.clone();
    let mut cover_changed = false;

    let mut name = artist.name.clone();
    let mut cover = artist.cover_url.clone();
    let mut bio = artist.bio.clone();

    if let Some(value) = payload.name {
        let value = value.ok_or_else(|| AppError::bad_request("艺人名称不能为空"))?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(AppError::bad_request("艺人名称不能为空"));
        }
        if trimmed.chars().count() > 255 {
            return Err(AppError::validation("body.name", "长度不能超过 255 个字符"));
        }
        let clash: Option<i64> =
            sqlx::query_scalar("SELECT id FROM artists WHERE name = ? AND id != ?")
                .bind(trimmed)
                .bind(artist.id)
                .fetch_optional(&state.db)
                .await?;
        if clash.is_some() {
            return Err(AppError::conflict("已存在同名艺人"));
        }
        name = trimmed.chars().take(255).collect();
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
    if let Some(value) = payload.bio {
        bio = value
            .map(|raw| raw.trim().to_string())
            .filter(|value| !value.is_empty());
    }

    sqlx::query(
        "UPDATE artists SET name = ?, cover_url = ?, bio = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&name)
    .bind(&cover)
    .bind(&bio)
    .bind(now_text())
    .bind(artist.id)
    .execute(&state.db)
    .await?;

    if cover_changed {
        delete_media_file(&state.settings, &state.db, old_cover.as_ref(), &user, false).await;
    }

    let artist = artist_or_404(&state.db, artist_id).await?;
    let albums = artist_albums(&state, artist.id, Some(&user)).await?;
    Ok(ok(ArtistDetail {
        base: ArtistBrief::from(&artist),
        owner_id: hydrate::owner_uuid(&state.db, artist.owner_id).await,
        albums,
    }))
}

async fn delete_artist(
    State(state): State<AppState>,
    ApiPath(artist_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let artist = artist_or_404(&state.db, artist_id).await?;
    require_owner_or_admin(&user, artist.owner_id)?;

    let mut media: Vec<String> = artist.cover_url.clone().into_iter().collect();
    let album_rows = sqlx::query_as::<_, crate::models::AlbumRow>(&format!(
        "{ALBUM_SELECT} WHERE al.artist_id = ?"
    ))
    .bind(artist.id)
    .fetch_all(&state.db)
    .await?;
    for album in &album_rows {
        if let Some(cover) = &album.cover_url {
            media.push(cover.clone());
        }
        let audios: Vec<Option<String>> =
            sqlx::query_scalar("SELECT audio_url FROM tracks WHERE album_id = ?")
                .bind(album.id)
                .fetch_all(&state.db)
                .await?;
        media.extend(audios.into_iter().flatten());
    }

    sqlx::query("DELETE FROM artists WHERE id = ?")
        .bind(artist.id)
        .execute(&state.db)
        .await?;
    for url in &media {
        delete_media_file(&state.settings, &state.db, Some(url), &user, false).await;
    }
    tracing::info!("删除艺人 id={} by={}", artist_id, user.uuid);
    Ok(ok(serde_json::json!({ "deleted": artist_id })))
}
