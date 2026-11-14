//! 歌曲接口：专辑下的增删改序、全局搜索、详情、换源、LRC 歌词。

use std::path::Path;

use axum::{
    extract::{DefaultBodyLimit, State},
    routing::{get, post},
    Router,
};
use serde::Deserialize;

use crate::dto::{lyrics_out, LyricsOut, Page, TrackOut, TrackWithAlbum};
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{
    ApiJson, ApiMultipart, ApiPath, ApiQuery, CurrentUser, OptionalUser, Pagination,
};
use crate::hydrate;
use crate::lyrics::normalize_lrc;
use crate::media::{delete_media_file, register_upload, save_field, SavedUpload};
use crate::models::{TrackRow, UserRow};
use crate::payload::{
    validate_media_url, LyricsUpdate, TrackCreate, TrackReorderRequest, TrackUpdate,
};
use crate::repo::{list_tracks, renumber_tracks, TRACK_SELECT};
use crate::services::{
    create_track, dedup_ids, like_pattern, reorder_tracks, require_album_owner,
    require_audio_upload_permission, require_track_manager, sync_track_artists, visible_album,
    visible_album_clause, visible_track, visible_track_clause, NewTrack,
};
use crate::sqlutil::{bind_all, Param};
use crate::state::AppState;
use crate::timeutil::now_text;

pub fn album_tracks_router() -> Router<AppState> {
    Router::new()
        .route("/albums/{album_id}/tracks", get(list_album_tracks).post(add_track))
        .route(
            "/albums/{album_id}/tracks/upload",
            post(upload_track)
                .layer(DefaultBodyLimit::max(crate::settings::MAX_AUDIO_SIZE as usize + 64 * 1024 * 1024)),
        )
        .route("/albums/{album_id}/tracks/reorder", post(reorder_album_tracks))
}

pub fn tracks_router() -> Router<AppState> {
    let audio_limit = crate::settings::MAX_AUDIO_SIZE as usize + 64 * 1024 * 1024;
    Router::new()
        .route("/tracks", get(list_tracks_all))
        .route("/tracks/{track_id}", get(get_track).patch(update_track).delete(delete_track))
        .route(
            "/tracks/{track_id}/audio",
            post(replace_track_audio).layer(DefaultBodyLimit::max(audio_limit)),
        )
        .route(
            "/tracks/{track_id}/lyrics",
            get(get_lyrics).put(put_lyrics).delete(delete_lyrics),
        )
}

async fn track_out(state: &AppState, track_id: i64) -> ApiResult<TrackOut> {
    hydrate::tracks_out(&state.db, &[track_id])
        .await?
        .remove(&track_id)
        .ok_or_else(|| AppError::not_found("歌曲不存在"))
}

async fn track_with_album(state: &AppState, track_id: i64) -> ApiResult<TrackWithAlbum> {
    hydrate::tracks_with_album(&state.db, &[track_id])
        .await?
        .remove(&track_id)
        .ok_or_else(|| AppError::not_found("歌曲不存在"))
}

// ------------------------- 专辑下的歌曲集合 -------------------------

async fn list_album_tracks(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    OptionalUser(user): OptionalUser,
) -> ApiResult<crate::error::ApiOk<Vec<TrackOut>>> {
    let album = visible_album(&state.db, album_id, user.as_ref()).await?;
    let rows = list_tracks(&state.db, album.id).await?;
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let map = hydrate::tracks_out(&state.db, &ids).await?;
    let items = ids.iter().filter_map(|id| map.get(id).cloned()).collect();
    Ok(ok(items))
}

async fn add_track(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<TrackCreate>,
) -> ApiResult<crate::error::ApiOk<TrackOut>> {
    let album = visible_album(&state.db, album_id, Some(&user)).await?;
    require_album_owner(&user, &album)?;

    if payload.artist_ids.is_empty() {
        return Err(AppError::validation("body.artist_ids", "至少需要一位参与艺人"));
    }
    if payload.title.chars().count() > 255 {
        return Err(AppError::validation("body.title", "长度不能超过 255 个字符"));
    }
    if let Some(duration) = payload.duration_sec {
        if duration < 0.0 {
            return Err(AppError::validation("body.duration_sec", "必须大于等于 0"));
        }
    }
    if let Some(order) = payload.order_index {
        if order < 0 {
            return Err(AppError::validation("body.order_index", "必须大于等于 0"));
        }
    }
    let audio_url = match payload.audio_url.as_deref() {
        Some(value) => validate_media_url("body.audio_url", value)?,
        None => None,
    };

    let mut tx = state.db.begin().await?;
    let track_id = create_track(
        &mut tx,
        &album,
        NewTrack {
            title: payload.title.clone(),
            audio_url,
            file_size: None,
            file_md5: None,
            duration_sec: payload.duration_sec,
            order_index: payload.order_index,
            artist_ids: payload.artist_ids.clone(),
        },
    )
    .await?;
    tx.commit().await?;

    tracing::info!("新增歌曲 track={} album={} by={}", track_id, album_id, user.uuid);
    Ok(ok(track_out(&state, track_id).await?))
}

