//! 校园点位：POST /api/v560/get/1/distance/1（sportType=4）。
//!
//! runec = 信封(f"{uid}{经度6位}{纬度6位}{起始时间秒级ms整}"，insert→observed 序列化)；
//! sign = MD5(http版URL + 盐)。带 300s 缓存（限流 10603：5 分钟 3 次），失败回退最近缓存。

use super::client::ApiClient;
use super::model;
use crate::crypto::envelope::{build_envelope, OuterOrder};
use crate::crypto::sign::md5_url_sign;
use crate::location::Coordinate;
use serde_json::{json, Value};

pub const POINTS_PATH: &str = "/api/v560/get/1/distance/1";

#[derive(Clone, Debug)]
pub struct PointsContext {
    pub points: Vec<Value>,
    pub area: crate::track::wire::RunAreaMeta,
}

/// 经纬度六位小数字符串。
fn six_digit(v: f64) -> String {
    format!("{v:.6}")
}

/// 拉取实时点位（带缓存回退）。
pub fn fetch_points(
    client: &mut ApiClient,
    anchor: Coordinate,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<Value>, String> {
    Ok(fetch_points_context(client, anchor, log)?.points)
}

pub fn fetch_points_context(
    client: &mut ApiClient,
    anchor: Coordinate,
    log: &mut dyn FnMut(&str),
) -> Result<PointsContext, String> {
    fetch_points_context_ext(client, anchor, None, log)
}

/// run_area_id：学校配置了区域时 App 会附带；未配置则不传（与 App 一致）。
pub fn fetch_points_ext(
    client: &mut ApiClient,
    anchor: Coordinate,
    run_area_id: Option<String>,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<Value>, String> {
    Ok(fetch_points_context_ext(client, anchor, run_area_id, log)?.points)
}

pub fn fetch_points_context_ext(
    client: &mut ApiClient,
    anchor: Coordinate,
    run_area_id: Option<String>,
    log: &mut dyn FnMut(&str),
) -> Result<PointsContext, String> {
    anchor.validate()?;
    // ① TTL 内命中缓存直接返回
    if let Some((ts, pts, area)) = model::load_points_cache_context_for(anchor) {
        if !pts.is_empty()
            && crate::crypto::envelope::now_ms() - ts < model::POINTS_TTL_MS
        {
            log(&format!("[points] 缓存命中（{} 秒前，{} 点）", (crate::crypto::envelope::now_ms() - ts) / 1000, pts.len()));
            return Ok(PointsContext { points: pts, area });
        }
    }
    // ② 请求接口
    let uid = client.login.as_ref().map(|s| s.uid).unwrap_or(0);
    let unid = client
        .login
        .as_ref()
        .map(|s| s.unid.clone())
        .unwrap_or_else(|| "0".into());
    let lat = anchor.latitude;
    let lon = anchor.longitude;
    let url = format!("{}{}", model::HOST, POINTS_PATH);

    let start_ms = crate::crypto::envelope::now_ms();
    let runec_input = format!("{uid}{}{}{}", six_digit(lon), six_digit(lat), (start_ms / 1000) * 1000);
    let runec_env = build_envelope(&mut client.session, &runec_input, OuterOrder::Observed);
    let runec = runec_env.json;

    let mut body = json!({
        "sportType": 4,
        "longitude": lon,
        "latitude": lat,
        "sign": md5_url_sign(&url),
        "uuid": uuid::Uuid::new_v4().to_string(),
        "selectedUnid": unid,
        "runec": runec,
    });
    if let Some(area) = run_area_id {
        body["runAreaId"] = json!(area);
    }
    let body = body.to_string();

    let out = client.envelope_request("POST", &url, &body, crate::crypto::header::UA_IOS, &[])?;
    let fallback = |log: &mut dyn FnMut(&str)| -> Result<PointsContext, String> {
        if let Some((_ts, pts, area)) = model::load_points_cache_context_for(anchor) {
            if !pts.is_empty() {
                log("[points] 接口失败，回退最近一次接口结果缓存");
                return Ok(PointsContext { points: pts, area });
            }
        }
        Err("点位接口失败且无缓存".into())
    };
    let Some(dec) = out.decrypted else {
        return fallback(log);
    };
    let payload = &dec.business;
    // pointsResModels：优先顶层，其次 data 子对象
    let pts: Vec<Value> = super::client::get_field(payload, "pointsResModels")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    if !pts.is_empty() {
        let area = area_from_payload(payload, &pts);
        let _ = model::save_points_cache_context(anchor, &pts, &area);
        return Ok(PointsContext { points: pts, area });
    }
    let err = payload.get("error").and_then(|e| e.as_i64()).unwrap_or(0);
    log(&format!(
        "[points] 接口无点位 error={err}: {}",
        payload.get("message").and_then(|m| m.as_str()).unwrap_or("")
    ));
    fallback(log)
}

fn area_from_payload(payload: &Value, points: &[Value]) -> crate::track::wire::RunAreaMeta {
    let first_point = points.first().unwrap_or(&Value::Null);
    let lookup = |names: &[&str]| -> Option<&Value> {
        names.iter()
            .find_map(|name| super::client::get_field(payload, name))
            .or_else(|| names.iter().find_map(|name| first_point.get(*name)))
    };
    let run_area_id = lookup(&["runAreaId", "runAreaID", "areaId"])
        .and_then(value_as_i64)
        .unwrap_or(-1);
    let geo_fences_json = lookup(&["geoFencesJson", "geoFences", "geoFenceJson", "fences"])
        .map(value_as_json_string)
        .filter(|value| !value.trim().is_empty() && value.trim() != "null")
        .unwrap_or_else(|| "[]".into());
    let freedom_show_fence = lookup(&["freedomShowFence", "showFence"])
        .and_then(Value::as_bool)
        .unwrap_or(run_area_id >= 0 && geo_fences_json.trim() != "[]");
    crate::track::wire::RunAreaMeta {
        run_area_id,
        geo_fences_json,
        freedom_show_fence,
    }
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value.as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn value_as_json_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "[]".into(),
        other => other.to_string(),
    }
}

/// 点位中心（BD 系）。
#[allow(dead_code)]
pub fn center_bd(points: &[Value]) -> (f64, f64) {
    let n = points.len().max(1) as f64;
    let lat = points.iter().filter_map(|p| p.get("lat").and_then(|v| v.as_f64())).sum::<f64>() / n;
    let lon = points.iter().filter_map(|p| p.get("lon").and_then(|v| v.as_f64())).sum::<f64>() / n;
    (lat, lon)
}

/// 点位 → (lat, lon) BD 系数组（轨迹输入）。
pub fn points_bd(points: &[Value]) -> Vec<(f64, f64)> {
    points
        .iter()
        .filter_map(|p| {
            let lat = p.get("lat")?.as_f64()?;
            let lon = p.get("lon")?.as_f64()?;
            Some((lat, lon))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_metadata_accepts_top_level_and_string_values() {
        let payload = json!({
            "runAreaId": "42",
            "geoFencesJson": [{"lat": 1.0, "lon": 2.0}],
            "freedomShowFence": true,
        });
        let area = area_from_payload(&payload, &[]);
        assert_eq!(area.run_area_id, 42);
        assert_eq!(area.geo_fences_json, "[{\"lat\":1.0,\"lon\":2.0}]");
        assert!(area.freedom_show_fence);
    }

    #[test]
    fn area_metadata_falls_back_to_point_fields() {
        let points = vec![json!({"runAreaId": 7, "geoFences": "[]"})];
        let area = area_from_payload(&Value::Null, &points);
        assert_eq!(area.run_area_id, 7);
        assert!(!area.freedom_show_fence);
    }
}