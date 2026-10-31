//! 领域规则：可见性、权限、歌曲顺序、艺人关联、标签。
//!
//! 事务边界在路由层；本模块只操作传入的连接/事务，不自行提交。

use sqlx::sqlite::SqlitePool;

use crate::error::{ApiResult, AppError};
use crate::models::{AlbumRow, ArtistRow, TrackRow};
use crate::repo;
use crate::sqlutil::placeholders;
use crate::state::AppState;
use crate::timeutil::now_text;

// ------------------------------ 可见性 ------------------------------

/// 转义 LIKE 通配符，避免用户输入的 % / _ 影响模糊搜索（配合 ESCAPE '\'）。
pub fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub fn like_pattern(keyword: &str) -> String {
    format!("%{}%", escape_like(keyword))
}

/// (viewer_id, is_admin)；匿名用 -1，任何 owner_id 都不会等于它。
pub fn viewer_params(user: Option<&crate::models::UserRow>) -> (i64, i64) {
    match user {
        Some(user) => (user.id, if user.is_admin() { 1 } else { 0 }),
        None => (-1, 0),
    }
}

pub struct Clause {
    pub sql: String,
    pub binds: Vec<i64>,
}

pub fn visible_album_clause(alias: &str, user: Option<&crate::models::UserRow>) -> Clause {
    let (id, admin) = viewer_params(user);
    Clause {
        sql: format!("({alias}.is_public = 1 OR {alias}.owner_id = ? OR ? = 1)"),
        binds: vec![id, admin],
    }
}

/// 任意 tracks.id 表达式所属专辑的可见性；参数顺序 (viewer_id, is_admin)。
pub fn visible_album_of_track_sql(track_expr: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM tracks vt JOIN albums va ON va.id = vt.album_id
                 WHERE vt.id = {track_expr} AND (va.is_public = 1 OR va.owner_id = ? OR ? = 1))"
    )
}

pub fn visible_track_clause(alias: &str, user: Option<&crate::models::UserRow>) -> Clause {
    let (id, admin) = viewer_params(user);
    Clause {
        sql: format!(
            "{alias}.album_id IN (SELECT va.id FROM albums va WHERE va.is_public = 1 OR va.owner_id = ? OR ? = 1)"
        ),
        binds: vec![id, admin],
    }
}

pub fn album_visible(album: &AlbumRow, user: Option<&crate::models::UserRow>) -> bool {
    album.public()
        || user
            .map(|user| user.is_admin() || album.owner_id == Some(user.id))
            .unwrap_or(false)
}

pub async fn visible_album(
    db: &SqlitePool,
    album_id: i64,
    user: Option<&crate::models::UserRow>,
) -> ApiResult<AlbumRow> {
    let album = repo::fetch_album(db, album_id)
        .await?
        .ok_or_else(|| AppError::not_found("专辑不存在"))?;
    if !album_visible(&album, user) {
        return Err(AppError::not_found("专辑不存在"));
    }
    Ok(album)
}

pub async fn visible_track(
    db: &SqlitePool,
    track_id: i64,
    user: Option<&crate::models::UserRow>,
) -> ApiResult<(TrackRow, AlbumRow)> {
    let track = repo::fetch_track(db, track_id)
        .await?
        .ok_or_else(|| AppError::not_found("歌曲不存在"))?;
    let album = visible_album(db, track.album_id, user).await?;
    Ok((track, album))
}

pub async fn artist_or_404(db: &SqlitePool, artist_id: i64) -> ApiResult<ArtistRow> {
    repo::fetch_artist(db, artist_id)
        .await?
        .ok_or_else(|| AppError::not_found("艺人不存在"))
}

// ------------------------------ 权限 ------------------------------

pub fn require_owner_or_admin(
    user: &crate::models::UserRow,
    owner_id: Option<i64>,
) -> ApiResult<()> {
    if user.is_admin() {
        return Ok(());
    }
    if owner_id.is_none() || owner_id != Some(user.id) {
        return Err(AppError::forbidden("只能操作自己创建的内容"));
    }
    Ok(())
}

pub fn require_album_owner(
    user: &crate::models::UserRow,
    album: &AlbumRow,
) -> ApiResult<()> {
    require_owner_or_admin(user, album.owner_id)
}

