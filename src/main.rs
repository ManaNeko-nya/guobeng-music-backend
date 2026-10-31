//! MusicServer 后端（Rust 版）：艺人 / 专辑 / 歌曲 / 歌词 / 收藏 / 历史 / 账户。

mod db;
mod dto;
mod error;
mod extract;
mod hydrate;
mod lyrics;
mod media;
#[allow(dead_code)]
mod models;
mod payload;
mod repo;
mod routes;
mod runtime_config;
mod security;
mod services;
mod settings;
mod sqlutil;
mod state;
mod timeutil;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use axum::{
    extract::State,
    http::{header, HeaderName, HeaderValue, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use clap::Parser;
use rand::Rng;
use sqlx::sqlite::SqlitePool;
use tower_http::cors::{Any, CorsLayer};

use crate::runtime_config::RuntimeConfig;
use crate::security::{hash_password, LoginRateLimiter};
use crate::settings::{
    ensure_config, Overrides, Settings, API_PREFIX, API_VERSION, APP_NAME,
};
use crate::state::AppState;

#[derive(Debug, Parser)]
#[command(name = "music-server", version, about = "个人音乐库后端")]
struct Cli {
    /// 监听地址，默认 127.0.0.1
    #[arg(long)]
    host: Option<String>,
    /// 监听端口，默认取 config.json 的 backend_port（否则 8000）
    #[arg(long)]
    port: Option<u16>,
    /// 配置文件与 data/ 所在目录，默认当前工作目录
    #[arg(long)]
    base_dir: Option<PathBuf>,
    /// 数据目录，默认 <base-dir>/data
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// 调试模式：输出访问日志
    #[arg(long)]
    debug: bool,
    /// 关闭 /docs 说明页
    #[arg(long)]
    no_docs: bool,
    /// 仅管理员可创建艺人/专辑
    #[arg(long)]
    admin_only_create: bool,
    /// 只初始化数据库与 root 账户后退出
    #[arg(long)]
    init_only: bool,
}

fn main() {
    #[cfg(windows)]
    unsafe {
        // 让终端按 UTF-8 输出中文日志
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
        windows_sys::Win32::System::Console::SetConsoleCP(65001);
    }

    if let Err(err) = run() {
        eprintln!("启动失败：{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let settings = Settings::load(Overrides {
        base_dir: cli.base_dir.clone(),
        data_dir: cli.data_dir.clone(),
        host: cli.host.clone(),
        port: cli.port,
        debug: cli.debug,
        admin_only_create: cli.admin_only_create,
        enable_docs: !cli.no_docs,
    })?;
    init_tracing(settings.debug);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("无法创建运行时：{err}"))?;

    runtime.block_on(async move {
        ensure_config(&settings.base_dir, settings.port);
        let pool = db::init(&settings).await?;
        let account = ensure_root_account(&pool, &settings).await?;
        if let Some(account) = &account {
            println!("{}", root_notice(account));
            tracing::warn!("已创建 root 账户，密码文件：{}", account.file);
        }
        prune_refresh_tokens(&pool).await;

        tracing::info!(
            "服务启动：debug={}，数据库={}，媒体目录={}",
            settings.debug,
            settings.database_path.display(),
            settings.media_dir.display()
        );

        if cli.init_only {
            println!("初始化完成，未启动 HTTP 服务。");
            return Ok(());
        }

        serve(pool, settings).await
    })
}

fn init_tracing(debug: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if debug {
            tracing_subscriber::EnvFilter::new("info,music_server=debug")
        } else {
            tracing_subscriber::EnvFilter::new("info")
        }
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

struct RootAccount {
    username: String,
    password: String,
    file: String,
}

fn generate_password(length: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    loop {
        let password: String = (0..length)
            .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
            .collect();
        let lower = password.chars().any(|c| c.is_ascii_lowercase());
        let upper = password.chars().any(|c| c.is_ascii_uppercase());
        let digit = password.chars().any(|c| c.is_ascii_digit());
        if lower && upper && digit {
            return password;
        }
    }
}

async fn ensure_root_account(
    pool: &SqlitePool,
    settings: &Settings,
) -> Result<Option<RootAccount>, String> {
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(|err| format!("读取账户失败：{err}"))?;
    if users > 0 {
        return Ok(None);
    }

    let password = generate_password(20);
    let secret = password.clone();
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&secret))
        .await
        .map_err(|err| format!("密码处理失败：{err}"))?;
    let now = timeutil::now_text();
    sqlx::query(
        "INSERT INTO users (uuid, username, password_hash, nickname, bio, role, created_at, updated_at)
         VALUES (?, 'root', ?, 'root', '超级管理员（仅限本机登录）', 'root', ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().simple().to_string())
    .bind(&password_hash)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|err| format!("创建 root 账户失败：{err}"))?;

