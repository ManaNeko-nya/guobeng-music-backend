//! 上传落盘与媒体文件管理：扩展名白名单、文件头校验、随机文件名、归属校验。

use std::path::{Path, PathBuf};

use axum::extract::multipart::Field;
use md5::{Digest, Md5};
use sqlx::sqlite::SqlitePool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::error::{ApiResult, AppError};
use crate::models::UserRow;
use crate::settings::{
    Settings, AUDIO_EXTENSIONS, IMAGE_EXTENSIONS, MAX_AUDIO_SIZE, MAX_IMAGE_SIZE,
};
use crate::timeutil::now_text;

pub const KIND_COVER: &str = "cover";
pub const KIND_AVATAR: &str = "avatar";
pub const KIND_AUDIO: &str = "audio";

pub struct SavedUpload {
    pub url: String,
    pub filename: String,
    pub original: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub kind: String,
    pub md5: String,
}

fn kind_dir(kind: &str) -> Option<&'static str> {
    match kind {
        KIND_COVER => Some("covers"),
        KIND_AVATAR => Some("avatars"),
        KIND_AUDIO => Some("audio"),
        _ => None,
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        KIND_COVER => "封面",
        KIND_AVATAR => "头像",
        _ => "音频",
    }
}

fn allowed_extensions(kind: &str) -> &'static [&'static str] {
    if kind == KIND_AUDIO {
        AUDIO_EXTENSIONS
    } else {
        IMAGE_EXTENSIONS
    }
}

fn max_size(kind: &str) -> u64 {
    if kind == KIND_AUDIO {
        MAX_AUDIO_SIZE
    } else {
        MAX_IMAGE_SIZE
    }
}

fn signature_ok(suffix: &str, head: &[u8]) -> bool {
    let at = |start: usize, end: usize| -> &[u8] {
        if head.len() >= end {
            &head[start..end]
        } else {
            &[]
        }
    };
    match suffix {
        "jpg" | "jpeg" => head.starts_with(&[0xff, 0xd8, 0xff]),
        "png" => head.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "gif" => head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a"),
        "bmp" => head.starts_with(b"BM"),
        "webp" => at(0, 4) == b"RIFF" && at(8, 12) == b"WEBP",
        "mp3" => {
            head.starts_with(b"ID3")
                || at(0, 2) == [0xff, 0xfb]
                || at(0, 2) == [0xff, 0xf3]
                || at(0, 2) == [0xff, 0xf2]
                || at(0, 2) == [0xff, 0xf1]
        }
        "flac" => head.starts_with(b"fLaC"),
        "wav" => at(0, 4) == b"RIFF" && at(8, 12) == b"WAVE",
        "ogg" | "opus" => head.starts_with(b"OggS"),
        "m4a" | "aac" => at(4, 8) == b"ftyp" || at(0, 2) == [0xff, 0xf1] || at(0, 2) == [0xff, 0xf9],
        _ => false,
    }
}

pub fn is_audio_kind(kind: &str) -> bool {
    kind == KIND_AUDIO
}

