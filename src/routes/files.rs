//! 通用文件上传：封面 / 头像 / 音频。

use axum::{
    extract::{DefaultBodyLimit, State},
    routing::post,
    Router,
};
use serde::Deserialize;

use crate::dto::UploadOut;
use crate::error::{ok, ApiResult, AppError};
use crate::extract::{ApiMultipart, ApiQuery, CurrentUser};
use crate::media::{is_audio_kind, register_upload, save_field};
use crate::services::require_audio_upload_permission;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    let limit = crate::settings::MAX_AUDIO_SIZE as usize + 64 * 1024 * 1024;
    Router::new()
        .route("/files/upload", post(upload_file).layer(DefaultBodyLimit::max(limit)))
}

#[derive(Debug, Deserialize, Default)]
struct UploadQuery {
    #[serde(default)]
    kind: Option<String>,
}

async fn upload_file(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    ApiQuery(query): ApiQuery<UploadQuery>,
    ApiMultipart(mut multipart): ApiMultipart,
) -> ApiResult<crate::error::ApiOk<UploadOut>> {
    let kind = query.kind.unwrap_or_else(|| "cover".to_string());
    if is_audio_kind(&kind) {
        require_audio_upload_permission(&state, &user)?;
    }

    let mut saved = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|err| AppError::bad_request(format!("上传表单解析失败：{err}")))?
    {
        if field.name() == Some("file") && saved.is_none() {
            saved = Some(save_field(&mut field, &kind, &state.settings).await?);
        }
    }
    let Some(upload) = saved else {
        return Err(AppError::bad_request("缺少上传文件"));
    };

    register_upload(&state.db, &upload, &user).await?;
    tracing::info!("上传文件 user={} kind={} size={}", user.uuid, upload.kind, upload.size);
    Ok(ok(UploadOut {
        url: upload.url,
        filename: upload.filename,
        size: upload.size,
        content_type: upload.content_type,
        kind: upload.kind,
        md5: upload.md5,
    }))
}