fn parse_form_int(value: &str) -> ApiResult<i64> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| AppError::validation("body.artist_ids", "必须是整数"))
}

/// 表单里的时长字段：空串视为不传；非法值给出 422 字段错误。
fn parse_form_duration(value: &str) -> ApiResult<Option<f64>> {
    let text = value.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let secs = text
        .parse::<f64>()
        .map_err(|_| AppError::validation("body.duration_sec", "必须是数字"))?;
    if !secs.is_finite() || secs < 0.0 {
        return Err(AppError::validation("body.duration_sec", "必须大于等于 0"));
    }
    Ok(Some(secs))
}

fn parse_form_artist_ids(value: &str) -> ApiResult<Vec<i64>> {
    let mut ids = Vec::new();
    for part in value.split(|c: char| c == ',' || c.is_whitespace()) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        ids.push(parse_form_int(part)?);
    }
    Ok(ids)
}

async fn upload_track(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiMultipart(mut multipart): ApiMultipart,
) -> ApiResult<crate::error::ApiOk<TrackOut>> {
    let album = visible_album(&state.db, album_id, Some(&user)).await?;
    require_album_owner(&user, &album)?;
    require_audio_upload_permission(&state, &user)?;

    let mut saved: Option<SavedUpload> = None;
    let mut title: Option<String> = None;
    let mut duration: Option<f64> = None;
    let mut order_index: Option<i64> = None;
    let mut artist_ids: Vec<i64> = Vec::new();

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::bad_request(format!("上传表单解析失败：{err}")))?
    {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                if saved.is_none() {
                    saved = Some(save_field(&mut field, "audio", &state.settings).await?);
                }
            }
            "title" => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| AppError::bad_request(format!("title 读取失败：{err}")))?;
                if !value.trim().is_empty() {
                    title = Some(value);
                }
            }
            "duration_sec" => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| AppError::bad_request(format!("duration_sec 读取失败：{err}")))?;
                if let Some(secs) = parse_form_duration(&value)? {
                    duration = Some(secs);
                }
            }
            "order_index" => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| AppError::bad_request(format!("order_index 读取失败：{err}")))?;
                if !value.trim().is_empty() {
                    order_index = Some(value.trim().parse::<i64>().map_err(|_| {
                        AppError::validation("body.order_index", "必须是整数")
                    })?);
                }
            }
            "artist_ids" => {
                let value = field
                    .text()
                    .await
                    .map_err(|err| AppError::bad_request(format!("artist_ids 读取失败：{err}")))?;
                artist_ids.extend(parse_form_artist_ids(&value)?);
            }
            _ => {}
        }
    }

    let Some(upload) = saved else {
        return Err(AppError::bad_request("缺少上传文件"));
    };
    if artist_ids.is_empty() {
        delete_media_file(&state.settings, &state.db, Some(&upload.url), &user, true).await;
        return Err(AppError::bad_request("必须指定至少一位参与艺人"));
    }

    // 只在可见范围内查重，避免泄露他人私有专辑里的曲目信息
    let clause = visible_track_clause("t", Some(&user));
    let existing = sqlx::query_as::<_, (i64, String)>(&format!(
        "SELECT t.id, t.title FROM tracks t WHERE t.file_md5 = ? AND {} LIMIT 1",
        clause.sql
    ))
    .bind(&upload.md5)
    .bind(clause.binds[0])
    .bind(clause.binds[1])
    .fetch_optional(&state.db)
    .await?;
    if let Some((id, existing_title)) = existing {
        delete_media_file(&state.settings, &state.db, Some(&upload.url), &user, true).await;
        return Err(AppError::conflict(format!(
            "相同音频已存在：{existing_title}（歌曲 ID {id}）"
        )));
    }

    let default_title = Path::new(&upload.original)
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("未命名歌曲")
        .to_string();
    let track_title = title.unwrap_or(default_title);

    let mut tx = state.db.begin().await?;
    let result = create_track(
        &mut tx,
        &album,
        NewTrack {
            title: track_title,
            audio_url: Some(upload.url.clone()),
            file_size: Some(upload.size as i64),
            file_md5: Some(upload.md5.clone()),
            duration_sec: duration,
            order_index,
            artist_ids,
        },
    )
    .await;

    let track_id = match result {
        Ok(id) => id,
        Err(err) => {
            drop(tx);
            delete_media_file(&state.settings, &state.db, Some(&upload.url), &user, true).await;
            return Err(err);
        }
    };

    if let Err(err) = register_upload(&mut *tx, &upload, &user).await {
        drop(tx);
        delete_media_file(&state.settings, &state.db, Some(&upload.url), &user, true).await;
        return Err(AppError::from(err));
    }
    tx.commit().await?;

    tracing::info!(
        "上传音频成功 track={} album={} size={}",
        track_id,
        album_id,
        upload.size
    );
    Ok(ok(track_out(&state, track_id).await?))
}

