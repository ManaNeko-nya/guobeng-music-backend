//! DTO 装配：批量查询关联数据，避免逐条 SQL。

use std::collections::HashMap;

use sqlx::sqlite::SqlitePool;

use crate::dto::{AlbumBrief, ArtistBrief, TrackOut, TrackWithAlbum};
use crate::error::ApiResult;
use crate::models::{AlbumRow, ArtistRow, TrackRow};
use crate::repo::{ALBUM_SELECT, ARTIST_SELECT, TRACK_SELECT};
use crate::sqlutil::placeholders;
use crate::timeutil::to_iso;

pub async fn owner_uuid(db: &SqlitePool, owner_id: Option<i64>) -> Option<String> {
    let id = owner_id?;
    sqlx::query_scalar::<_, String>("SELECT uuid FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

pub async fn artists_brief(
    db: &SqlitePool,
    ids: &[i64],
) -> ApiResult<HashMap<i64, ArtistBrief>> {
    let mut map = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let mut unique: Vec<i64> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();

    let sql = format!("{ARTIST_SELECT} WHERE ar.id IN ({})", placeholders(unique.len()));
    let mut query = sqlx::query_as::<_, ArtistRow>(&sql);
    for id in &unique {
        query = query.bind(id);
    }
    for row in query.fetch_all(db).await? {
        map.insert(row.id, ArtistBrief::from(&row));
    }
    Ok(map)
}

pub async fn albums_brief(
    db: &SqlitePool,
    ids: &[i64],
) -> ApiResult<HashMap<i64, AlbumBrief>> {
    let mut map = HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let mut unique: Vec<i64> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();

    let sql = format!("{ALBUM_SELECT} WHERE al.id IN ({})", placeholders(unique.len()));
    let mut query = sqlx::query_as::<_, AlbumRow>(&sql);
    for id in &unique {
        query = query.bind(id);
    }
    let albums = query.fetch_all(db).await?;
    if albums.is_empty() {
        return Ok(map);
    }

    let sql = format!(
        "SELECT album_id, artist_id, position FROM album_artists WHERE album_id IN ({}) ORDER BY album_id, position, id",
        placeholders(unique.len())
    );
    let mut query = sqlx::query_as::<_, (i64, i64, i64)>(&sql);
    for id in &unique {
        query = query.bind(id);
    }
    let mut album_artists: HashMap<i64, Vec<i64>> = HashMap::new();
    for (album_id, artist_id, _) in query.fetch_all(db).await? {
        album_artists.entry(album_id).or_default().push(artist_id);
    }

    let sql = format!(
        "SELECT album_id, name FROM album_tags WHERE album_id IN ({}) ORDER BY album_id, id",
        placeholders(unique.len())
    );
    let mut query = sqlx::query_as::<_, (i64, String)>(&sql);
    for id in &unique {
        query = query.bind(id);
    }
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    for (album_id, name) in query.fetch_all(db).await? {
        tags.entry(album_id).or_default().push(name);
    }

    let mut artist_ids: Vec<i64> = albums.iter().map(|album| album.artist_id).collect();
    for list in album_artists.values() {
        artist_ids.extend(list.iter().copied());
    }
    let artists = artists_brief(db, &artist_ids).await?;

    for album in &albums {
        let ordered = album_artists.get(&album.id).cloned().unwrap_or_default();
        let mut list: Vec<ArtistBrief> = ordered
            .iter()
            .filter_map(|id| artists.get(id).cloned())
            .collect();
        if list.is_empty() {
            if let Some(main) = artists.get(&album.artist_id) {
                list.push(main.clone());
            }
        }
        map.insert(
            album.id,
            AlbumBrief {
                id: album.id,
                artist_id: album.artist_id,
                title: album.title.clone(),
                cover_url: album.cover_url.clone(),
                description: album.description.clone(),
                album_type: album.album_type.clone(),
                release_year: album.release_year,
                release_date: album.release_date.clone(),
                score: album.score,
                tags: tags.get(&album.id).cloned().unwrap_or_default(),
                track_count: album.track_count,
                view_count: album.view_count,
                is_public: album.public(),
                created_at: to_iso(&album.created_at),
                updated_at: to_iso(&album.updated_at),
                artist: artists.get(&album.artist_id).cloned(),
                artists: list,
                external_id: album.external_id.clone(),
            },
        );
    }
    Ok(map)
}

async fn build_tracks(
    db: &SqlitePool,
    rows: &[TrackRow],
) -> ApiResult<HashMap<i64, TrackOut>> {
    let mut map = HashMap::new();
    if rows.is_empty() {
        return Ok(map);
    }
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();

    let sql = format!(
        "SELECT track_id, artist_id, position FROM track_artists WHERE track_id IN ({}) ORDER BY track_id, position, id",
        placeholders(ids.len())
    );
    let mut query = sqlx::query_as::<_, (i64, i64, i64)>(&sql);
    for id in &ids {
        query = query.bind(id);
    }
    let mut track_artists: HashMap<i64, Vec<i64>> = HashMap::new();
    for (track_id, artist_id, _) in query.fetch_all(db).await? {
        track_artists.entry(track_id).or_default().push(artist_id);
    }

    let mut artist_ids: Vec<i64> = track_artists.values().flatten().copied().collect();
    artist_ids.sort_unstable();
    artist_ids.dedup();
    let artists = artists_brief(db, &artist_ids).await?;

    for row in rows {
        let list: Vec<ArtistBrief> = track_artists
            .get(&row.id)
            .map(|ids| ids.iter().filter_map(|id| artists.get(id).cloned()).collect())
            .unwrap_or_default();
        map.insert(
            row.id,
            TrackOut {
                id: row.id,
                album_id: row.album_id,
                title: row.title.clone(),
                order_index: row.order_index,
                audio_url: row.audio_url.clone(),
                file_size: row.file_size,
                file_md5: row.file_md5.clone(),
                duration_sec: row.duration_sec,
                has_lyrics: row
                    .lyrics
                    .as_deref()
                    .map(|text| !text.trim().is_empty())
                    .unwrap_or(false),
                comment_count: row.comment_count,
                created_at: to_iso(&row.created_at),
                updated_at: to_iso(&row.updated_at),
                artists: list,
            },
        );
    }
    Ok(map)
}

pub async fn tracks_out(
    db: &SqlitePool,
    ids: &[i64],
) -> ApiResult<HashMap<i64, TrackOut>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut unique: Vec<i64> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    let sql = format!("{TRACK_SELECT} WHERE t.id IN ({})", placeholders(unique.len()));
    let mut query = sqlx::query_as::<_, TrackRow>(&sql);
    for id in &unique {
        query = query.bind(id);
    }
    let rows = query.fetch_all(db).await?;
    build_tracks(db, &rows).await
}

pub async fn tracks_with_album(
    db: &SqlitePool,
    ids: &[i64],
) -> ApiResult<HashMap<i64, TrackWithAlbum>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut unique: Vec<i64> = ids.to_vec();
    unique.sort_unstable();
    unique.dedup();
    let sql = format!("{TRACK_SELECT} WHERE t.id IN ({})", placeholders(unique.len()));
    let mut query = sqlx::query_as::<_, TrackRow>(&sql);
    for id in &unique {
        query = query.bind(id);
    }
    let rows = query.fetch_all(db).await?;
    let outs = build_tracks(db, &rows).await?;
    let album_ids: Vec<i64> = rows.iter().map(|row| row.album_id).collect();
    let albums = albums_brief(db, &album_ids).await?;

    let mut map = HashMap::new();
    for row in &rows {
        if let Some(base) = outs.get(&row.id) {
            map.insert(
                row.id,
                TrackWithAlbum {
                    base: base.clone(),
                    album: albums.get(&row.album_id).cloned(),
                },
            );
        }
    }
    Ok(map)
}