/// 流式保存上传文件：边写边限长、边算 MD5，失败时清理半成品。
pub async fn save_field(
    field: &mut Field<'_>,
    kind: &str,
    settings: &Settings,
) -> ApiResult<SavedUpload> {
    let dir = kind_dir(kind)
        .ok_or_else(|| AppError::bad_request(format!("不支持的上传类型：{kind}")))?;
    let label = kind_label(kind);

    let original = field.file_name().unwrap_or_default().to_string();
    let suffix = Path::new(&original)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let allowed = allowed_extensions(kind);
    if !allowed.contains(&suffix.as_str()) {
        return Err(AppError::bad_request(format!(
            "{label}格式不支持，允许：{}",
            allowed.join(", ")
        )));
    }
    let content_type = field.content_type().map(|value| value.to_string());

    let directory = settings.media_dir.join(dir);
    tokio::fs::create_dir_all(&directory).await?;
    let filename = format!("{}.{suffix}", Uuid::new_v4().simple());
    let dest = directory.join(&filename);
    let limit = max_size(kind);

    let mut file = tokio::fs::File::create(&dest).await?;
    let mut hasher = Md5::new();
    let mut size: u64 = 0;

    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                size += chunk.len() as u64;
                if size > limit {
                    drop(file);
                    let _ = tokio::fs::remove_file(&dest).await;
                    return Err(AppError::bad_request(format!(
                        "文件超过大小限制（最大 {} MB）",
                        limit / (1024 * 1024)
                    )));
                }
                hasher.update(&chunk);
                if let Err(err) = file.write_all(&chunk).await {
                    drop(file);
                    let _ = tokio::fs::remove_file(&dest).await;
                    return Err(AppError::from(err));
                }
            }
            Ok(None) => break,
            Err(err) => {
                drop(file);
                let _ = tokio::fs::remove_file(&dest).await;
                return Err(AppError::bad_request(format!("上传中断：{err}")));
            }
        }
    }
    file.flush().await?;
    drop(file);

    let mut head = [0u8; 12];
    let mut probe = match tokio::fs::File::open(&dest).await {
        Ok(file) => file,
        Err(err) => {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(AppError::from(err));
        }
    };
    let read = match probe.read(&mut head).await {
        Ok(read) => read,
        Err(err) => {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(AppError::from(err));
        }
    };
    drop(probe);
    if !signature_ok(&suffix, &head[..read]) {
        let _ = tokio::fs::remove_file(&dest).await;
        return Err(AppError::bad_request(format!("{label}内容与扩展名不符")));
    }

    let md5 = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    tracing::info!("保存上传文件 kind={kind} name={filename} size={size} md5={md5}");
    Ok(SavedUpload {
        url: format!("/media/{dir}/{filename}"),
        filename,
        original,
        size,
        content_type,
        kind: kind.to_string(),
        md5,
    })
}

/// 把相对路径安全拼到 base 下：拒绝空段、`.`、`..`、反斜杠与盘符。
pub fn join_relative(base: &Path, relative: &str) -> Option<PathBuf> {
    let mut path = base.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        if part.contains('\\') || part.contains(':') {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

pub fn resolve_media_path(settings: &Settings, relative_url: &str) -> Option<PathBuf> {
    join_relative(&settings.media_dir, relative_url.strip_prefix("/media/")?)
}

/// 登记上传文件的归属（调用方负责提交事务）。
pub async fn register_upload(
    pool: &SqlitePool,
    upload: &SavedUpload,
    user: &UserRow,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO media_files (url, kind, size, uploader_id, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&upload.url)
    .bind(&upload.kind)
    .bind(upload.size as i64)
    .bind(user.id)
    .bind(now_text())
    .execute(pool)
    .await?;
    Ok(())
}

/// 删除本地媒体文件：仅上传者本人或管理员可删，未登记文件仅管理员可删。
/// `trusted=true` 只用于回滚本请求刚上传、尚未入库的临时文件。
pub async fn delete_media_file(
    settings: &Settings,
    pool: &SqlitePool,
    relative_url: Option<&String>,
    actor: &UserRow,
    trusted: bool,
) {
    let Some(url) = relative_url else { return };
    let Some(path) = resolve_media_path(settings, url) else { return };
    if !path.is_file() {
        return;
    }

    if !trusted {
        let record = sqlx::query_as::<_, crate::models::MediaFileRow>(
            "SELECT url, uploader_id FROM media_files WHERE url = ?",
        )
        .bind(url)
        .fetch_optional(pool)
        .await;
        match record {
            Ok(Some(row)) => {
                if row.uploader_id != Some(actor.id) && !actor.is_admin() {
                    tracing::warn!("拒绝删除他人上传的媒体文件 url={url} actor={}", actor.uuid);
                    return;
                }
            }
            Ok(None) => {
                if !actor.is_admin() {
                    tracing::warn!("拒绝删除来源不明的媒体文件 url={url} actor={}", actor.uuid);
                    return;
                }
            }
            Err(err) => {
                tracing::error!("查询媒体文件归属失败：{err}");
                return;
            }
        }
    }

    if let Err(err) = tokio::fs::remove_file(&path).await {
        tracing::warn!("删除文件失败 {}：{err}", path.display());
    } else {
        let _ = sqlx::query("DELETE FROM media_files WHERE url = ?")
            .bind(url)
            .execute(pool)
            .await;
    }
}
