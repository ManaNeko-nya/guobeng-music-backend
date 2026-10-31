//! HTTP 提取器：鉴权用户、分页、JSON/查询参数错误统一成 422 信封。

use std::net::{IpAddr, SocketAddr};

use axum::{
    body::to_bytes,
    extract::{ConnectInfo, FromRequest, FromRequestParts, Query, Request},
    http::{header, request::Parts},
};
use serde::de::DeserializeOwned;

use crate::error::{ApiResult, AppError};
use crate::models::UserRow;
use crate::security::decode_access_token;
use crate::state::AppState;

// ------------------------------ 客户端 IP ------------------------------

fn peer_ip(parts: &Parts) -> Option<IpAddr> {
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(value) => value.is_loopback(),
        IpAddr::V6(value) => {
            value.is_loopback() || value.to_ipv4_mapped().map(|v4| v4.is_loopback()).unwrap_or(false)
        }
    }
}

/// 只有直连对端是本机时才采信 X-Forwarded-For，并取最右一跳。
fn resolve_ip(parts: &Parts) -> (String, bool) {
    let peer = peer_ip(parts);
    if let Some(peer) = peer {
        if is_loopback(peer) {
            if let Some(raw) = parts.headers.get("x-forwarded-for").and_then(|value| value.to_str().ok()) {
                if let Some(last) = raw
                    .split(',')
                    .map(|item| item.trim())
                    .rfind(|item| !item.is_empty())
                {
                    let local = last.eq_ignore_ascii_case("localhost")
                        || last.parse::<IpAddr>().map(is_loopback).unwrap_or(false);
                    return (last.to_string(), local);
                }
            }
        }
    }
    match peer {
        Some(ip) => (ip.to_string(), is_loopback(ip)),
        None => ("unknown".to_string(), false),
    }
}

pub fn client_ip(parts: &Parts) -> String {
    resolve_ip(parts).0
}

pub fn local_request(parts: &Parts) -> bool {
    resolve_ip(parts).1
}

/// 请求来源信息，供登录/刷新等需要 IP 与 UA 的接口使用。
pub struct ClientInfo {
    pub ip: String,
    pub local: bool,
    pub user_agent: Option<String>,
}

impl FromRequestParts<AppState> for ClientInfo {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &AppState) -> Result<Self, AppError> {
        let (ip, local) = resolve_ip(parts);
        Ok(ClientInfo { ip, local, user_agent: user_agent(parts) })
    }
}

pub fn user_agent(parts: &Parts) -> Option<String> {
    parts
        .headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(255).collect())
}

// ------------------------------ 鉴权 ------------------------------

fn bearer_token(parts: &Parts) -> Option<String> {
    let raw = parts.headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let token = raw.strip_prefix("Bearer ").or_else(|| raw.strip_prefix("bearer "))?;
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

pub async fn load_user(parts: &Parts, state: &AppState) -> Option<UserRow> {
    let token = bearer_token(parts)?;
    let claims = decode_access_token(&token, &state.settings.secret_key)?;
    let user = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE uuid = ?")
        .bind(&claims.sub)
        .fetch_optional(&state.db)
        .await
        .ok()??;
    if !user.active() {
        return None;
    }
    if user.is_root() && !local_request(parts) {
        tracing::warn!(
            "拒绝 root 账户的远程访问 ip={}",
            client_ip(parts)
        );
        return None;
    }
    Some(user)
}

pub struct CurrentUser(pub UserRow);

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        match load_user(parts, state).await {
            Some(user) => Ok(CurrentUser(user)),
            None => Err(AppError::unauthorized("请先登录")),
        }
    }
}

pub struct OptionalUser(pub Option<UserRow>);

impl FromRequestParts<AppState> for OptionalUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        Ok(OptionalUser(load_user(parts, state).await))
    }
}

// ------------------------------ 分页 ------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Pagination {
    pub page: i64,
    pub page_size: i64,
}

impl Pagination {
    pub fn offset(self) -> i64 {
        (self.page - 1) * self.page_size
    }
}