/// 读取换源表单；中途失败时删掉本次刚落盘的文件。
async fn parse_audio_form(
    state: &AppState,
    user: &UserRow,
    mut multipart: axum::extract::Multipart,
) -> ApiResult<(SavedUpload, Option<String>)> {
    let mut saved: Option<SavedUpload> = None;
    let mut duration_raw: Option<String> = None;

    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(err) => {
                if let Some(upload) = saved.as_ref() {
                    delete_media_file(&state.settings, &state.db, Some(&upload.url), user, true).await;
                }
                return Err(AppError::bad_request(format!("上传表单解析失败：{err}")));
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" if saved.is_none() => {
                saved = Some(save_field(&mut field, "audio", &state.settings).await?);
            }
            "duration_sec" => {
                match field.text().await {
                    Ok(value) => duration_raw = Some(value),
                    Err(err) => {
                        if let Some(upload) = saved.as_ref() {
                            delete_media_file(
                                &state.settings,
                                &state.db,
                                Some(&upload.url),
                                user,
                                true,
                            )
                            .await;
                        }
                        return Err(AppError::bad_request(format!("duration_sec 读取失败：{err}")));
                    }
                }
            }
            _ => {}
        }
    }
    let upload = saved.ok_or_else(|| AppError::bad_request("缺少上传文件"))?;
    Ok((upload, duration_raw))
}