pub fn require_track_manager(user: &crate::models::UserRow, album: &AlbumRow) -> ApiResult<()> {
    require_album_owner(user, album)
}

/// 作者本人、专辑创建者、管理员可删除用户产生的内容。
pub fn can_manage_authored_content(
    user: &crate::models::UserRow,
    author_id: Option<i64>,
    album_owner_id: Option<i64>,
) -> bool {
    user.is_admin() || author_id == Some(user.id) || album_owner_id == Some(user.id)
}

pub fn require_audio_upload_permission(state: &AppState, user: &crate::models::UserRow) -> ApiResult<()> {
    if user.is_admin() {
        return Ok(());
    }
    if !state.runtime.get().allow_user_audio_upload {
        return Err(AppError::forbidden("当前站点已关闭普通用户上传音频"));
    }
    Ok(())
}

// --------------------------- 艺人关联 ---------------------------

fn dedup(ids: &[i64]) -> Vec<i64> {
    let mut ordered = Vec::new();
    for id in ids {
        if !ordered.contains(id) {
            ordered.push(*id);
        }
    }
    ordered
}

async fn assert_artists_exist(
    exec: &mut sqlx::SqliteConnection,
    ids: &[i64],
) -> ApiResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let marks = placeholders(ids.len());
    let sql = format!("SELECT id FROM artists WHERE id IN ({marks})");
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for id in ids {
        query = query.bind(id);
    }
    let found: Vec<i64> = query.fetch_all(&mut *exec).await?;
    let missing: Vec<String> = ids
        .iter()
        .filter(|id| !found.contains(id))
        .map(|id| id.to_string())
        .collect();
    if !missing.is_empty() {
        return Err(AppError::bad_request(format!("艺人不存在：[{}]", missing.join(", "))));
    }
    Ok(())
}

async fn sync_join_rows(
    exec: &mut sqlx::SqliteConnection,
    table: &str,
    column: &str,
    parent_id: i64,
    ordered: &[i64],
) -> Result<(), sqlx::Error> {
    if ordered.is_empty() {
        sqlx::query(&format!("DELETE FROM {table} WHERE {column} = ?"))
            .bind(parent_id)
            .execute(&mut *exec)
            .await?;
        return Ok(());
    }
    let marks = placeholders(ordered.len());
    let sql = format!(
        "DELETE FROM {table} WHERE {column} = ? AND artist_id NOT IN ({marks})"
    );
    let mut query = sqlx::query(&sql).bind(parent_id);
    for id in ordered {
        query = query.bind(id);
    }
    query.execute(&mut *exec).await?;

    for (index, artist_id) in ordered.iter().enumerate() {
        let sql = format!(
            "INSERT INTO {table} ({column}, artist_id, position) VALUES (?, ?, ?)
             ON CONFLICT({column}, artist_id) DO UPDATE SET position = excluded.position"
        );
        sqlx::query(&sql)
            .bind(parent_id)
            .bind(artist_id)
            .bind(index as i64)
            .execute(&mut *exec)
            .await?;
    }
    Ok(())
}

/// 重建专辑艺人关联；主艺人始终在首位。
pub async fn sync_album_artists(
    exec: &mut sqlx::SqliteConnection,
    album_id: i64,
    main_artist_id: i64,
    artist_ids: &[i64],
) -> ApiResult<()> {
    let mut ordered = dedup(artist_ids);
    if !ordered.contains(&main_artist_id) {
        ordered.insert(0, main_artist_id);
    }
    assert_artists_exist(&mut *exec, &ordered).await?;
    sync_join_rows(&mut *exec, "album_artists", "album_id", album_id, &ordered).await?;
    Ok(())
}

/// 重建歌曲艺人关联；歌曲的合作艺人同时并入专辑艺人。
pub async fn sync_track_artists(
    exec: &mut sqlx::SqliteConnection,
    track_id: i64,
    album_id: i64,
    main_artist_id: i64,
    artist_ids: &[i64],
) -> ApiResult<()> {
    let ordered = dedup(artist_ids);
    assert_artists_exist(&mut *exec, &ordered).await?;
    sync_join_rows(&mut *exec, "track_artists", "track_id", track_id, &ordered).await?;

    let mut merged: Vec<i64> = sqlx::query_scalar(
        "SELECT artist_id FROM album_artists WHERE album_id = ? ORDER BY position, id",
    )
    .bind(album_id)
    .fetch_all(&mut *exec)
    .await?;
    merged.extend(ordered);
    sync_album_artists(&mut *exec, album_id, main_artist_id, &merged).await
}