#[derive(serde::Deserialize, Default)]
struct PageQuery {
    #[serde(default)]
    page: Option<String>,
    #[serde(default)]
    page_size: Option<String>,
}

impl FromRequestParts<AppState> for Pagination {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        let Query(query) = Query::<PageQuery>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::validation("query", "查询参数格式不正确"))?;

        let page = match query.page.as_deref() {
            None | Some("") => 1,
            Some(raw) => match raw.parse::<i64>() {
                Ok(value) if value >= 1 => value,
                _ => return Err(AppError::validation("page", "必须是不小于 1 的整数")),
            },
        };
        let page_size = match query.page_size.as_deref() {
            None | Some("") => 20,
            Some(raw) => match raw.parse::<i64>() {
                Ok(value) if (1..=100).contains(&value) => value,
                _ => return Err(AppError::validation("page_size", "必须在 1-100 之间")),
            },
        };
        Ok(Pagination { page, page_size })
    }
}

// --------------------------- JSON / 查询包装 ---------------------------

pub struct ApiJson<T>(pub T);

impl<T> FromRequest<AppState> for ApiJson<T>
where
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, AppError> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(json_validation_error(&rejection.body_text())),
        }
    }
}

/// 取出 serde 报错里反引号包裹的字段名，例如 ``missing field `name` ``。
fn quoted_field(detail: &str, keyword: &str) -> Option<String> {
    let rest = &detail[detail.find(keyword)? + keyword.len()..];
    let start = rest.find('`')? + 1;
    let end = rest[start..].find('`')? + start;
    Some(rest[start..end].to_string())
}

/// 把 serde 的英文报错翻译成 `{field, message}`，字段名能在报错里定位就带上。
fn json_validation_error(detail: &str) -> AppError {
    if let Some(field) = quoted_field(detail, "missing field") {
        return AppError::validation(&field, "缺少必填字段");
    }
    if let Some(field) = quoted_field(detail, "unknown field") {
        return AppError::validation(&field, "不支持的字段");
    }
    if detail.contains("EOF")
        || detail.contains("expected value")
        || detail.contains("expected ident")
        || detail.contains("key must be a string")
    {
        return AppError::validation("body", "请求体不是合法的 JSON");
    }
    AppError::validation("body", "请求体格式不正确")
}

/// 允许空请求体：空 body 视为 None，供 logout 这类可选 body 的接口使用。
pub struct MaybeJson<T>(pub Option<T>);

impl<T> FromRequest<AppState> for MaybeJson<T>
where
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request(req: Request, _state: &AppState) -> Result<Self, AppError> {
        let bytes = to_bytes(req.into_body(), 64 * 1024)
            .await
            .map_err(|_| AppError::bad_request("请求体读取失败"))?;
        if bytes.is_empty() {
            return Ok(MaybeJson(None));
        }
        match serde_json::from_slice::<T>(&bytes) {
            Ok(value) => Ok(MaybeJson(Some(value))),
            Err(err) => Err(json_validation_error(&err.to_string())),
        }
    }
}

pub struct ApiQuery<T>(pub T);

impl<T> FromRequestParts<AppState> for ApiQuery<T>
where
    T: DeserializeOwned + Send,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AppError> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ApiQuery(value)),
            Err(err) => Err(json_validation_error(&err.body_text())),
        }
    }
}

pub struct ApiMultipart(pub axum::extract::Multipart);

impl FromRequest<AppState> for ApiMultipart {
    type Rejection = AppError;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, AppError> {
        axum::extract::Multipart::from_request(req, state)
            .await
            .map(ApiMultipart)
            .map_err(|err| AppError::bad_request(format!("上传表单解析失败：{err}")))
    }
}

pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, AppError> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(ApiPath(value)),
            Err(_) => Err(AppError::validation("path", "路径参数格式不正确")),
        }
    }
}

pub fn require_admin_only_create(state: &AppState, user: &UserRow, what: &str) -> ApiResult<()> {
    if state.settings.admin_only_create && !user.is_admin() {
        return Err(AppError::forbidden(format!("仅管理员可以创建{what}")));
    }
    Ok(())
}
