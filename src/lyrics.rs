//! LRC 校验与规范化：只接受带时间戳的歌词（或元数据行）。

use crate::error::{ApiResult, AppError};

const MAX_LYRICS_CHARS: usize = 200_000;
const MAX_LYRICS_LINES: usize = 10_000;

const META_KEYS: &[&str] = &["ti", "ar", "al", "by", "offset", "re", "ve", "au", "length", "tool"];

pub fn normalize_lrc(text: &str) -> ApiResult<String> {
    if text.trim().is_empty() {
        return Err(AppError::bad_request("歌词不能为空"));
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let normalized = normalized.trim().to_string();
    if normalized.chars().count() > MAX_LYRICS_CHARS {
        return Err(AppError::bad_request(format!(
            "歌词过长（最多 {MAX_LYRICS_CHARS} 个字符）"
        )));
    }

    let mut cleaned: Vec<String> = Vec::new();
    let mut timed_lines = 0usize;
    for raw in normalized.split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            cleaned.push(String::new());
            continue;
        }
        let stamps = count_timestamps(line);
        if stamps > 0 {
            timed_lines += stamps;
            cleaned.push(line.to_string());
            continue;
        }
        if is_meta_line(line) {
            cleaned.push(line.to_string());
            continue;
        }
        return Err(AppError::bad_request(
            "歌词格式错误：每一行都需要 [mm:ss.xx] 时间戳",
        ));
    }

    if cleaned.len() > MAX_LYRICS_LINES {
        return Err(AppError::bad_request(format!(
            "歌词行数过多（最多 {MAX_LYRICS_LINES} 行）"
        )));
    }
    if timed_lines == 0 {
        return Err(AppError::bad_request("歌词至少需要一行带时间戳的内容"));
    }
    Ok(cleaned.join("\n"))
}

fn count_timestamps(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut count = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'[' {
            index += 1;
            continue;
        }
        let Some(close) = line[index + 1..].find(']').map(|offset| index + 1 + offset) else {
            break;
        };
        if is_timestamp(&line[index + 1..close]) {
            count += 1;
        }
        index = close + 1;
    }
    count
}

fn is_timestamp(inner: &str) -> bool {
    let Some(colon) = inner.find(':') else { return false };
    let minutes = &inner[..colon];
    if minutes.is_empty() || minutes.len() > 3 || !minutes.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let rest = &inner[colon + 1..];
    let (seconds, fraction) = match rest.find(['.', ':']) {
        Some(split) => (&rest[..split], Some(&rest[split + 1..])),
        None => (rest, None),
    };
    if seconds.is_empty() || seconds.len() > 2 || !seconds.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match seconds.parse::<u32>() {
        Ok(value) if value <= 59 => {}
        _ => return false,
    }
    if let Some(fraction) = fraction {
        if fraction.is_empty() || fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return false;
        }
    }
    true
}

fn is_meta_line(line: &str) -> bool {
    if !line.starts_with('[') || !line.ends_with(']') || line.len() < 4 {
        return false;
    }
    let inner = &line[1..line.len() - 1];
    if inner.contains(']') {
        return false;
    }
    let Some(colon) = inner.find(':') else { return false };
    let key = inner[..colon].to_ascii_lowercase();
    META_KEYS.contains(&key.as_str())
}