/// 事务内换掉音频字段并登记新文件；提交成功后清理不再被引用的旧文件。
async fn apply_audio_replacement(
    state: &AppState,
    user: &UserRow,
    track: &TrackRow,
    upload: &SavedUpload,
    duration: Option<f64>,
) -> ApiResult<()> {
    // 只在可见范围内查重，本曲自身不算冲突（允许重传同一文件）。
    let clause = visible_track_clause("t", Some(user));
    let existing = sqlx::query_as::<_, (i64, String)>(&format!(
        "SELECT t.id, t.title FROM tracks t WHERE t.file_md5 = ? AND t.id <> ? AND {} LIMIT 1",
        clause.sql
    ))
    .bind(&upload.md5)
    .bind(track.id)
    .bind(clause.binds[0])
    .bind(clause.binds[1])
    .fetch_optional(&state.db)
    .await?;
    if let Some((id, title)) = existing {
        return Err(AppError::conflict(format!(
            "相同音频已存在：{title}（歌曲 ID {id}）"
        )));
    }

    let old_url = track.audio_url.clone();
    let mut tx = state.db.begin().await?;
    let write = sqlx::query(
        "UPDATE tracks SET audio_url = ?, file_size = ?, file_md5 = ?, duration_sec = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&upload.url)
    .bind(upload.size as i64)
    .bind(&upload.md5)
    .bind(duration)
    .bind(now_text())
    .bind(track.id)
    .execute(&mut *tx)
    .await;
    if let Err(err) = write {
        drop(tx);
        return Err(AppError::from(err));
    }
    if let Err(err) = register_upload(&mut *tx, upload, user).await {
        drop(tx);
        return Err(AppError::from(err));
    }
    tx.commit().await?;

    // 外链与仍被其它歌曲引用的文件不动；媒体归属校验由 delete_media_file 负责。
    if let Some(url) = old_url.as_ref() {
        let used = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks WHERE audio_url = ?")
            .bind(url)
            .fetch_one(&state.db)
            .await?;
        if used == 0 {
            delete_media_file(&state.settings, &state.db, Some(url), user, false).await;
        }
    }
    Ok(())
}

/// 更换音源：歌曲 ID、歌词、评论、收藏与历史保持不变。
async fn replace_track_audio(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiMultipart(multipart): ApiMultipart,
) -> ApiResult<crate::error::ApiOk<TrackWithAlbum>> {
    let (track, album) = visible_track(&state.db, track_id, Some(&user)).await?;
    require_track_manager(&user, &album)?;
    require_audio_upload_permission(&state, &user)?;

    let (upload, duration_raw) = parse_audio_form(&state, &user, multipart).await?;
    let outcome = match parse_form_duration(duration_raw.as_deref().unwrap_or_default()) {
        Ok(duration) => apply_audio_replacement(&state, &user, &track, &upload, duration).await,
        Err(err) => Err(err),
    };
    if let Err(err) = outcome {
        delete_media_file(&state.settings, &state.db, Some(&upload.url), &user, true).await;
        return Err(err);
    }

    tracing::info!(
        "更换音源成功 track={} size={} by={}",
        track_id,
        upload.size,
        user.uuid
    );
    Ok(ok(track_with_album(&state, track.id).await?))
}

async fn reorder_album_tracks(
    State(state): State<AppState>,
    ApiPath(album_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<TrackReorderRequest>,
) -> ApiResult<crate::error::ApiOk<Vec<TrackOut>>> {
    let album = visible_album(&state.db, album_id, Some(&user)).await?;
    require_album_owner(&user, &album)?;

    let mut tx = state.db.begin().await?;
    reorder_tracks(&mut tx, album.id, &payload.track_ids).await?;
    tx.commit().await?;

    let rows = list_tracks(&state.db, album.id).await?;
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let map = hydrate::tracks_out(&state.db, &ids).await?;
    Ok(ok(ids.iter().filter_map(|id| map.get(id).cloned()).collect()))
}

// ---------------------------- 单首歌曲 ----------------------------

#[derive(Debug, Deserialize, Default)]
struct TrackListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    album_id: Option<i64>,
    #[serde(default)]
    artist_id: Option<i64>,
    #[serde(default)]
    md5: Option<String>,
    #[serde(default)]
    sort: Option<String>,
}

async fn list_tracks_all(
    State(state): State<AppState>,
    pagination: Pagination,
    OptionalUser(user): OptionalUser,
    ApiQuery(query): ApiQuery<TrackListQuery>,
) -> ApiResult<crate::error::ApiOk<Page<TrackWithAlbum>>> {
    let clause = visible_album_clause("al", user.as_ref());
    let mut sql = format!("WHERE {}", clause.sql);
    let mut params: Vec<Param> = clause.binds.iter().copied().map(Param::Int).collect();

    let keyword = query.q.trim();
    if !keyword.is_empty() {
        let pattern = like_pattern(keyword);
        sql.push_str(
            " AND (t.title LIKE ? ESCAPE '\\' OR al.title LIKE ? ESCAPE '\\' \
             OR EXISTS (SELECT 1 FROM artists a WHERE a.id = al.artist_id AND a.name LIKE ? ESCAPE '\\') \
             OR EXISTS (SELECT 1 FROM album_tags at WHERE at.album_id = al.id AND at.name LIKE ? ESCAPE '\\') \
             OR EXISTS (SELECT 1 FROM track_artists ta JOIN artists a2 ON a2.id = ta.artist_id \
                        WHERE ta.track_id = t.id AND a2.name LIKE ? ESCAPE '\\'))",
        );
        for _ in 0..5 {
            params.push(Param::Text(pattern.clone()));
        }
    }
    if let Some(album_id) = query.album_id {
        sql.push_str(" AND t.album_id = ?");
        params.push(Param::Int(album_id));
    }
    if let Some(artist_id) = query.artist_id {
        sql.push_str(" AND t.id IN (SELECT track_id FROM track_artists WHERE artist_id = ?)");
        params.push(Param::Int(artist_id));
    }
    let fingerprint = query
        .md5
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if !fingerprint.is_empty() {
        sql.push_str(" AND t.file_md5 = ?");
        params.push(Param::Text(fingerprint));
    }

    let count_sql =
        format!("SELECT COUNT(*) FROM tracks t JOIN albums al ON al.id = t.album_id {sql}");
    let total = bind_all(sqlx::query_scalar::<_, i64>(&count_sql), &params)
        .fetch_one(&state.db)
        .await?;

    let order = match query.sort.as_deref() {
        Some("title") => "t.title ASC, t.id ASC",
        Some("release") => "al.release_date DESC, t.id DESC",
        _ => "t.created_at DESC, t.id DESC",
    };
    let page_sql =
        format!("{TRACK_SELECT} JOIN albums al ON al.id = t.album_id {sql} ORDER BY {order} LIMIT ? OFFSET ?");
    let mut page_params = params;
    page_params.push(Param::Int(pagination.page_size));
    page_params.push(Param::Int(pagination.offset()));
    let rows = bind_all(sqlx::query_as::<_, TrackRow>(&page_sql), &page_params)
        .fetch_all(&state.db)
        .await?;

    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let map = hydrate::tracks_with_album(&state.db, &ids).await?;
    let items = ids.iter().filter_map(|id| map.get(id).cloned()).collect();
    Ok(ok(Page::new(items, total, pagination.page, pagination.page_size)))
}

async fn get_track(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    OptionalUser(user): OptionalUser,
) -> ApiResult<crate::error::ApiOk<TrackWithAlbum>> {
    let (track, _) = visible_track(&state.db, track_id, user.as_ref()).await?;
    Ok(ok(track_with_album(&state, track.id).await?))
}

/// 把某首歌放到专辑内指定位置，并把整张专辑的顺序压实为 0..n-1。
/// 先把其余歌曲排好再插入目标歌，避免逐条更新时踩到中间顺序。
async fn place_track(
    exec: &mut sqlx::SqliteConnection,
    album_id: i64,
    track_id: i64,
    position: Option<i64>,
) -> ApiResult<()> {
    let mut ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tracks WHERE album_id = ? AND id <> ? ORDER BY order_index, id",
    )
    .bind(album_id)
    .bind(track_id)
    .fetch_all(&mut *exec)
    .await?;
    let index = match position {
        Some(value) => (value.max(0) as usize).min(ids.len()),
        None => ids.len(),
    };
    ids.insert(index, track_id);
    for (order, id) in ids.iter().enumerate() {
        sqlx::query("UPDATE tracks SET order_index = ? WHERE id = ?")
            .bind(order as i64)
            .bind(id)
            .execute(&mut *exec)
            .await?;
    }
    Ok(())
}