    let file = settings.data_dir.join("root_password.txt");
    let content = format!(
        "{APP_NAME} root 账户（首次启动自动生成）\n\
         用户名: root\n\
         密码: {password}\n\
         创建时间: {}\n\n\
         说明：\n\
         1. root 账户仅允许从本机（127.0.0.1 / ::1）登录；\n\
         2. 登录后请尽快修改密码，并删除本文件；\n\
         3. 若忘记密码，可删除数据库后重新初始化。\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    std::fs::write(&file, content).map_err(|err| format!("写入密码文件失败：{err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }

    Ok(Some(RootAccount {
        username: "root".to_string(),
        password,
        file: file.display().to_string(),
    }))
}

fn root_notice(account: &RootAccount) -> String {
    let line = "=".repeat(64);
    format!(
        "\n{line}\n 首次启动：{APP_NAME} 已创建 root 超级管理员\n\
           用户名: {}\n   密码  : {}\n   密码文件: {}\n   root 仅允许从本机登录，登录后请尽快修改密码并删除该文件\n{line}\n",
        account.username, account.password, account.file
    )
}

async fn prune_refresh_tokens(pool: &SqlitePool) {
    let cutoff = timeutil::now_plus_seconds(-30 * 24 * 3600);
    match sqlx::query(
        "DELETE FROM refresh_tokens WHERE expires_at < ? OR (revoked_at IS NOT NULL AND revoked_at < ?)",
    )
    .bind(&cutoff)
    .bind(&cutoff)
    .execute(pool)
    .await
    {
        Ok(result) if result.rows_affected() > 0 => {
            tracing::info!("已清理过期刷新令牌 {} 条", result.rows_affected())
        }
        Ok(_) => {}
        Err(err) => tracing::error!("清理刷新令牌失败：{err}"),
    }
}

fn cors_layer(settings: &Settings) -> CorsLayer {
    let exposed = [HeaderName::from_static("x-request-id")];
    if settings.cors_origins.iter().any(|origin| origin == "*") {
        return CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
            .expose_headers(exposed);
    }
    let origins: Vec<HeaderValue> = settings
        .cors_origins
        .iter()
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(Any)
        .allow_headers(Any)
        .expose_headers(exposed)
}

async fn request_context(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let started = Instant::now();

    let mut response = next.run(request).await;

    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    let headers = response.headers_mut();
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "SAMEORIGIN"),
        ("referrer-policy", "no-referrer"),
    ] {
        if !headers.contains_key(name) {
            headers.insert(name, HeaderValue::from_static(value));
        }
    }

    if state.settings.debug {
        tracing::debug!(
            "{} {} -> {} ({:.1} ms) request_id={}",
            method,
            path,
            response.status().as_u16(),
            started.elapsed().as_secs_f64() * 1000.0,
            request_id
        );
    }
    response
}

async fn docs_page() -> impl IntoResponse {
    let body = format!(
        "<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\">\
         <title>{APP_NAME} API</title>\
         <body style=\"font-family:system-ui;max-width:720px;margin:40px auto;line-height:1.7\">\
         <h1>{APP_NAME} {API_VERSION}</h1>\
         <p>基础路径：<code>{API_PREFIX}</code>；统一响应：<code>{{\"code\":0,\"message\":\"ok\",\"data\":…}}</code>。</p>\
         <p>认证：<code>Authorization: Bearer &lt;access_token&gt;</code>。</p>\
         <p>完整接口契约见仓库 <code>docs/API.md</code>。</p>\
         </body></html>"
    );
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
}

async fn serve(pool: SqlitePool, settings: Settings) -> Result<(), String> {
    let settings = Arc::new(settings);
    let state = AppState {
        db: pool,
        runtime: Arc::new(RuntimeConfig::new(settings.config_path())),
        limiter: Arc::new(LoginRateLimiter::new(
            settings.login_max_attempts,
            settings.login_window_seconds,
            settings.lock_seconds,
        )),
        settings: settings.clone(),
    };

    let mut router = Router::new()
        .nest(API_PREFIX, routes::api_router())
        .merge(routes::media::router())
        .merge(routes::health::router());
    if settings.enable_docs {
        router = router.route("/docs", get(docs_page));
    }

    let app = router
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(middleware::from_fn_with_state(state.clone(), request_context))
        .layer(cors_layer(&settings))
        .with_state(state);

    let address = format!("{}:{}", settings.host, settings.port);
    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .map_err(|err| format!("无法监听 {address}：{err}"))?;

    println!(
        "{APP_NAME} {API_VERSION} 已启动：http://{}:{}{}",
        display_host(&settings.host),
        settings.port,
        API_PREFIX
    );
    println!("数据库：{}", settings.database_path.display());
    println!("媒体目录：{}", settings.media_dir.display());
    println!("健康检查：http://{}:{}/health", display_host(&settings.host), settings.port);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|err| format!("服务异常退出：{err}"))?;

    tracing::info!("服务已停止");
    Ok(())
}

fn display_host(host: &str) -> String {
    match host {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "[::1]".to_string(),
        value => value.to_string(),
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("\n正在停止服务…");
}

async fn not_found(method: Method, uri: axum::http::Uri) -> Response {
    error::AppError::not_found(format!("接口不存在：{method} {}", uri.path())).into_response()
}

async fn method_not_allowed(method: Method, uri: axum::http::Uri) -> Response {
    error::AppError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        format!("不支持的方法：{method} {}", uri.path()),
    )
    .into_response()
}
