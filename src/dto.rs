//! 对外序列化结构：字段名与顺序按 docs/API.md 固定，客户端直接依赖。

use serde::Serialize;

use crate::timeutil::to_iso;

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
    pub total_pages: i64,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, total: i64, page: i64, page_size: i64) -> Self {
        let total_pages = if page_size > 0 { (total + page_size - 1) / page_size } else { 0 };
        Self { items, total, page, page_size, total_pages }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct UserPublic {
    pub id: String,
    pub username: String,
    pub nickname: String,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub role: String,
    pub created_at: String,
}

impl From<&crate::models::UserRow> for UserPublic {
    fn from(row: &crate::models::UserRow) -> Self {
        Self {
            id: row.uuid.clone(),
            username: row.username.clone(),
            nickname: if row.nickname.is_empty() {
                row.username.clone()
            } else {
                row.nickname.clone()
            },
            avatar_url: row.avatar_url.clone(),
            bio: row.bio.clone(),
            role: row.role.clone(),
            created_at: to_iso(&row.created_at),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct UserPrivate {
    #[serde(flatten)]
    pub base: UserPublic,
    pub last_login_at: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ArtistBrief {
    pub id: i64,
    pub name: String,
    pub cover_url: Option<String>,
    pub bio: Option<String>,
    pub album_count: i64,
    pub track_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&crate::models::ArtistRow> for ArtistBrief {
    fn from(row: &crate::models::ArtistRow) -> Self {
        Self {
            id: row.id,
            name: row.name.clone(),
            cover_url: row.cover_url.clone(),
            bio: row.bio.clone(),
            album_count: row.album_count,
            track_count: row.track_count,
            created_at: to_iso(&row.created_at),
            updated_at: to_iso(&row.updated_at),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ArtistDetail {
    #[serde(flatten)]
    pub base: ArtistBrief,
    pub owner_id: Option<String>,
    pub albums: Vec<AlbumBrief>,
}

#[derive(Debug, Serialize, Clone)]
pub struct AlbumBrief {
    pub id: i64,
    pub artist_id: i64,
    pub title: String,
    pub cover_url: Option<String>,
    pub description: Option<String>,
    pub album_type: String,
    pub release_year: Option<i64>,
    pub release_date: Option<String>,
    pub score: Option<f64>,
    pub tags: Vec<String>,
    pub track_count: i64,
    pub view_count: i64,
    pub is_public: bool,
    pub created_at: String,
    pub updated_at: String,
    pub artist: Option<ArtistBrief>,
    pub artists: Vec<ArtistBrief>,
    pub external_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AlbumDetail {
    #[serde(flatten)]
    pub base: AlbumBrief,
    pub owner_id: Option<String>,
    pub tracks: Vec<TrackOut>,
}

#[derive(Debug, Serialize, Clone)]
pub struct TrackOut {
    pub id: i64,
    pub album_id: i64,
    pub title: String,
    pub order_index: i64,
    pub audio_url: Option<String>,
    pub file_size: Option<i64>,
    pub file_md5: Option<String>,
    pub duration_sec: Option<f64>,
    pub has_lyrics: bool,
    pub comment_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub artists: Vec<ArtistBrief>,
}

#[derive(Debug, Serialize, Clone)]
pub struct TrackWithAlbum {
    #[serde(flatten)]
    pub base: TrackOut,
    pub album: Option<AlbumBrief>,
}

#[derive(Debug, Serialize, Clone)]
pub struct CommentOut {
    pub id: i64,
    pub track_id: i64,
    pub parent_id: Option<i64>,
    pub content: String,
    pub created_at: String,
    pub author: Option<String>,
    pub author_id: Option<String>,
    pub author_avatar: Option<String>,
    pub source: Option<String>,
    pub replies: Vec<CommentOut>,
}

impl CommentOut {
    pub fn from_row(row: &crate::models::CommentRow) -> Self {
        let (author, author_id, avatar) = match (&row.user_uuid, &row.user_username) {
            (Some(uuid), Some(username)) => {
                let name = row
                    .user_nickname
                    .clone()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| username.clone());
                (Some(name), Some(uuid.clone()), row.user_avatar.clone())
            }
            _ => (
                Some(
                    row.author_name
                        .clone()
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "已注销用户".to_string()),
                ),
                None,
                row.author_avatar.clone(),
            ),
        };
        Self {
            id: row.id,
            track_id: row.track_id,
            parent_id: row.parent_id,
            content: row.content.clone(),
            created_at: to_iso(&row.created_at),
            author,
            author_id,
            author_avatar: avatar,
            source: row.source.clone(),
            replies: Vec::new(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct FavoriteListOut {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub cover_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct FavoriteOut {
    pub id: i64,
    pub track_id: i64,
    pub created_at: String,
    pub collected_at: Option<String>,
    pub list_ids: Vec<i64>,
    pub track: Option<TrackWithAlbum>,
}

#[derive(Debug, Serialize)]
pub struct FavoriteCheck {
    pub track_id: i64,
    pub favorited: bool,
    pub list_ids: Vec<i64>,
    pub collected_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HistoryOut {
    pub id: i64,
    pub track_id: i64,
    pub progress_sec: f64,
    pub duration_sec: f64,
    pub completed: bool,
    pub updated_at: String,
    pub track: Option<TrackWithAlbum>,
}

#[derive(Debug, Serialize)]
pub struct UploadOut {
    pub url: String,
    pub filename: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub kind: String,
    pub md5: String,
}

#[derive(Debug, Serialize)]
pub struct LyricsOut {
    pub track_id: i64,
    pub lyrics: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub refresh_expires_in: i64,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub user: UserPublic,
    pub tokens: TokenPair,
}

pub fn lyrics_out(track_id: i64, lyrics: &Option<String>, updated_at: &str) -> LyricsOut {
    LyricsOut {
        track_id,
        lyrics: lyrics.clone().unwrap_or_default(),
        updated_at: Some(to_iso(updated_at)),
    }
}

pub fn history_out(row: &crate::models::HistoryRow, track: Option<TrackWithAlbum>) -> HistoryOut {
    HistoryOut {
        id: row.id,
        track_id: row.track_id,
        progress_sec: row.progress_sec,
        duration_sec: row.duration_sec,
        completed: row.completed != 0,
        updated_at: to_iso(&row.updated_at),
        track,
    }
}
