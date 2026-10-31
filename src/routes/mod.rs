//! HTTP 路由装配：业务接口统一挂在 /api/v1，媒体与健康检查独立。

pub mod albums;
pub mod artists;
pub mod auth;
pub mod comments;
pub mod favorites;
pub mod files;
pub mod health;
pub mod history;
pub mod media;
pub mod tracks;
pub mod users;

use axum::Router;

use crate::state::AppState;

pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(auth::router())
        .merge(users::router())
        .merge(artists::router())
        .merge(albums::router())
        .merge(tracks::album_tracks_router())
        .merge(tracks::tracks_router())
        .merge(comments::router())
        .merge(favorites::router())
        .merge(favorites::lists_router())
        .merge(history::router())
        .merge(files::router())
}
