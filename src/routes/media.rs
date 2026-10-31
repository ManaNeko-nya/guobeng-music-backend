//! 静态媒体：/media/* 支持 Range（206）与 HEAD，路径穿越一律拒绝。

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::error::AppError;
use crate::extract::ApiPath;
use crate::media::join_relative;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/media/{*path}", get(serve))
}

/// 解析单段 Range；None 表示忽略（返回整文件），Some(Err) 表示 416。
fn parse_range(raw: &str, len: u64) -> Option<Result<(u64, u64), ()>> {
    let value = raw.trim();
    let rest = value.strip_prefix("bytes=")?;
    let first = rest.split(',').next().unwrap_or("").trim();
    let Some((start_raw, end_raw)) = first.split_once('-') else {
        return Some(Err(()));
    };
    if len == 0 {
        return Some(Err(()));
    }

    if start_raw.is_empty() {
        let Ok(suffix) = end_raw.trim().parse::<u64>() else {
            return Some(Err(()));
        };
        if suffix == 0 {
            return Some(Err(()));
        }
        let start = len.saturating_sub(suffix);
        return Some(Ok((start, len - 1)));
    }

    let Ok(start) = start_raw.trim().parse::<u64>() else {
        return Some(Err(()));
    };
    if start >= len {
        return Some(Err(()));
    }
    let end = if end_raw.trim().is_empty() {
        len - 1
    } else {
        match end_raw.trim().parse::<u64>() {
            Ok(value) => value.min(len - 1),
            Err(_) => return Some(Err(())),
        }
    };
    if start > end {
        return Some(Err(()));
    }
    Some(Ok((start, end)))
}

async fn serve(
    State(state): State<AppState>,
    ApiPath(relative): ApiPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let Some(path) = join_relative(&state.settings.media_dir, &relative) else {
        return AppError::not_found("文件不存在").into_response();
    };
    if !path.is_file() {
        return AppError::not_found("文件不存在").into_response();
    }
    let length = match tokio::fs::metadata(&path).await {
        Ok(meta) => meta.len(),
        Err(_) => return AppError::not_found("文件不存在").into_response(),
    };
    let content_type = mime_guess::from_path(&path)
        .first_or_octet_stream()
        .to_string();

    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_range(value, length));

    let (status, start, end) = match range {
        None => (StatusCode::OK, 0u64, length.saturating_sub(1)),
        Some(Err(())) => {
            let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            response.headers_mut().insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{length}"))
                    .unwrap_or_else(|_| HeaderValue::from_static("bytes */0")),
            );
            return response;
        }
        Some(Ok((start, end))) => (StatusCode::PARTIAL_CONTENT, start, end),
    };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ACCEPT_RANGES, "bytes");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{length}"),
        );
    }
    let size = if length == 0 { 0 } else { end - start + 1 };
    builder = builder.header(header::CONTENT_LENGTH, size);

    if method == Method::HEAD {
        return builder.body(Body::empty()).unwrap_or_else(|_| {
            AppError::internal("响应构造失败").into_response()
        });
    }

    let mut file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => return AppError::not_found("文件不存在").into_response(),
    };
    if status == StatusCode::PARTIAL_CONTENT
        && file.seek(std::io::SeekFrom::Start(start)).await.is_err()
    {
        return AppError::internal("文件读取失败").into_response();
    }
    let reader = file.take(size);
    builder
        .body(Body::from_stream(ReaderStream::new(reader)))
        .unwrap_or_else(|_| AppError::internal("响应构造失败").into_response())
}
