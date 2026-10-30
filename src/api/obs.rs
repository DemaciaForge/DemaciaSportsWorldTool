//! OBS 上传：POST /api/obs/temporary/url 换签名 URL → PUT JSON。

use super::client::{get_field, parse_data_field, ApiClient};
use serde_json::{json, Value};

pub const OBS_SIGN_PATH: &str = "/api/obs/temporary/url";

/// 换取签名 URL。
///
/// `contentType` 必须是 **`text/plain`**：真机 OBS 对象是「文本形态的
/// gzip+base64 映射」，签名时若声明 `application/json`，签名 URL 的
/// Content-Type 会跟着写成 JSON，与实际内容不符。
pub fn sign_url(client: &mut ApiClient, method: &str, key: &str) -> Result<String, String> {
    let body = json!({
        "bucketName": "iydsj-hbase-hot",
        "objectKey": key,
        "method": method,
        "contentType": "text/plain",
    })
    .to_string();
    let biz = client.call("POST", OBS_SIGN_PATH, &body, &[])?;
    // business.data 可能是 JSON 字符串 {"signedUrl": ...}
    let data = parse_data_field(&biz);
    let signed = data
        .get("signedUrl")
        .and_then(|v| v.as_str())
        .or_else(|| get_field(&biz, "signedUrl").and_then(|v| v.as_str()))
        .ok_or("OBS 签名响应缺 signedUrl")?;
    Ok(signed.to_string())
}

/// PUT 上传 OBS 对象。
pub fn put_object(
    signed_url: &str,
    payload: &[u8],
    log: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let agent = super::client::make_agent();
    let resp = agent
        .put(signed_url)
        // 与签名时的 contentType 保持一致（真机是文本形态对象）。
        .set("Content-Type", "text/plain")
        .send_bytes(payload)
        .map_err(super::client::ureq_err)?;
    log(&format!(
        "[obs] PUT {} -> {}",
        shorten(signed_url),
        resp.status()
    ));
    Ok(())
}

/// 上传到给定的 objectKey 列表。
///
/// 真机只写 1 个 key（`track::wire::obs_keys` 已收敛为日期式单键），这里
/// 保留循环是为了让调用方仍能显式传入多键做对照实验。
pub fn upload_both_keys(
    client: &mut ApiClient,
    keys: &[String],
    payload: &[u8],
    log: &mut dyn FnMut(&str),
) -> usize {
    let mut ok = 0;
    for key in keys {
        match sign_url(client, "Put", key).and_then(|url| put_object(&url, payload, log)) {
            Ok(()) => ok += 1,
            Err(e) => log(&format!("[obs] PUT {key} 失败: {e}")),
        }
    }
    ok
}

fn shorten(url: &str) -> &str {
    // 只显示 objectKey 部分，避免日志过长
    let start = url.find("run_data").unwrap_or(0);
    let end = (start + 60).min(url.len());
    &url[start..end]
}

/// 回读验证（GET 签名）。
#[allow(dead_code)]
pub fn fetch_object(
    client: &mut ApiClient,
    key: &str,
    log: &mut dyn FnMut(&str),
) -> Result<Value, String> {
    let signed = sign_url(client, "Get", key)?;
    let agent = super::client::make_agent();
    let resp = agent.get(&signed).call().map_err(super::client::ureq_err)?;
    let text = resp.into_string().unwrap_or_default();
    log(&format!("[obs] GET 回读 {} 字节", text.len()));
    serde_json::from_str(&text).map_err(|e| format!("OBS 对象解析失败: {e}"))
}
