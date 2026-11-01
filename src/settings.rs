//! 进程级配置：命令行 > 环境变量 > 根目录 config.json > 默认值。
//!
//! 只读一次：端口与数据目录改完必须重启；业务开关见 runtime_config。
//! config.json 中缺失的键会在启动时按默认值补齐写回，避免用户看不到可选项。

use std::{
    fs,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde_json::{Map, Value};

pub const APP_NAME: &str = "Guobeng Music Server";
pub const API_PREFIX: &str = "/api/v1";
pub const API_VERSION: &str = "2.0.0";

pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus"];

pub const MAX_IMAGE_SIZE: u64 = 10 * 1024 * 1024;
pub const MAX_AUDIO_SIZE: u64 = 1024 * 1024 * 1024;

const DEFAULT_PORT: u16 = 8000;
const DEFAULT_HOST: &str = "127.0.0.1";

#[derive(Debug, Default)]
pub struct Overrides {
    pub base_dir: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub debug: bool,
    pub admin_only_create: bool,
    pub enable_docs: bool,
}

#[derive(Debug)]
pub struct Settings {
    pub debug: bool,
    pub host: String,
    pub port: u16,
    pub base_dir: PathBuf,
    pub data_dir: PathBuf,
    pub media_dir: PathBuf,
    pub database_path: PathBuf,
    pub secret_key: String,
    pub access_token_ttl: i64,
    pub refresh_token_ttl: i64,
    pub admin_only_create: bool,
    pub enable_docs: bool,
    pub cors_origins: Vec<String>,
    pub login_max_attempts: usize,
    pub login_window_seconds: i64,
    pub lock_seconds: i64,
    pub password_min_length: usize,
}

impl Settings {
    pub fn load(overrides: Overrides) -> Result<Self, String> {
        let base_dir = match overrides.base_dir {
            Some(dir) => dir,
            None => std::env::current_dir().map_err(|err| format!("无法获取当前目录：{err}"))?,
        };
        let base_dir = absolutize(&base_dir);

        let data_dir = match overrides.data_dir.or_else(|| env_path("MUSIC_SERVER_DATA_DIR")) {
            Some(dir) if dir.is_absolute() => dir,
            Some(dir) => base_dir.join(dir),
            None => base_dir.join("data"),
        };

        fs::create_dir_all(&data_dir)
            .map_err(|err| format!("无法创建数据目录 {}：{err}", data_dir.display()))?;
        let media_dir = data_dir.join("media");
        for sub in ["covers", "avatars", "audio"] {
            fs::create_dir_all(media_dir.join(sub))
                .map_err(|err| format!("无法创建媒体目录：{err}"))?;
        }

        let config = read_config_object(&base_dir.join("config.json"));
        let port = resolve_port(overrides.port, &config);

        let secret_key = load_or_create_secret(&data_dir)?;

        let host = overrides
            .host
            .or_else(|| env_string("MUSIC_SERVER_HOST"))
            .or_else(|| config_string(&config, "host"))
            .unwrap_or_else(|| DEFAULT_HOST.to_string());

        let debug = overrides.debug || env_flag("MUSIC_SERVER_DEBUG");
        let admin_only_create = overrides.admin_only_create
            || env_flag("MUSIC_SERVER_ADMIN_ONLY_CREATE");
        let enable_docs = if overrides.enable_docs {
            env_flag_default("MUSIC_SERVER_ENABLE_DOCS", true)
        } else {
            false
        };
        let cors_origins = env_string("MUSIC_SERVER_CORS_ORIGINS")
            .map(|raw| {
                raw.split(',')
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|list| !list.is_empty())
            .unwrap_or_else(|| vec!["*".to_string()]);

        Ok(Self {
            debug,
            host,
            port,
            base_dir,
            database_path: data_dir.join("music.db"),
            media_dir,
            data_dir,
            secret_key,
            access_token_ttl: 30 * 60,
            refresh_token_ttl: 7 * 24 * 3600,
            admin_only_create,
            enable_docs,
            cors_origins,
            login_max_attempts: 8,
            login_window_seconds: 300,
            lock_seconds: 300,
            password_min_length: 8,
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.base_dir.join("config.json")
    }
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_flag(name: &str) -> bool {
    env_flag_default(name, false)
}

fn env_flag_default(name: &str, default: bool) -> bool {
    match env_string(name) {
        Some(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env_string(name).map(PathBuf::from)
}

/// 端口优先级：命令行 > MUSIC_SERVER_PORT > config.json backend_port > 8000。
fn resolve_port(cli_port: Option<u16>, config: &Map<String, Value>) -> u16 {
    if let Some(port) = cli_port {
        return port;
    }
    if let Some(raw) = env_string("MUSIC_SERVER_PORT") {
        match raw.parse::<u16>() {
            Ok(port) if port > 0 => return port,
            _ => tracing::error!("环境变量 MUSIC_SERVER_PORT 不是合法端口，已忽略：{raw}"),
        }
    }
    match config.get("backend_port") {
        Some(value) => port_from_value(value, "backend_port").unwrap_or(DEFAULT_PORT),
        None => DEFAULT_PORT,
    }
}

/// 配置文件里的端口值：整数或纯数字字符串，范围 1-65535；非法值记日志后按缺失处理。
fn port_from_value(value: &Value, key: &str) -> Option<u16> {
    let parsed = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    };
    match parsed {
        Some(port) if (1..=65535).contains(&port) => Some(port as u16),
        _ => {
            tracing::error!("配置项 {key} 的端口必须是 1-65535 的整数，已忽略：{value}");
            None
        }
    }
}

/// 配置文件里的字符串项；空白串与非法类型按缺失处理。
fn config_string(config: &Map<String, Value>, key: &str) -> Option<String> {
    match config.get(key) {
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(Value::Null) | None => None,
        Some(other) => {
            tracing::error!("配置项 {key} 必须是字符串，已忽略：{other}");
            None
        }
    }
}

pub fn read_config_object(path: &Path) -> Map<String, Value> {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                tracing::error!("配置文件顶层必须是对象，已退回默认值：{}", path.display());
                Map::new()
            }
            Err(err) => {
                tracing::error!("配置文件解析失败，已退回默认值：{}：{err}", path.display());
                Map::new()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(err) => {
            tracing::error!("配置文件读取失败，已退回默认值：{}：{err}", path.display());
            Map::new()
        }
    }
}

/// 确保唯一配置文件存在且含全部可配置项；已有值一律不动。
///
/// 写入的是**默认值**而不是本次启动的实际值：命令行临时覆盖不该被固化进文件，
/// 否则一次 `--port 9000` 会永久改掉默认端口。
pub fn ensure_config(base_dir: &Path) {
    let path = base_dir.join("config.json");
    let existed = path.exists();
    let mut map = read_config_object(&path);
    let before = map.clone();

    for key in ["allow_registration", "allow_user_audio_upload"] {
        if !map.contains_key(key) {
            map.insert(key.to_string(), Value::Bool(true));
        }
    }
    if !map.contains_key("backend_port") {
        map.insert("backend_port".to_string(), Value::from(DEFAULT_PORT));
    }
    if !map.contains_key("host") {
        map.insert("host".to_string(), Value::from(DEFAULT_HOST));
    }

    if existed && before == map {
        return;
    }
    match serde_json::to_string_pretty(&Value::Object(map)) {
        Ok(text) => {
            if let Err(err) = fs::write(&path, text + "\n") {
                tracing::error!("配置文件写入失败：{}：{err}", path.display());
            }
        }
        Err(err) => tracing::error!("配置文件序列化失败：{err}"),
    }
}

/// 密钥持久化到 data/secret.key；环境变量优先。
fn load_or_create_secret(data_dir: &Path) -> Result<String, String> {
    if let Some(key) = env_string("MUSIC_SERVER_SECRET_KEY") {
        return Ok(key);
    }
    let path = data_dir.join("secret.key");
    if let Ok(text) = fs::read_to_string(&path) {
        let key = text.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }
    let mut raw = [0u8; 48];
    rand::thread_rng().fill_bytes(&mut raw);
    let key = URL_SAFE_NO_PAD.encode(raw);
    fs::write(&path, &key).map_err(|err| format!("无法写入密钥文件：{err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}
