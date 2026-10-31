//! 安全：PBKDF2 密码哈希、JWT access token、opaque refresh token、登录限流。

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD_NO_PAD, engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PASSWORD_SCHEME: &str = "pbkdf2_sha256";
pub const PBKDF2_ITERATIONS: u32 = 600_000;
const SALT_BYTES: usize = 16;
const HASH_BYTES: usize = 32;

pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; SALT_BYTES];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut digest = [0u8; HASH_BYTES];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, PBKDF2_ITERATIONS, &mut digest);
    format!(
        "{PASSWORD_SCHEME}${PBKDF2_ITERATIONS}${}${}",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(digest)
    )
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let mut parts = stored.split('$');
    let (Some(scheme), Some(iterations), Some(salt), Some(expected)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if scheme != PASSWORD_SCHEME {
        return false;
    }
    let Ok(iterations) = iterations.parse::<u32>() else { return false };
    let (Ok(salt), Ok(expected)) = (
        STANDARD_NO_PAD.decode(salt),
        STANDARD_NO_PAD.decode(expected),
    ) else {
        return false;
    };
    if salt.is_empty() || expected.is_empty() {
        return false;
    }
    let mut actual = vec![0u8; expected.len()];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), &salt, iterations, &mut actual);
    constant_time_eq(&actual, &expected)
}

/// 等长比较，耗时与差异位置无关。
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

pub fn needs_rehash(stored: &str) -> bool {
    let mut parts = stored.split('$');
    match (parts.next(), parts.next()) {
        (Some(scheme), Some(iterations)) => {
            scheme != PASSWORD_SCHEME
                || iterations.parse::<u32>().map(|value| value < PBKDF2_ITERATIONS).unwrap_or(true)
        }
        _ => true,
    }
}

/// 账号不存在时也做一次等价耗时校验，避免按响应时间枚举用户名。
pub fn dummy_password_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| hash_password("timing-equalizer-not-a-real-password"))
}

pub fn validate_password_strength(password: &str, min_length: usize) -> Result<(), String> {
    let length = password.chars().count();
    if length < min_length {
        return Err(format!("密码长度至少 {min_length} 位"));
    }
    if length > 128 {
        return Err("密码长度不能超过 128 位".to_string());
    }
    if password.trim() != password {
        return Err("密码首尾不能包含空白字符".to_string());
    }
    let lower = password.chars().any(|c| c.is_lowercase());
    let upper = password.chars().any(|c| c.is_uppercase());
    let digit = password.chars().any(|c| c.is_ascii_digit());
    if [lower, upper, digit].iter().filter(|value| **value).count() < 2 {
        return Err("密码需至少包含字母、数字、大小写中的两类字符".to_string());
    }
    Ok(())
}

// ------------------------------- JWT ---------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub typ: String,
    pub iat: i64,
    pub nbf: i64,
    pub exp: i64,
    pub jti: String,
}

pub fn create_access_token(user_uuid: &str, secret: &str, ttl: i64) -> (String, i64) {
    let now = chrono::Utc::now().timestamp();
    let mut jti = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut jti);
    let claims = Claims {
        sub: user_uuid.to_string(),
        typ: "access".to_string(),
        iat: now,
        nbf: now,
        exp: now + ttl,
        jti: hex(&jti),
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap_or_default();
    (token, ttl)
}

pub fn decode_access_token(token: &str, secret: &str) -> Option<Claims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.leeway = 0;
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|err| tracing::debug!("JWT 校验失败：{err}"))
    .ok()?;
    if data.claims.typ != "access" {
        return None;
    }
    Some(data.claims)
}

// -------------------------- 刷新令牌（opaque） --------------------------

pub fn generate_refresh_token() -> String {
    let mut raw = [0u8; 48];
    rand::thread_rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

pub fn hash_refresh_token(token: &str) -> String {
    hex(Sha256::digest(token.as_bytes()).as_slice())
}

pub fn hex(raw: &[u8]) -> String {
    let mut text = String::with_capacity(raw.len() * 2);
    for byte in raw {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

// ------------------------------ 登录限流 ------------------------------

pub struct LoginRateLimiter {
    max_attempts: usize,
    window: Duration,
    lock_for: Duration,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    failures: HashMap<String, Vec<Instant>>,
    locked: HashMap<String, Instant>,
}

const MAX_KEYS: usize = 10_000;

impl LoginRateLimiter {
    pub fn new(max_attempts: usize, window_seconds: i64, lock_seconds: i64) -> Self {
        Self {
            max_attempts,
            window: Duration::from_secs(window_seconds.max(1) as u64),
            lock_for: Duration::from_secs(lock_seconds.max(1) as u64),
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn locked_seconds(&self, key: &str) -> i64 {
        let Ok(mut inner) = self.inner.lock() else { return 0 };
        let Some(until) = inner.locked.get(key).copied() else { return 0 };
        let now = Instant::now();
        if until <= now {
            inner.locked.remove(key);
            return 0;
        }
        (until - now).as_secs() as i64
    }

    pub fn register_failure(&self, key: &str) {
        let Ok(mut inner) = self.inner.lock() else { return };
        if inner.failures.len() >= MAX_KEYS {
            let now = Instant::now();
            let window = self.window;
            inner.failures.retain(|_, times| {
                times.retain(|time| now.duration_since(*time) < window);
                !times.is_empty()
            });
            inner.locked.retain(|_, until| *until > now);
        }
        let now = Instant::now();
        let times = inner.failures.entry(key.to_string()).or_default();
        times.push(now);
        let window = self.window;
        times.retain(|time| now.duration_since(*time) < window);
        if times.len() >= self.max_attempts {
            inner.locked.insert(key.to_string(), now + self.lock_for);
            inner.failures.remove(key);
            tracing::warn!("登录失败次数过多，已临时锁定 key={key}");
        }
    }

    pub fn register_success(&self, key: &str) {
        let Ok(mut inner) = self.inner.lock() else { return };
        inner.failures.remove(key);
        inner.locked.remove(key);
    }
}
