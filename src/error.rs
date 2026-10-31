//! 统一错误与响应封装：所有接口（含错误）都返回 `{code, message, data}`。

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub message: String,
    pub data: Option<Value>,
}

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), data: None }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, message)
    }

    /// 请求结构校验失败（对应 FastAPI 的 422），细节放进 data.errors。
    pub fn validation(field: &str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: "参数校验失败".to_string(),
            data: Some(json!({
                "errors": [{ "field": field, "message": message.into() }]
            })),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status.as_u16(), self.message)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = json!({
            "code": self.status.as_u16(),
            "message": self.message,
            "data": self.data,
        });
        (self.status, Json(body)).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        tracing::error!("数据库错误：{err}");
        Self::internal("服务器内部错误")
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        tracing::error!("IO 错误：{err}");
        Self::internal("服务器内部错误")
    }
}

pub type ApiResult<T> = Result<T, AppError>;

/// 成功响应包装：`ApiOk(value)` 序列化为 `{code:0, message:"ok", data:value}`。
pub struct ApiOk<T>(pub T);

impl<T: Serialize> IntoResponse for ApiOk<T> {
    fn into_response(self) -> Response {
        match serde_json::to_value(&self.0) {
            Ok(value) => (StatusCode::OK, Json(json!({ "code": 0, "message": "ok", "data": value })))
                .into_response(),
            Err(err) => {
                tracing::error!("响应序列化失败：{err}");
                AppError::internal("服务器内部错误").into_response()
            }
        }
    }
}

pub fn ok<T>(value: T) -> ApiOk<T> {
    ApiOk(value)
}
