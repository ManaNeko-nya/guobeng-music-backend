//! 时间处理：数据库统一存 UTC，文本格式固定为 `YYYY-MM-DD HH:MM:SS.ffffff`，
//! 这样按字符串排序即按时间排序；对外输出转成 ISO 8601（T 分隔，微秒为 0 时省略）。

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

pub const STORE_FMT: &str = "%Y-%m-%d %H:%M:%S%.6f";

pub fn now_text() -> String {
    Utc::now().naive_utc().format(STORE_FMT).to_string()
}

pub fn now_plus_seconds(seconds: i64) -> String {
    let target = Utc::now() + chrono::Duration::seconds(seconds);
    target.naive_utc().format(STORE_FMT).to_string()
}

/// 数据库文本 -> API 输出格式。
pub fn to_iso(stored: &str) -> String {
    let mut text = stored.replace(' ', "T");
    if text.ends_with(".000000") {
        text.truncate(text.len() - 7);
    }
    text
}

pub fn opt_iso(stored: &Option<String>) -> Option<String> {
    stored.as_deref().map(to_iso)
}

/// 接受 RFC3339（含时区/Z）或本地 naive 时间，统一换成 UTC 存储格式。
pub fn normalize_input(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(datetime) = DateTime::parse_from_rfc3339(text) {
        return Some(datetime.with_timezone(&Utc).naive_utc().format(STORE_FMT).to_string());
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, fmt) {
            return Some(naive.format(STORE_FMT).to_string());
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return date.and_hms_opt(0, 0, 0).map(|naive| naive.format(STORE_FMT).to_string());
    }
    None
}