async fn update_track(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<TrackUpdate>,
) -> ApiResult<crate::error::ApiOk<TrackWithAlbum>> {
    let (track, album) = visible_track(&state.db, track_id, Some(&user)).await?;
    require_track_manager(&user, &album)?;

    // 先把所有字段校验完，再开事务，避免中途失败留下半成品。
    let title = match payload.title.as_deref() {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err(AppError::bad_request("歌曲标题不能为空"));
            }
            if trimmed.chars().count() > 255 {
                return Err(AppError::validation("body.title", "长度不能超过 255 个字符"));
            }
            Some(trimmed.to_string())
        }
        None => None,
    };

    let artist_ids = match payload.artist_ids {
        Some(list) => {
            let ordered = dedup_ids(&list);
            if ordered.is_empty() {
                return Err(AppError::validation("body.artist_ids", "至少需要一位参与艺人"));
            }
            Some(ordered)
        }
        None => None,
    };

    if let Some(order) = payload.order_index {
        if order < 0 {
            return Err(AppError::validation("body.order_index", "必须大于等于 0"));
        }
    }

    let duration = match payload.duration_sec {
        None => None,
        Some(value) => {
            if let Some(secs) = value {
                if !secs.is_finite() || secs < 0.0 {
                    return Err(AppError::validation("body.duration_sec", "必须大于等于 0"));
                }
            }
            Some(value)
        }
    };

    // 换专辑需要同时是目标专辑的所有者，否则拒绝，避免把歌塞进别人的专辑。
    let target_album = match payload.album_id {
        Some(id) if id != album.id => {
            let target = visible_album(&state.db, id, Some(&user)).await?;
            require_album_owner(&user, &target)?;
            Some(target)
        }
        _ => None,
    };
    let album_changed = target_album.is_some();
    let target_id = target_album.as_ref().map(|item| item.id).unwrap_or(album.id);
    let main_artist_id = target_album
        .as_ref()
        .map(|item| item.artist_id)
        .unwrap_or(album.artist_id);
    let now = now_text();

    let mut tx = state.db.begin().await?;
    if let Some(value) = &title {
        sqlx::query("UPDATE tracks SET title = ?, updated_at = ? WHERE id = ?")
            .bind(value)
            .bind(&now)
            .bind(track.id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(value) = &duration {
        sqlx::query("UPDATE tracks SET duration_sec = ?, updated_at = ? WHERE id = ?")
            .bind(*value)
            .bind(&now)
            .bind(track.id)
            .execute(&mut *tx)
            .await?;
    }
    if album_changed {
        sqlx::query("UPDATE tracks SET album_id = ?, updated_at = ? WHERE id = ?")
            .bind(target_id)
            .bind(&now)
            .bind(track.id)
            .execute(&mut *tx)
            .await?;
        // 原专辑移走一首后立即压实，避免留空洞。
        renumber_tracks(&mut tx, album.id).await?;
    }
    if album_changed || payload.order_index.is_some() {
        place_track(&mut tx, target_id, track.id, payload.order_index).await?;
    }

    // 显式改艺人 → 用新列表；只换专辑 → 把原艺人并入新专辑的艺人集合。
    let sync_ids = match artist_ids {
        Some(list) => Some(list),
        None if album_changed => {
            let existing: Vec<i64> = sqlx::query_scalar(
                "SELECT artist_id FROM track_artists WHERE track_id = ? ORDER BY position, id",
            )
            .bind(track.id)
            .fetch_all(&mut *tx)
            .await?;
            if existing.is_empty() {
                None
            } else {
                Some(existing)
            }
        }
        None => None,
    };
    if let Some(list) = sync_ids {
        sync_track_artists(&mut tx, track.id, target_id, main_artist_id, &list).await?;
    }
    tx.commit().await?;

    tracing::info!(
        "修改歌曲 track={} album={} by={}",
        track_id,
        target_id,
        user.uuid
    );
    Ok(ok(track_with_album(&state, track.id).await?))
}

async fn delete_track(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let (track, album) = visible_track(&state.db, track_id, Some(&user)).await?;
    require_track_manager(&user, &album)?;
    let audio_url = track.audio_url.clone();

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM tracks WHERE id = ?")
        .bind(track.id)
        .execute(&mut *tx)
        .await?;
    renumber_tracks(&mut tx, album.id).await?;
    tx.commit().await?;

    delete_media_file(&state.settings, &state.db, audio_url.as_ref(), &user, false).await;
    tracing::info!("删除歌曲 track={} by={}", track_id, user.uuid);
    Ok(ok(serde_json::json!({ "deleted": track_id })))
}

// ------------------------------ 歌词 ------------------------------

async fn get_lyrics(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    OptionalUser(user): OptionalUser,
) -> ApiResult<crate::error::ApiOk<LyricsOut>> {
    let (track, _) = visible_track(&state.db, track_id, user.as_ref()).await?;
    Ok(ok(lyrics_out(track.id, &track.lyrics, &track.updated_at)))
}

async fn put_lyrics(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
    ApiJson(payload): ApiJson<LyricsUpdate>,
) -> ApiResult<crate::error::ApiOk<LyricsOut>> {
    let (track, album) = visible_track(&state.db, track_id, Some(&user)).await?;
    require_track_manager(&user, &album)?;
    let lyrics = normalize_lrc(&payload.lyrics)?;
    let now = now_text();
    sqlx::query("UPDATE tracks SET lyrics = ?, updated_at = ? WHERE id = ?")
        .bind(&lyrics)
        .bind(&now)
        .bind(track.id)
        .execute(&state.db)
        .await?;
    Ok(ok(lyrics_out(track.id, &Some(lyrics), &now)))
}

async fn delete_lyrics(
    State(state): State<AppState>,
    ApiPath(track_id): ApiPath<i64>,
    CurrentUser(user): CurrentUser,
) -> ApiResult<crate::error::ApiOk<serde_json::Value>> {
    let (track, album) = visible_track(&state.db, track_id, Some(&user)).await?;
    require_track_manager(&user, &album)?;
    sqlx::query("UPDATE tracks SET lyrics = NULL, updated_at = ? WHERE id = ?")
        .bind(now_text())
        .bind(track.id)
        .execute(&state.db)
        .await?;
    Ok(ok(serde_json::json!({ "deleted": true })))
}
