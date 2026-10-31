//! 运行时可编辑的业务开关（config.json）。
//!
//! 每次请求按文件 mtime 判断是否需要重读，改完立即生效，无需重启。

use std::{
    fs,
    path::PathBuf,
    sync::RwLock,
    time::SystemTime,
};

use serde_json::Value;

use crate::settings::read_config_object;

#[derive(Debug, Clone, Copy)]
pub struct Flags {
    pub allow_registration: bool,
    pub allow_user_audio_upload: bool,
}

impl Default for Flags {
    fn default() -> Self {
        Self { allow_registration: true, allow_user_audio_upload: true }
    }
}

pub struct RuntimeConfig {
    path: PathBuf,
    cache: RwLock<Cache>,
}

#[derive(Default)]
struct Cache {
    mtime: Option<SystemTime>,
    flags: Flags,
}

impl RuntimeConfig {
    pub fn new(path: PathBuf) -> Self {
        Self { path, cache: RwLock::new(Cache::default()) }
    }

    pub fn get(&self) -> Flags {
        let mtime = match fs::metadata(&self.path).and_then(|meta| meta.modified()) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::error!("配置文件状态读取失败 {}：{err}", self.path.display());
                return self.cache.read().map(|cache| cache.flags).unwrap_or_default();
            }
        };

        if let Ok(cache) = self.cache.read() {
            if cache.mtime.is_some() && cache.mtime == mtime {
                return cache.flags;
            }
        }

        let flags = parse_flags(&read_config_object(&self.path));
        if let Ok(mut cache) = self.cache.write() {
            cache.mtime = mtime;
            cache.flags = flags;
        }
        flags
    }
}

fn parse_flags(map: &serde_json::Map<String, Value>) -> Flags {
    let mut flags = Flags::default();
    for (key, target) in [
        ("allow_registration", &mut flags.allow_registration),
        ("allow_user_audio_upload", &mut flags.allow_user_audio_upload),
    ] {
        match map.get(key) {
            Some(Value::Bool(value)) => *target = *value,
            Some(Value::Null) | None => {}
            Some(other) => tracing::error!("配置项 {key} 必须是 true/false，已忽略：{other}"),
        }
    }
    flags
}
