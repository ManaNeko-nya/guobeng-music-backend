//! SQLite 连接池与表结构。
//!
//! 全新实现，不兼容旧 Python 库；媒体目录仍沿用 `data/media/{covers,avatars,audio}`。

use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous};

use crate::settings::Settings;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id             INTEGER PRIMARY KEY,
    uuid           TEXT NOT NULL UNIQUE,
    username       TEXT NOT NULL UNIQUE,
    password_hash  TEXT NOT NULL,
    nickname       TEXT NOT NULL DEFAULT '',
    avatar_url     TEXT,
    bio            TEXT,
    role           TEXT NOT NULL DEFAULT 'user',
    is_active      INTEGER NOT NULL DEFAULT 1,
    last_login_at  TEXT,
    last_login_ip  TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artists (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    cover_url  TEXT,
    bio        TEXT,
    owner_id   INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_artists_created_at ON artists(created_at);
CREATE INDEX IF NOT EXISTS ix_artists_owner ON artists(owner_id);

CREATE TABLE IF NOT EXISTS albums (
    id           INTEGER PRIMARY KEY,
    artist_id    INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
    title        TEXT NOT NULL,
    external_id  TEXT,
    cover_url    TEXT,
    description  TEXT,
    album_type   TEXT NOT NULL DEFAULT 'album',
    release_year INTEGER,
    release_date TEXT,
    score        REAL,
    owner_id     INTEGER REFERENCES users(id) ON DELETE SET NULL,
    view_count   INTEGER NOT NULL DEFAULT 0,
    is_public    INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_albums_artist ON albums(artist_id);
CREATE INDEX IF NOT EXISTS ix_albums_title ON albums(title);
CREATE INDEX IF NOT EXISTS ix_albums_created_at ON albums(created_at);
CREATE INDEX IF NOT EXISTS ix_albums_updated_at ON albums(updated_at);
CREATE UNIQUE INDEX IF NOT EXISTS uq_albums_external_id
    ON albums(external_id) WHERE external_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS album_artists (
    id        INTEGER PRIMARY KEY,
    album_id  INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
    position  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (album_id, artist_id)
);
CREATE INDEX IF NOT EXISTS ix_album_artists_artist ON album_artists(artist_id);

CREATE TABLE IF NOT EXISTS album_tags (
    id       INTEGER PRIMARY KEY,
    album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    name     TEXT NOT NULL,
    UNIQUE (album_id, name)
);
CREATE INDEX IF NOT EXISTS ix_album_tags_name ON album_tags(name);

CREATE TABLE IF NOT EXISTS tracks (
    id           INTEGER PRIMARY KEY,
    album_id     INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    title        TEXT NOT NULL,
    order_index  INTEGER NOT NULL DEFAULT 0,
    audio_url    TEXT,
    file_size    INTEGER,
    file_md5     TEXT,
    duration_sec REAL,
    lyrics       TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_tracks_album_order ON tracks(album_id, order_index);
CREATE INDEX IF NOT EXISTS ix_tracks_file_md5 ON tracks(file_md5);

CREATE TABLE IF NOT EXISTS track_artists (
    id        INTEGER PRIMARY KEY,
    track_id  INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    artist_id INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
    position  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (track_id, artist_id)
);
CREATE INDEX IF NOT EXISTS ix_track_artists_artist ON track_artists(artist_id);

CREATE TABLE IF NOT EXISTS comments (
    id            INTEGER PRIMARY KEY,
    track_id      INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    user_id       INTEGER REFERENCES users(id) ON DELETE SET NULL,
    parent_id     INTEGER REFERENCES comments(id) ON DELETE CASCADE,
    content       TEXT NOT NULL,
    author_name   TEXT,
    author_avatar TEXT,
    source        TEXT,
    source_id     TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_comments_track_created ON comments(track_id, created_at);
CREATE INDEX IF NOT EXISTS ix_comments_parent ON comments(parent_id);

CREATE TABLE IF NOT EXISTS favorite_lists (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    position   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (user_id, name)
);
CREATE INDEX IF NOT EXISTS ix_favorite_lists_user ON favorite_lists(user_id);

CREATE TABLE IF NOT EXISTS favorites (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    track_id     INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    collected_at TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE (user_id, track_id)
);
CREATE INDEX IF NOT EXISTS ix_favorites_track ON favorites(track_id);

CREATE TABLE IF NOT EXISTS favorite_items (
    id          INTEGER PRIMARY KEY,
    list_id     INTEGER NOT NULL REFERENCES favorite_lists(id) ON DELETE CASCADE,
    favorite_id INTEGER NOT NULL REFERENCES favorites(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    UNIQUE (list_id, favorite_id)
);
CREATE INDEX IF NOT EXISTS ix_favorite_items_favorite ON favorite_items(favorite_id);
CREATE INDEX IF NOT EXISTS ix_favorite_items_position ON favorite_items(list_id, position);

CREATE TABLE IF NOT EXISTS listen_history (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    track_id     INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    progress_sec REAL NOT NULL DEFAULT 0,
    duration_sec REAL NOT NULL DEFAULT 0,
    completed    INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE (user_id, track_id)
);
CREATE INDEX IF NOT EXISTS ix_history_user_updated ON listen_history(user_id, updated_at);

CREATE TABLE IF NOT EXISTS media_files (
    id          INTEGER PRIMARY KEY,
    url         TEXT NOT NULL UNIQUE,
    kind        TEXT NOT NULL,
    size        INTEGER,
    uploader_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_media_files_uploader ON media_files(uploader_id);

CREATE TABLE IF NOT EXISTS refresh_tokens (
    id          INTEGER PRIMARY KEY,
    user_id     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,
    expires_at  TEXT NOT NULL,
    revoked_at  TEXT,
    replaced_by TEXT,
    user_agent  TEXT,
    ip          TEXT,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_refresh_tokens_user ON refresh_tokens(user_id);
"#;

pub async fn init(settings: &Settings) -> Result<SqlitePool, String> {
    let options = SqliteConnectOptions::new()
        .filename(&settings.database_path)
        .create_if_missing(true)
    .foreign_keys(true)
    .journal_mode(SqliteJournalMode::Wal)
    .synchronous(SqliteSynchronous::Normal)
    .busy_timeout(Duration::from_secs(15));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(15))
        .connect_with(options)
        .await
        .map_err(|err| format!("无法打开数据库 {}：{err}", settings.database_path.display()))?;

    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .map_err(|err| format!("建表失败：{err}"))?;

    Ok(pool)
}
