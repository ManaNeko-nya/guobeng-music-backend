//! 数据库读取：统一的 SELECT 片段与常用查询（计数用关联子查询，避免 N+1）。

use sqlx::sqlite::SqlitePool;

use crate::models::{AlbumRow, ArtistRow, CommentRow, TrackRow};

pub const ARTIST_SELECT: &str = "\
SELECT ar.id, ar.name, ar.cover_url, ar.bio, ar.owner_id, ar.created_at, ar.updated_at,
       (SELECT COUNT(DISTINCT aa.album_id) FROM album_artists aa WHERE aa.artist_id = ar.id) AS album_count,
       (SELECT COUNT(DISTINCT ta.track_id) FROM track_artists ta WHERE ta.artist_id = ar.id) AS track_count
FROM artists ar";

pub const ALBUM_SELECT: &str = "\
SELECT al.id, al.artist_id, al.title, al.external_id, al.cover_url, al.description, al.album_type,
       al.release_year, al.release_date, al.score, al.owner_id, al.view_count, al.is_public,
       al.created_at, al.updated_at,
       (SELECT COUNT(*) FROM tracks t WHERE t.album_id = al.id) AS track_count
FROM albums al";

pub const TRACK_SELECT: &str = "\
SELECT t.id, t.album_id, t.title, t.order_index, t.audio_url, t.file_size, t.file_md5,
       t.duration_sec, t.lyrics, t.created_at, t.updated_at,
       (SELECT COUNT(*) FROM comments c WHERE c.track_id = t.id) AS comment_count
FROM tracks t";

pub const COMMENT_SELECT: &str = "\
SELECT c.id, c.track_id, c.user_id, c.parent_id, c.content, c.author_name, c.author_avatar,
       c.source, c.created_at,
       u.uuid AS user_uuid, u.nickname AS user_nickname, u.username AS user_username,
       u.avatar_url AS user_avatar
FROM comments c LEFT JOIN users u ON u.id = c.user_id";

pub async fn fetch_artist(db: &SqlitePool, id: i64) -> Result<Option<ArtistRow>, sqlx::Error> {
    sqlx::query_as::<_, ArtistRow>(&format!("{ARTIST_SELECT} WHERE ar.id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn fetch_album(db: &SqlitePool, id: i64) -> Result<Option<AlbumRow>, sqlx::Error> {
    sqlx::query_as::<_, AlbumRow>(&format!("{ALBUM_SELECT} WHERE al.id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn fetch_track(db: &SqlitePool, id: i64) -> Result<Option<TrackRow>, sqlx::Error> {
    sqlx::query_as::<_, TrackRow>(&format!("{TRACK_SELECT} WHERE t.id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn fetch_comment(db: &SqlitePool, id: i64) -> Result<Option<CommentRow>, sqlx::Error> {
    sqlx::query_as::<_, CommentRow>(&format!("{COMMENT_SELECT} WHERE c.id = ?"))
        .bind(id)
        .fetch_optional(db)
        .await
}

pub async fn list_tracks(db: &SqlitePool, album_id: i64) -> Result<Vec<TrackRow>, sqlx::Error> {
    sqlx::query_as::<_, TrackRow>(&format!(
        "{TRACK_SELECT} WHERE t.album_id = ? ORDER BY t.order_index, t.id"
    ))
    .bind(album_id)
    .fetch_all(db)
    .await
}

pub async fn next_track_order(
    exec: &mut sqlx::SqliteConnection,
    album_id: i64,
) -> Result<i64, sqlx::Error> {
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(order_index) FROM tracks WHERE album_id = ?",
    )
    .bind(album_id)
    .fetch_one(&mut *exec)
    .await?;
    Ok(current.unwrap_or(-1) + 1)
}

/// 把某专辑的歌曲顺序压实为 0..n-1。
pub async fn renumber_tracks(
    exec: &mut sqlx::SqliteConnection,
    album_id: i64,
) -> Result<(), sqlx::Error> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tracks WHERE album_id = ? ORDER BY order_index, id",
    )
    .bind(album_id)
    .fetch_all(&mut *exec)
    .await?;
    for (index, id) in ids.iter().enumerate() {
        sqlx::query("UPDATE tracks SET order_index = ? WHERE id = ?")
            .bind(index as i64)
            .bind(id)
            .execute(&mut *exec)
            .await?;
    }
    Ok(())
}

pub async fn count(db: &SqlitePool, table: &str) -> Result<i64, sqlx::Error> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    sqlx::query_scalar(&sql).fetch_one(db).await
}
