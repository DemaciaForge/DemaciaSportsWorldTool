//! 日志文本处理：着色前缀剥离与关键词归类，GUI/CLI 共用。

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

/// 只取展示文本（CLI 输出用）。
pub fn clean(t: &str) -> &str {
    classify(t).1
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
    // 登录会话、GT4 验证及常见鉴权响应中的凭证字段。
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
            assert!(out.chars().count() <= n.max(0));
        }
        assert_eq!(truncate("中文abc", 4), "中文ab");
    }
}
