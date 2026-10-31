//! 请求体结构：字段语义与校验规则对齐原 API（含「显式 null」与「未传」的区分）。

use chrono::NaiveDate;
use serde::{Deserialize, Deserializer};

use crate::error::AppError;

/// 让 `Option<Option<T>>` 能区分「未传」= None 与「显式 null」= Some(None)。
pub fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlbumType {
    Album,
    Ep,
    Single,
    Compilation,
    Ost,
}

impl AlbumType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Ep => "ep",
            Self::Single => "single",
            Self::Compilation => "compilation",
            Self::Ost => "ost",
        }
    }
}

/// 标签既接受数组也接受分隔字符串（`,` `，` `、` 空白）。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TagsInput {
    Text(String),
    List(Vec<String>),
    Numbers(Vec<i64>),
}

const TAG_MAX_COUNT: usize = 20;
const TAG_MAX_LENGTH: usize = 64;

pub fn normalize_tags(input: Option<&TagsInput>) -> Vec<String> {
    let Some(input) = input else { return Vec::new() };
    let raw: Vec<String> = match input {
        TagsInput::Text(text) => vec![text.clone()],
        TagsInput::List(list) => list.clone(),
        TagsInput::Numbers(list) => list.iter().map(|item| item.to_string()).collect(),
    };
    let mut names: Vec<String> = Vec::new();
    for item in raw {
        for part in item.split(|c: char| c == ',' || c == '，' || c == '、' || c.is_whitespace()) {
            let name: String = part.trim().chars().take(TAG_MAX_LENGTH).collect();
            if name.is_empty() {
                continue;
            }
            if !names.contains(&name) {
                names.push(name);
            }
            if names.len() >= TAG_MAX_COUNT {
                return names;
            }
        }
    }
    names
}

fn char_len(value: &str) -> usize {
    value.chars().count()
}

pub fn ensure_length(
    field: &str,
    value: &str,
    min: usize,
    max: usize,
) -> Result<(), AppError> {
    let length = char_len(value);
    if length < min {
        return Err(AppError::validation(field, format!("长度不能少于 {min} 个字符")));
    }
    if length > max {
        return Err(AppError::validation(field, format!("长度不能超过 {max} 个字符")));
    }
    Ok(())
}

/// 封面/头像/音频地址只接受站内路径或 http(s) 外链。
pub fn validate_media_url(field: &str, value: &str) -> Result<Option<String>, AppError> {
    let text = value.trim();
    if text.is_empty() {
        return Ok(None);
    }
    if !(text.starts_with('/') || text.starts_with("http://") || text.starts_with("https://")) {
        return Err(AppError::validation(field, "地址必须以 / 或 http(s):// 开头"));
    }
    if char_len(text) > 512 {
        return Err(AppError::validation(field, "长度不能超过 512 个字符"));
    }
    Ok(Some(text.to_string()))
}

pub fn parse_release_date(field: &str, raw: &str) -> Result<Option<String>, AppError> {
    let text = raw.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let candidate = if text.len() >= 10 && text.is_char_boundary(10) {
        &text[..10]
    } else {
        text
    };
    match NaiveDate::parse_from_str(candidate, "%Y-%m-%d") {
        Ok(date) => Ok(Some(date.format("%Y-%m-%d").to_string())),
        Err(_) => Err(AppError::validation(field, "日期格式应为 YYYY-MM-DD")),
    }
}

// ------------------------------- 账户 -------------------------------

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub nickname: Option<String>,
}

impl RegisterRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        let name = self.username.trim();
        ensure_length("body.username", name, 3, 32)?;
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
        {
            return Err(AppError::validation("body.username", "只能包含字母、数字与 _ . -"));
        }
        ensure_length("body.password", &self.password, 8, 128)?;
        if let Some(nickname) = &self.nickname {
            if char_len(nickname) > 32 {
                return Err(AppError::validation("body.nickname", "长度不能超过 32 个字符"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub account: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct LogoutRequest {
    #[serde(default)]
    pub refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PasswordChangeRequest {
    pub old_password: String,
    pub new_password: String,
}

impl PasswordChangeRequest {
    pub fn validate(&self) -> Result<(), AppError> {
        ensure_length("body.new_password", &self.new_password, 8, 128)
    }
}

#[derive(Debug, Deserialize)]
pub struct ProfileUpdateRequest {
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub bio: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub avatar_url: Option<Option<String>>,
}

// ------------------------------- 艺人 -------------------------------

#[derive(Debug, Deserialize)]
pub struct ArtistCreate {
    pub name: String,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub bio: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ArtistUpdate {
    #[serde(default, deserialize_with = "double_option")]
    pub name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub cover_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub bio: Option<Option<String>>,
}

// ------------------------------- 专辑 -------------------------------

#[derive(Debug, Deserialize)]
pub struct AlbumCreate {
    pub artist_id: i64,
    pub title: String,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_album_type")]
    pub album_type: AlbumType,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub release_year: Option<i64>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub tags: Option<TagsInput>,
    #[serde(default = "default_true")]
    pub is_public: bool,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub artist_ids: Vec<i64>,
}

fn default_album_type() -> AlbumType {
    AlbumType::Album
}

#[derive(Debug, Deserialize, Default)]
pub struct AlbumUpdate {
    #[serde(default, deserialize_with = "double_option")]
    pub title: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub cover_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub description: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub album_type: Option<Option<AlbumType>>,
    #[serde(default, deserialize_with = "double_option")]
    pub release_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub release_year: Option<Option<i64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub score: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    pub tags: Option<Option<TagsInput>>,
    #[serde(default, deserialize_with = "double_option")]
    pub is_public: Option<Option<bool>>,
}

// ------------------------------- 歌曲 -------------------------------

#[derive(Debug, Deserialize)]
pub struct TrackCreate {
    pub title: String,
    #[serde(default)]
    pub audio_url: Option<String>,
    #[serde(default)]
    pub duration_sec: Option<f64>,
    #[serde(default)]
    pub order_index: Option<i64>,
    #[serde(default)]
    pub artist_ids: Vec<i64>,
}

#[derive(Debug, Deserialize, Default)]
pub struct TrackUpdate {
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct TrackReorderRequest {
    #[serde(default)]
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct LyricsUpdate {
    pub lyrics: String,
}

// ------------------------------- 评论 -------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentCreate {
    pub content: String,
    #[serde(default)]
    pub parent_id: Option<i64>,
}

// ------------------------------- 收藏 -------------------------------

#[derive(Debug, Deserialize)]
pub struct FavoriteListCreate {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FavoriteListUpdate {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct FavoriteListOrderUpdate {
    pub list_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FavoriteOrderUpdate {
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct FavoriteCreate {
    pub track_id: i64,
    pub list_ids: Vec<i64>,
    #[serde(default)]
    pub collected_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FavoriteUpdate {
    #[serde(default)]
    pub list_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub collected_at: Option<String>,
}

// ------------------------------ 播放历史 ------------------------------

#[derive(Debug, Deserialize)]
pub struct HistoryUpsert {
    pub track_id: i64,
    #[serde(default)]
    pub progress_sec: f64,
    #[serde(default)]
    pub duration_sec: f64,
    #[serde(default)]
    pub completed: Option<bool>,
}