// ------------------------------ 歌曲 ------------------------------

pub struct NewTrack {
    pub title: String,
    pub audio_url: Option<String>,
    pub file_size: Option<i64>,
    pub file_md5: Option<String>,
    pub duration_sec: Option<f64>,
    pub order_index: Option<i64>,
    pub artist_ids: Vec<i64>,
}

pub async fn create_track(
    exec: &mut sqlx::SqliteConnection,
    album: &AlbumRow,
    input: NewTrack,
) -> ApiResult<i64> {
    let title: String = input.title.trim().chars().take(255).collect();
    if title.is_empty() {
        return Err(AppError::bad_request("歌曲标题不能为空"));
    }
    if let Some(duration) = input.duration_sec {
        if duration < 0.0 {
            return Err(AppError::bad_request("歌曲时长不能为负数"));
        }
    }

    let next_order = repo::next_track_order(exec, album.id).await?;
    let position = match input.order_index {
        None => next_order,
        Some(value) => {
            let value = value.max(0).min(next_order);
            if value < next_order {
                sqlx::query(
                    "UPDATE tracks SET order_index = order_index + 1
                     WHERE album_id = ? AND order_index >= ?",
                )
                .bind(album.id)
                .bind(value)
                .execute(&mut *exec)
                .await?;
            }
            value
        }
    };

    let now = now_text();
    let track_id: i64 = sqlx::query_scalar(
        "INSERT INTO tracks (album_id, title, order_index, audio_url, file_size, file_md5,
                             duration_sec, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(album.id)
    .bind(&title)
    .bind(position)
    .bind(&input.audio_url)
    .bind(input.file_size)
    .bind(&input.file_md5)
    .bind(input.duration_sec)
    .bind(&now)
    .bind(&now)
    .fetch_one(&mut *exec)
    .await?;

    sync_track_artists(&mut *exec, track_id, album.id, album.artist_id, &input.artist_ids).await?;
    repo::renumber_tracks(&mut *exec, album.id).await?;
    Ok(track_id)
}

pub async fn reorder_tracks(
    exec: &mut sqlx::SqliteConnection,
    album_id: i64,
    track_ids: &[i64],
) -> ApiResult<()> {
    let existing: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM tracks WHERE album_id = ? ORDER BY order_index, id",
    )
    .bind(album_id)
    .fetch_all(&mut *exec)
    .await?;
    let mut wanted: Vec<i64> = track_ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let mut current = existing.clone();
    current.sort_unstable();
    if wanted != current || track_ids.len() != existing.len() {
        return Err(AppError::bad_request("歌曲列表不匹配，请提交该专辑下的全部歌曲 ID"));
    }
    for (index, track_id) in track_ids.iter().enumerate() {
        sqlx::query("UPDATE tracks SET order_index = ? WHERE id = ?")
            .bind(index as i64)
            .bind(track_id)
            .execute(&mut *exec)
            .await?;
    }
    Ok(())
}

// ------------------------------ 标签 ------------------------------

pub async fn set_album_tags(
    exec: &mut sqlx::SqliteConnection,
    album_id: i64,
    names: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM album_tags WHERE album_id = ?")
        .bind(album_id)
        .execute(&mut *exec)
        .await?;
    for name in names {
        sqlx::query("INSERT OR IGNORE INTO album_tags (album_id, name) VALUES (?, ?)")
            .bind(album_id)
            .bind(name)
            .execute(&mut *exec)
            .await?;
    }
    Ok(())
}

/// 删除专辑时需要清理的本地文件：封面 + 每首歌的音频。
pub async fn album_media_urls(db: &SqlitePool, album: &AlbumRow) -> Vec<String> {
    let mut urls = Vec::new();
    if let Some(cover) = &album.cover_url {
        urls.push(cover.clone());
    }
    match sqlx::query_scalar::<_, Option<String>>(
        "SELECT audio_url FROM tracks WHERE album_id = ?",
    )
    .bind(album.id)
    .fetch_all(db)
    .await
    {
        Ok(rows) => urls.extend(rows.into_iter().flatten()),
        Err(err) => tracing::error!("读取专辑音频地址失败：{err}"),
    }
    urls
}
