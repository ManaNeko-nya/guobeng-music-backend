//! 数据库行结构。时间一律以文本读取，输出时再转 ISO。

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub id: i64,
    pub uuid: String,
    pub username: String,
    pub password_hash: String,
    pub nickname: String,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub role: String,
    pub is_active: i64,
    pub last_login_at: Option<String>,
    pub last_login_ip: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl UserRow {
    pub fn is_root(&self) -> bool {
        self.role == "root"
    }

    pub fn is_admin(&self) -> bool {
        self.role == "root" || self.role == "admin"
    }

    pub fn active(&self) -> bool {
        self.is_active != 0
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ArtistRow {
    pub id: i64,
    pub name: String,
    pub cover_url: Option<String>,
    pub bio: Option<String>,
    pub owner_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub album_count: i64,
    pub track_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AlbumRow {
    pub id: i64,
    pub artist_id: i64,
    pub title: String,
    pub external_id: Option<String>,
    pub cover_url: Option<String>,
    pub description: Option<String>,
    pub album_type: String,
    pub release_year: Option<i64>,
    pub release_date: Option<String>,
    pub score: Option<f64>,
    pub owner_id: Option<i64>,
    pub view_count: i64,
    pub is_public: i64,
    pub created_at: String,
    pub updated_at: String,
    pub track_count: i64,
}

impl AlbumRow {
    pub fn public(&self) -> bool {
        self.is_public != 0
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TrackRow {
    pub id: i64,
    pub album_id: i64,
    pub title: String,
    pub order_index: i64,
    pub audio_url: Option<String>,
    pub file_size: Option<i64>,
    pub file_md5: Option<String>,
    pub duration_sec: Option<f64>,
    pub lyrics: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub comment_count: i64,
}

/// 评论行附带作者账号信息（一条 JOIN 查完，避免 N+1）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CommentRow {
    pub id: i64,
    pub track_id: i64,
    pub user_id: Option<i64>,
    pub parent_id: Option<i64>,
    pub content: String,
    pub author_name: Option<String>,
    pub author_avatar: Option<String>,
    pub source: Option<String>,
    pub created_at: String,
    pub user_uuid: Option<String>,
    pub user_nickname: Option<String>,
    pub user_username: Option<String>,
    pub user_avatar: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FavoriteRow {
    pub id: i64,
    pub track_id: i64,
    pub collected_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FavoriteItemRow {
    pub list_id: i64,
    pub favorite_id: i64,
    pub position: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FavoriteListRow {
    pub id: i64,
    pub name: String,
    pub position: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HistoryRow {
    pub id: i64,
    pub track_id: i64,
    pub progress_sec: f64,
    pub duration_sec: f64,
    pub completed: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaFileRow {
    pub url: String,
    pub uploader_id: Option<i64>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RefreshTokenRow {
    pub id: i64,
    pub user_id: i64,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

impl RefreshTokenRow {
    pub fn valid(&self) -> bool {
        self.revoked_at.is_none() && self.expires_at > crate::timeutil::now_text()
    }
}
