//! 日志文本处理：着色前缀剥离、敏感字段脱敏与关键词归类，GUI/CLI 共用。

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogKind {
    Plain,
    Ok,
    Err,
    Warn,
}

/// 归类并剥离前缀：入站 √/×/⚠ 仅用于着色，不进入展示文本。
pub fn classify(t: &str) -> (LogKind, &str) {
    if let Some(rest) = t.strip_prefix('×') {
        return (LogKind::Err, rest.trim_start());
    }
    if let Some(rest) = t.strip_prefix('⚠') {
        return (LogKind::Warn, rest.trim_start());
    }
    if let Some(rest) = t.strip_prefix('√') {
        return (LogKind::Ok, rest.trim_start());
    }
    if t.contains("失败") || t.contains("错误") {
        return (LogKind::Err, t);
    }
    if t.contains("成功") {
        return (LogKind::Ok, t);
    }
    (LogKind::Plain, t)
}

const SENSITIVE_KEYS: &[&str] = &[
    "token",
    "password",
    "passToken",
    "access_token",
    "refresh_token",
    "authorization",
    "secret",
];

fn boundary(byte: Option<u8>) -> bool {
    !matches!(byte, Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_'))
}

/// Redact key/value pairs in non-JSON diagnostics such as `token=... uid=...`.
fn redact_pairs(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pos = 0;
    while pos < input.len() {
        let mut matched = None;
        for key in SENSITIVE_KEYS {
            let Some(candidate) = input.get(pos..pos + key.len()) else {
                continue;
            };
            if !candidate.eq_ignore_ascii_case(key)
                || !boundary(
                    pos.checked_sub(1)
                        .and_then(|i| input.as_bytes().get(i).copied()),
                )
                || !boundary(input.as_bytes().get(pos + key.len()).copied())
            {
                continue;
            }
            let mut delimiter = pos + key.len();
            if matches!(input.as_bytes().get(delimiter), Some(b'"' | b'\'')) {
                delimiter += 1;
            }
            while delimiter < input.len() && input.as_bytes()[delimiter].is_ascii_whitespace() {
                delimiter += 1;
            }
            if delimiter >= input.len() || !matches!(input.as_bytes()[delimiter], b':' | b'=') {
                continue;
            }
            let mut value_start = delimiter + 1;
            while value_start < input.len() && input.as_bytes()[value_start].is_ascii_whitespace() {
                value_start += 1;
            }
            matched = Some(value_start);
            break;
        }

        let Some(value_start) = matched else {
            let ch = input[pos..].chars().next().expect("valid UTF-8");
            out.push(ch);
            pos += ch.len_utf8();
            continue;
        };

        out.push_str(&input[pos..value_start]);
        if let Some(&quote @ (b'"' | b'\'')) = input.as_bytes().get(value_start) {
            out.push(quote as char);
            let mut end = value_start + 1;
            let mut escaped = false;
            while end < input.len() {
                let byte = input.as_bytes()[end];
                if byte == quote && !escaped {
                    break;
                }
                escaped = byte == b'\\' && !escaped;
                if byte != b'\\' {
                    escaped = false;
                }
                end += 1;
            }
            out.push_str("***");
            if end < input.len() {
                out.push(quote as char);
                pos = end + 1;
            } else {
                pos = input.len();
            }
        } else {
            let mut end = value_start;
            while end < input.len()
                && !matches!(
                    input.as_bytes()[end],
                    b',' | b';' | b'&' | b' ' | b'\t' | b'\r' | b'\n' | b'}' | b']'
                )
            {
                end += 1;
            }
            out.push_str("***");
            pos = end;
        }
    }
    out
}

/// Redact credentials and device identifiers from JSON or key/value diagnostics.
pub fn redact_text(t: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(t) {
        return serde_json::to_string(&redact_json(&value)).unwrap_or_else(|_| "***".into());
    }
    redact_pairs(t)
}

/// 只取展示文本（CLI 输出用），同时确保不会泄露敏感字段。
pub fn clean(t: &str) -> String {
    redact_text(classify(t).1)
}

/// 按字符截断（字节切片会劈开多字节字符导致 panic）。
pub fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 敏感值保留前后各 4 个字符，中间打码；不超过 8 个字符时全部掩码。
pub fn mask(v: &str) -> String {
    let chars: Vec<char> = v.chars().collect();
    if chars.len() <= 8 {
        return "****".into();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}****{tail}")
}

/// 递归掩码敏感键的字符串值，键名忽略大小写；保留原业务数据。
pub fn redact_json(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    const SENSITIVE_KEYS: &[&str] = &[
        "token",
        "password",
        "passToken",
        "access_token",
        "refresh_token",
        "authorization",
        "secret",
    ];
    match v {
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(key, value)| {
                    let redacted = match value.as_str() {
                        Some(s) if SENSITIVE_KEYS.iter().any(|k| key.eq_ignore_ascii_case(k)) => {
                            Value::String(mask(s))
                        }
                        _ => redact_json(value),
                    };
                    (key.clone(), redacted)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        _ => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_mask_long_and_multibyte() {
        assert_eq!(mask("abcdefghijkl"), "abcd****ijkl");
        assert_eq!(mask("123456789"), "1234****6789");
        assert_eq!(mask("中文字符测试多字节"), "中文字符****试多字节");
        assert_eq!(mask("🦀🐱😀🎉abcdef"), "🦀🐱😀🎉****cdef");
    }

    #[test]
    fn test_mask_short_and_empty() {
        for value in ["", "a", "1234567", "12345678", "中文字符测试"] {
            assert_eq!(mask(value), "****");
        }
    }

    #[test]
    fn test_redact_json_sensitive_keys_case_insensitive() {
        for key in [
            "Token",
            "PASSWORD",
            "PassToken",
            "ACCESS_TOKEN",
            "refresh_token",
            "Authorization",
            "secret",
        ] {
            let value = json!({(key): "abcdefghijkl"});
            assert_eq!(redact_json(&value), json!({(key): "abcd****ijkl"}));
            assert_eq!(value[key], "abcdefghijkl");
        }
    }

    #[test]
    fn test_redact_json_nested_objects_and_arrays() {
        let value = json!({"data": {"token": "short", "items": [
            {"password": "abcdefghijkl"}, [{"secret": "12345678"}]],
            "authorization": {"token": "short"}}});
        let expected = json!({"data": {"token": "****", "items": [
            {"password": "abcd****ijkl"}, [{"secret": "****"}]],
            "authorization": {"token": "****"}}});
        assert_eq!(redact_json(&value), expected);
    }

    #[test]
    fn test_redact_json_preserves_other_values() {
        let value = json!({"message": "成功", "uid": 123, "token": 42,
            "password": null, "secret": false, "items": ["plain", 7, null, true]});
        assert_eq!(redact_json(&value), value);
        for value in [json!("plain"), json!(42), json!(null), json!(true)] {
            assert_eq!(redact_json(&value), value);
        }
    }

    /// 多字节字符中间截断不允许 panic。
    #[test]
    fn test_truncate_multibyte() {
        let s = "中文字符串测试";
        for n in 0..=s.len() + 4 {
            let out = truncate(s, n);
            assert!(out.chars().count() <= n);
        }
        assert_eq!(truncate("中文abc", 4), "中文ab");
    }

    #[test]
    fn clean_removes_prefix_and_redacts() {
        assert_eq!(clean("√ token=secret"), "token=***");
    }
}
