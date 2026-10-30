//! OBS 对象组装。
//!
//! 10 键对象，每值 gzip+base64；BD→GCJ 单次转换；30 键协议点集（与官方点结构
//! 对齐）；10s 窗（speed/step_freq，与提交体共用 [`ten_second_spans`]）；
//! 每 1000m 一圈；五点 fixed_point_json。
#![allow(non_snake_case)]

use chrono::{Local, TimeZone};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::io::Write;

use super::geom::round_to;
use super::model::{GenPoint, Track};

const X_PI: f64 = std::f64::consts::PI * 3000.0 / 180.0;

/// The run-area metadata returned by the point endpoint.
///
/// Older callers can keep using the default value (free-run semantics), while
/// campus runs pass through the server-provided area id and fence JSON so the
/// record detail page can draw the same green boundary as the official app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAreaMeta {
    pub run_area_id: i64,
    pub geo_fences_json: String,
    pub freedom_show_fence: bool,
}

impl Default for RunAreaMeta {
    fn default() -> Self {
        Self {
            run_area_id: -1,
            geo_fences_json: "[]".into(),
            freedom_show_fence: false,
        }
    }
}

/// 百度 BD-09 → 高德 GCJ-02。
pub fn bd09_to_gcj02(bd_lat: f64, bd_lng: f64) -> (f64, f64) {
    let x = bd_lng - 0.0065;
    let y = bd_lat - 0.006;
    let z = (x * x + y * y).sqrt() - 0.00002 * (y * X_PI).sin();
    let theta = y.atan2(x) - 0.000003 * (x * X_PI).cos();
    (z * theta.sin(), z * theta.cos())
}

/// gzip + base64。
pub fn gz(data: &[u8]) -> String {
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(data).expect("gzip 写入内存失败");
    let out = enc.finish().expect("gzip 收尾失败");
    super::super::crypto::envelope::b64_encode(&out)
}

fn gz_json(v: &Value) -> String {
    gz(v.to_string().as_bytes())
}

fn gz_str(v: &str) -> String {
    gz(v.as_bytes())
}

/// 累计量在任意时刻的插值。
///
/// 服务端读的是「每个窗口结束时刻的累计快照」之差；窗口边界（10 的整数倍）通常
/// 不与采样点重合，因此必须插值——直接取「边界前最后一个采样点」会让窗口距离
/// 系统性偏短，并让「窗内距离合计」对不上总距离。
pub fn cumulative_at(locs: &[GenPoint], t: f64, pick: impl Fn(&GenPoint) -> f64) -> f64 {
    if locs.is_empty() {
        return 0.0;
    }
    if t <= 0.0 {
        return pick(&locs[0]);
    }
    for pair in locs.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if t >= a.totalTime as f64 && t <= b.totalTime as f64 {
            // 起点哨兵与首点同刻（区间退化），跳过它。
            if b.totalTime == a.totalTime {
                continue;
            }
            let span = (b.totalTime - a.totalTime) as f64;
            let k = ((t - a.totalTime as f64) / span).clamp(0.0, 1.0);
            return pick(a) + (pick(b) - pick(a)) * k;
        }
    }
    pick(locs.last().unwrap())
}

/// 一个 10 秒窗口的边界与量值。
///
/// 提交体的 `speedPerTenSec` / `stepsPerTenSec` 与 OBS 的 `speed_json` /
/// `step_freq_json` 必须读同一份数据——两处各算一遍，只要口径差一点，服务端把
/// 两个载荷对起来就能看出矛盾。因此窗口只在这里定义一次。
#[derive(Clone, Copy, Debug)]
pub struct TenSecondSpan {
    /// 相对起跑时刻的秒数（10 的整数倍）。
    pub begin_s: i64,
    /// 相对起跑时刻的秒数；末窗不足 10 秒时等于总时长。
    pub end_s: i64,
    pub distance_m: f64,
    pub steps: i64,
}

/// 按 10 秒切窗：边界严格落在 10 的整数倍上，末窗可能不足 10 秒。
///
/// 各窗距离之和恒等于总距离、时长之和恒等于总时长（插值后逐窗相减是望远镜求和），
/// 因此「窗合计 vs 总量」这条守恒关系天然成立。
pub fn ten_second_spans(track: &Track) -> Vec<TenSecondSpan> {
    let total = track.totalTime;
    let locs = &track.locations;
    if total <= 0 || locs.len() < 2 {
        return Vec::new();
    }
    let n = (total + 9) / 10;
    let mut out = Vec::with_capacity(n as usize);
    for k in 0..n {
        let begin = k * 10;
        let end = (begin + 10).min(total);
        let d = cumulative_at(locs, end as f64, |p| p.totalDis)
            - cumulative_at(locs, begin as f64, |p| p.totalDis);
        let s = cumulative_at(locs, end as f64, |p| p.steps as f64)
            - cumulative_at(locs, begin as f64, |p| p.steps as f64);
        out.push(TenSecondSpan {
            begin_s: begin,
            end_s: end,
            distance_m: round_to(d.max(0.0), 4),
            steps: s.max(0.0).round() as i64,
        });
    }
    out
}

/// 协议点集（gen 点 → OBS 点；gLat/gLng 由 BD 转 GCJ）。
///
/// 字段与官方 payload **逐项对齐**。
///
/// 真机实测（3 条确认真跑记录、1039 个定位点）的点结构只有 27 个字段。
/// 之前我们多发了 `accuracy` / `hasAltitude` / `steps` 三个，
/// 而 `stepDistance` 真机**全部为 0**——多出的字段比字段值不对
/// 更容易被 schema 白名单挑出来。
pub fn conv_point(p: &GenPoint, start_ms: i64) -> Value {
    let (glat, glng) = bd09_to_gcj02(p.gLat, p.gLng);
    json!({
        "avgSpeed": round_to(p.avgSpeed, 4),
        "bdA": round_to(p.bdA, 2),
        "bdD": round_to(p.bdD, 2),
        "bdG": p.bdG,
        "bdS": round_to(p.bdS, 4),
        "coorType": "gcj02",
        "count": p.count,
        "dtr": 0.0,
        "flag": start_ms,
        "gLat": round_to(glat, 7),
        "gLng": round_to(glng, 7),
        "gainTime": p.gainTime,
        "id": p.id,
        "lat": -1.0,
        "lng": -1.0,
        "locType": p.locType,
        "locationId": "",
        "queueNum": p.queueNum,
        "radius": round_to(p.radius, 2),
        "speed": round_to(p.speed, 4),
        "state": p.state,
        // 真机这个字段恒为 0.0（实测 1039/1039 个点）。原先按“累计量”填了
        // 非零值，但真机从不启用它，填非零反而是差异。
        "stepDistance": 0.0,
        "totalDis": round_to(p.totalDis, 4),
        "totalTime": p.totalTime,
        "type": p.ptype,
        "validDis": round_to(p.validDis, 4),
        "validTime": p.validTime,
    })
}

/// 五点实体（实时点位 → 跑完态 `isPass=true`）。
///
/// `isFixed` **保留服务端下发的原值**，不做任何改写：
///   - `isFixed=1` = 必经点位（地图上黄色）
///   - `isFixed=0` = 普通点位（地图上蓝色）
///
/// 「已通过」由 `isPass=true` 表达——官方详情页据此把点画成绿色圆框打勾，与
/// `isFixed` 是**两个独立维度**。曾两次搞错这个区分：一次把所有点标 1（服务端
/// 数出 0 个普通点，type 3 无处可依），一次把 2 个点降成 0（详情页出现橙点）。
/// 「至少通过 2 个普通点位」依赖服务端能从载荷里认出哪些是普通点。
pub fn five_point_payload(points: &[Value], start_ms: i64) -> Vec<Value> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            json!({
                "flag": start_ms,
                "glat": p["glat"].as_f64().unwrap_or(0.0),
                "glon": p["glon"].as_f64().unwrap_or(0.0),
                "id": i as i64 + 1,
                // 跑完态：本次命中的打卡点全部标为固定点（详情页显示绿色打勾）。
                // 原样回传服务端标记：改写会让「必经/普通」分类与地图颜色失真。
                "isFixed": p.get("isFixed").and_then(|v| v.as_i64()).unwrap_or(1),
                "isPass": true,
                "lat": p["lat"].as_f64().unwrap_or(0.0),
                "lon": p["lon"].as_f64().unwrap_or(0.0),
                "pointName": p["pointName"].as_str().unwrap_or(""),
                "position": 999,
                "state": 0,
            })
        })
        .collect()
}

/// 统计点位里的普通点（服务端标为 `isFixed=0`）数量，供日志说明。
pub fn ordinary_point_count(points: &[Value]) -> usize {
    points
        .iter()
        .filter(|p| p.get("isFixed").and_then(|v| v.as_i64()).unwrap_or(0) == 0)
        .count()
}

/// 提交 body 的 fivePointJson 包装串。
#[allow(dead_code)]
pub fn five_point_wrapper(points: &[Value], start_ms: i64) -> String {
    five_point_wrapper_with_area(points, start_ms, &RunAreaMeta::default())
}

/// Build the fixed-point wrapper while preserving the server's campus area
/// and geofence metadata.
pub fn five_point_wrapper_with_area(points: &[Value], start_ms: i64, area: &RunAreaMeta) -> String {
    let five = five_point_payload(points, start_ms);
    json!({
        "useZip": false,
        "fivePointJson": Value::Array(five).to_string(),
        "runAreaId": area.run_area_id,
        "geoFencesJson": area.geo_fences_json,
        "freedomShowFence": area.freedom_show_fence,
    })
    .to_string()
}

/// 校验提交用五点轨迹仍是同一次点位请求产生的有效数据。
pub fn validate_five_point_wrapper(wrapper: &str) -> Result<(), String> {
    let outer: Value =
        serde_json::from_str(wrapper).map_err(|e| format!("五点轨迹 JSON 无效: {e}"))?;
    let raw = outer
        .get("fivePointJson")
        .and_then(Value::as_str)
        .ok_or("五点轨迹缺少 fivePointJson")?;
    let points: Vec<Value> =
        serde_json::from_str(raw).map_err(|e| format!("五点轨迹数组无效: {e}"))?;
    if points.is_empty() {
        return Err("五点轨迹不能为空".into());
    }
    for point in points {
        let lat = point.get("lat").and_then(Value::as_f64).unwrap_or(0.0);
        let lon = point.get("lon").and_then(Value::as_f64).unwrap_or(0.0);
        let glat = point.get("glat").and_then(Value::as_f64).unwrap_or(0.0);
        let glon = point.get("glon").and_then(Value::as_f64).unwrap_or(0.0);
        if (lat == 0.0 && lon == 0.0) || (glat == 0.0 && glon == 0.0) {
            return Err("五点轨迹包含缺失坐标".into());
        }
        crate::location::Coordinate::new(lat, lon, 0.0)?;
        crate::location::Coordinate::new(glat, glon, 0.0)?;
    }
    validate_fence_metadata(&outer)?;
    Ok(())
}

/// 围栏是学校能力差异字段：没有围栏时允许自由跑提交；如果服务端
/// 返回了围栏，则保持完整元数据并拒绝损坏或隐藏的围栏，避免详情页误显示。
fn validate_fence_metadata(outer: &Value) -> Result<(), String> {
    let Some(value) = outer.get("geoFencesJson") else {
        return Ok(());
    };
    let parsed = match value {
        Value::Null => return Ok(()),
        Value::String(text) if text.trim().is_empty() => return Ok(()),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .map_err(|e| format!("五点轨迹围栏 JSON 无效: {e}"))?,
        other => other.clone(),
    };
    let has_fences = match parsed {
        Value::Null => false,
        Value::Array(items) => !items.is_empty(),
        _ => return Err("五点轨迹围栏必须是数组".into()),
    };
    if !has_fences {
        return Ok(());
    }
    let run_area_id = outer
        .get("runAreaId")
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
        .unwrap_or(-1);
    if run_area_id < -1 {
        return Err("五点轨迹 runAreaId 无效".into());
    }
    if outer.get("freedomShowFence").and_then(Value::as_bool) != Some(true) {
        return Err("五点轨迹围栏元数据未启用".into());
    }
    Ok(())
}

#[cfg(test)]
mod validation_tests {
    use super::*;
    #[test]
    fn rejects_empty_or_malformed_five_point_payload() {
        assert!(validate_five_point_wrapper("{}").is_err());
        assert!(validate_five_point_wrapper(r#"{"fivePointJson":"[]"}"#).is_err());
    }

    fn five_point_wrapper_without_fence_fields() -> Value {
        json!({
            "fivePointJson": r#"[{"lat":39.4,"lon":116.2,"glat":39.4,"glon":116.2}]"#
        })
    }

    #[test]
    fn five_point_validation_allows_missing_fence_metadata() {
        assert!(validate_five_point_wrapper(
            &five_point_wrapper_without_fence_fields().to_string()
        )
        .is_ok());
    }

    #[test]
    fn five_point_validation_allows_empty_fence_metadata() {
        let mut wrapper = five_point_wrapper_without_fence_fields();
        wrapper["runAreaId"] = json!(-1);
        wrapper["geoFencesJson"] = json!("[]");
        wrapper["freedomShowFence"] = json!(false);
        assert!(validate_five_point_wrapper(&wrapper.to_string()).is_ok());
    }

    #[test]
    fn five_point_validation_allows_null_fence_metadata() {
        let mut wrapper = five_point_wrapper_without_fence_fields();
        wrapper["runAreaId"] = Value::Null;
        wrapper["geoFencesJson"] = Value::Null;
        wrapper["freedomShowFence"] = Value::Null;
        assert!(validate_five_point_wrapper(&wrapper.to_string()).is_ok());
    }

    #[test]
    fn five_point_validation_accepts_valid_fence_metadata() {
        let mut wrapper = five_point_wrapper_without_fence_fields();
        wrapper["runAreaId"] = json!(-1);
        wrapper["geoFencesJson"] = json!(r#"[{"id":1}]"#);
        wrapper["freedomShowFence"] = json!(true);
        assert!(validate_five_point_wrapper(&wrapper.to_string()).is_ok());
    }

    #[test]
    fn five_point_validation_rejects_hidden_non_empty_fence() {
        let mut wrapper = five_point_wrapper_without_fence_fields();
        wrapper["runAreaId"] = json!(-1);
        wrapper["geoFencesJson"] = json!(r#"[{"id":1}]"#);
        wrapper["freedomShowFence"] = json!(false);
        assert!(validate_five_point_wrapper(&wrapper.to_string()).is_err());
    }

    #[test]
    fn five_point_validation_rejects_malformed_present_fence() {
        let mut wrapper = five_point_wrapper_without_fence_fields();
        wrapper["runAreaId"] = json!(-1);
        wrapper["geoFencesJson"] = json!("not-json");
        wrapper["freedomShowFence"] = json!(true);
        assert!(validate_five_point_wrapper(&wrapper.to_string()).is_err());
    }
    fn wrapper_without_fence() -> Value {
        json!({"fivePointJson": r#"[{"lat":39.4,"lon":116.2,"glat":39.4,"glon":116.2}]"#})
    }

    #[test]
    fn fence_metadata_is_optional() {
        assert!(validate_five_point_wrapper(&wrapper_without_fence().to_string()).is_ok());
        let mut empty = wrapper_without_fence();
        empty["runAreaId"] = json!(-1);
        empty["geoFencesJson"] = json!("[]");
        empty["freedomShowFence"] = json!(false);
        assert!(validate_five_point_wrapper(&empty.to_string()).is_ok());
    }

    #[test]
    fn valid_fence_metadata_is_kept_but_invalid_present_metadata_is_rejected() {
        let mut valid = wrapper_without_fence();
        valid["geoFencesJson"] = json!(r#"[{"id":1}]"#);
        valid["freedomShowFence"] = json!(true);
        assert!(validate_five_point_wrapper(&valid.to_string()).is_ok());

        let mut hidden = valid.clone();
        hidden["freedomShowFence"] = json!(false);
        assert!(validate_five_point_wrapper(&hidden.to_string()).is_err());

        let mut malformed = valid;
        malformed["geoFencesJson"] = json!("not-json");
        assert!(validate_five_point_wrapper(&malformed.to_string()).is_err());
    }

    #[test]
    fn area_metadata_is_written_to_fixed_point_wrapper() {
        let area = RunAreaMeta {
            run_area_id: 42,
            geo_fences_json: "[{\"x\":1}]".into(),
            freedom_show_fence: true,
        };
        let wrapper = five_point_wrapper_with_area(
            &[json!({"lat": 39.9, "lon": 116.4, "glat": 39.9, "glon": 116.4})],
            1_700_000_000_000,
            &area,
        );
        let value: Value = serde_json::from_str(&wrapper).unwrap();
        assert_eq!(value["runAreaId"], 42);
        assert_eq!(value["geoFencesJson"], "[{\"x\":1}]");
        assert_eq!(value["freedomShowFence"], true);
    }

    #[test]
    fn completed_fixed_points_are_marked_fixed_even_if_source_point_is_zero() {
        let points = vec![json!({
            "lat": 39.9,
            "lon": 116.4,
            "glat": 39.9,
            "glon": 116.4,
            "isFixed": 0,
        })];
        let value: Value =
            serde_json::from_str(&five_point_wrapper(&points, 1_700_000_000_000)).unwrap();
        let raw = value["fivePointJson"].as_str().unwrap();
        let fixed: Value = serde_json::from_str(raw).unwrap();
        // isFixed 原样传回（这里服务端给的是 0），isPass 才表示已通过
        assert_eq!(fixed[0]["isFixed"], 0);
        assert_eq!(fixed[0]["isPass"], true);
    }

    /// 普通点配额：起点恒为 1，其后至多 2 个「服务端本来就标 0」的点保留 0，
    /// 其余仍标 1。这是「至少通过2个普通点位」能达标的关键。
    fn points_with_flags(flags: &[i64]) -> Vec<Value> {
        flags
            .iter()
            .enumerate()
            .map(|(i, f)| {
                json!({
                    "lat": 39.9 + i as f64 * 0.001,
                    "lon": 116.4 + i as f64 * 0.001,
                    "glat": 39.9 + i as f64 * 0.001,
                    "glon": 116.4 + i as f64 * 0.001,
                    "isFixed": f,
                    "pointName": format!("P{i}"),
                })
            })
            .collect()
    }

    fn payload_flags(points: &[Value]) -> Vec<i64> {
        five_point_payload(points, 1_700_000_000_000)
            .iter()
            .map(|p| p["isFixed"].as_i64().unwrap())
            .collect()
    }

    /// 跑完态一律全 1：详情页据此显示 5 个绿色打勾。
    ///
    /// `isFixed=0` 在官方详情页显示为橙色，是「未跑完/普通点」的形态。曾为凑
    /// 「至少通过 2 个普通点位」把 2 个点降成 0，结果出现 2 绿 3 橙，与真实
    /// 跑完记录不符。
    /// `isFixed` 原样回传：服务端的「必经/普通」分类不能被改写。
    #[test]
    fn completed_payload_preserves_server_fixed_flags() {
        let pts = points_with_flags(&[1, 0, 0, 0, 0]);
        assert_eq!(payload_flags(&pts), vec![1, 0, 0, 0, 0]);
        assert_eq!(ordinary_point_count(&pts), 4);
    }

    #[test]
    fn completed_payload_keeps_all_fixed_when_server_says_all_must_pass() {
        let pts = points_with_flags(&[1, 1, 1, 1]);
        assert_eq!(payload_flags(&pts), vec![1, 1, 1, 1]);
        assert_eq!(ordinary_point_count(&pts), 0);
    }

    /// `isPass` 独立于 `isFixed`：已通过才算跑完，与颜色无关。
    #[test]
    fn completed_payload_marks_every_point_passed() {
        let pts = points_with_flags(&[1, 0, 0]);
        let payload = five_point_payload(&pts, 1_700_000_000_000);
        assert!(payload.iter().all(|p| p["isPass"] == true));
    }

    #[test]
    fn laps_are_rebuilt_from_overridden_altitude() {
        let points = vec![(38.901678, 121.540241), (38.902564, 121.541233)];
        let mut track = crate::track::generator::build(
            1200.0,
            600,
            7,
            (38.9, 121.54),
            1_700_000_000_000,
            &points,
        );
        crate::track::altitude::override_bd_a(&mut track, 36.75).unwrap();
        let laps = build_laps(&track, track.startTime);
        assert!(!laps.is_empty());
        assert!(laps.iter().all(|lap| lap["elevationGain"] == 0.0));
        assert!(laps.iter().all(|lap| lap["endAltAbs"] == 36.75));
        assert!(laps.iter().all(|lap| lap["endAltRel"] == 0.0));
    }

    #[test]
    /// 真实手机载荷的统计特征：这些数字是从一条 918 s 真机记录里量出来的。
    ///
    /// 任何一项明显偏离，"这条记录是不是这个 App 记的"就成了问题。
    #[test]
    fn payload_matches_device_reading_signatures() {
        let points = vec![
            (38.901678, 121.540241),
            (38.902564, 121.541233),
            (38.900921, 121.542310),
            (38.899823, 121.541010),
            (38.900455, 121.539512),
        ];
        for seed in [0u64, 5, 42] {
            let track = crate::track::generator::build(
                2000.0,
                918,
                seed,
                (38.9, 121.54),
                1_700_000_000_000,
                &points,
            );
            let pts: Vec<serde_json::Value> = track
                .locations
                .iter()
                .map(|p| conv_point(p, track.startTime))
                .collect();
            let n = pts.len();

            // 1) 定位芯片给不出速度时读数为 0：真机约 19%，不能一个都没有，也不能过半。
            let zeros = pts
                .iter()
                .filter(|p| p["speed"].as_f64().unwrap() <= 0.0)
                .count();
            let zfrac = zeros as f64 / n as f64;
            assert!(
                (0.05..=0.35).contains(&zfrac),
                "seed={seed}: speed==0 占比 {:.0}%（真机约 19%）",
                zfrac * 100.0
            );

            // 2) stepDistance 真机恒为 0.0（实测 1039/1039 个点），
            //    我们也必须填 0；填累计值反而是差异。
            let last_sd = pts[n - 1]["stepDistance"].as_f64().unwrap();
            assert!(
                last_sd.abs() < 1e-9,
                "seed={seed}: stepDistance 应为 0，实际 {last_sd}"
            );

            // 2b) 点字段数必须与真机一致（27 个）。
            //     多字段比值不对更容易被 schema 白名单挑出来。
            let n_keys = pts[n - 1].as_object().unwrap().len();
            assert_eq!(n_keys, 27, "点字段数 {n_keys} 与真机 27 不一致");

            // 3) speed 与位移/时间差的比值：中位数接近 1，但分布必须明显散开
            //    （真机实测 0.64–1.26）。清一色 1.00 就是机器特征。
            let mut ratios: Vec<f64> = Vec::new();
            for i in 1..n {
                let a = &pts[i - 1];
                let b = &pts[i];
                let dy = (b["gLat"].as_f64().unwrap() - a["gLat"].as_f64().unwrap()) * 111132.0;
                let dx = (b["gLng"].as_f64().unwrap() - a["gLng"].as_f64().unwrap()) * 86600.0;
                let dt = b["totalTime"].as_i64().unwrap() - a["totalTime"].as_i64().unwrap();
                let sp = b["speed"].as_f64().unwrap();
                let d = (dx * dx + dy * dy).sqrt();
                if dt >= 1 && sp > 0.1 && d > 0.5 {
                    ratios.push(sp / (d / dt as f64 * 3.6));
                }
            }
            ratios.sort_by(|x: &f64, y: &f64| x.total_cmp(y));
            let p50 = ratios[ratios.len() / 2];
            let p10 = ratios[ratios.len() / 10];
            let p90 = ratios[ratios.len() * 9 / 10];
            assert!(
                (0.85..=1.15).contains(&p50),
                "seed={seed}: speed/位移 中位比 {p50:.2}"
            );
            assert!(
                p90 - p10 > 0.35,
                "seed={seed}: speed 噪声过窄（p10={p10:.2} p90={p90:.2}）"
            );

            // 4) 采样间隔：以 5 s 为主，但不能清一色 5 s。
            let mut gaps: Vec<i64> = Vec::new();
            for i in 2..n {
                gaps.push(
                    pts[i]["totalTime"].as_i64().unwrap()
                        - pts[i - 1]["totalTime"].as_i64().unwrap(),
                );
            }
            let five = gaps.iter().filter(|g| **g == 5).count() as f64 / gaps.len() as f64;
            // 真机定位回调严格 5 s 一拍（10 条真实记录的中位数与 P90 都是 5 s），
            // 只在丢点时出现 10/15 s。不该有 1–4 s 的「加密采样」。
            assert!(
                five >= 0.88,
                "seed={seed}: 5 s 间隔占比 {:.0}%，应接近 100%",
                five * 100.0
            );
            assert!(
                gaps.iter().all(|g| !(1..=4).contains(g)),
                "seed={seed}: 出现 1–4 s 间隔，与真机 5 s 节拍不符"
            );
            assert!(
                gaps.iter().any(|g| *g >= 10),
                "seed={seed}: 完全没有丢点（10/15 s 间隔）"
            );
        }
    }
    /// 提交体与 OBS 的 10s 窗必须读同一份数据，且与总量守恒。
    ///
    /// 旧实现两处各算一遍：OBS 用插值、提交体用「边界前最后一个采样点」，采样点
    /// 不落在 10 s 边界上时两边读数不同——服务端把两个载荷对起来就能看见矛盾。
    #[test]
    fn ten_second_spans_are_shared_and_conserved() {
        let points = vec![(38.901678, 121.540241), (38.902564, 121.541233)];
        let track = crate::track::generator::build(
            1440.0,
            661,
            3,
            (38.9, 121.54),
            1_700_000_000_000,
            &points,
        );
        let spans = ten_second_spans(&track);
        assert!(!spans.is_empty());
        // 边界严格落在 10 的整数倍上，最后一段可能不足 10 秒。
        for (k, s) in spans.iter().enumerate() {
            assert_eq!(s.begin_s, k as i64 * 10);
            assert!(s.end_s > s.begin_s, "窗 {k} 宽度必须为正");
            assert!(s.end_s - s.begin_s <= 10);
        }
        assert_eq!(spans.last().unwrap().end_s, track.totalTime);
        // 距离 / 步数 / 时长三组都严格守恒。
        let d: f64 = spans.iter().map(|s| s.distance_m).sum();
        let n: i64 = spans.iter().map(|s| s.steps).sum();
        let t: i64 = spans.iter().map(|s| s.end_s - s.begin_s).sum();
        assert!(
            (d / track.totalDistance - 1.0).abs() < 1e-3,
            "窗距离合计 {d}"
        );
        assert!(
            (n - track.totalSteps).unsigned_abs() <= 2,
            "窗步数合计 {n} vs totalSteps {}",
            track.totalSteps
        );
        assert_eq!(t, track.totalTime);
        // 与 `speedPerTenSec` / `stepsPerTenSec` 逐项一致（OBS 读的就是这两组）。
        assert_eq!(spans.len(), track.speedPerTenSec.len());
        for (s, w) in spans.iter().zip(track.speedPerTenSec.iter()) {
            assert!((s.distance_m - w.value).abs() < 0.01);
        }
    }

    #[test]
    fn lap_metrics_use_minutes_per_kilometre_and_stride_centimetres() {
        let points = vec![(38.901678, 121.540241), (38.902564, 121.541233)];
        let mut track = crate::track::generator::build(
            1200.0,
            600,
            7,
            (38.9, 121.54),
            1_700_000_000_000,
            &points,
        );
        let mut start = track.locations[0].clone();
        start.totalTime = 0;
        start.totalDis = 0.0;
        start.steps = 0;
        let mut finish = start.clone();
        finish.totalTime = 600;
        finish.totalDis = 1000.0;
        finish.steps = 1200;
        track.locations = vec![start, finish];

        let lap = build_laps(&track, track.startTime).remove(0);
        assert_eq!(lap["avgPace"].as_f64(), Some(10.0));
        assert_eq!(lap["avgStride"].as_f64(), Some(83.33));
    }
}

/// 10s 时间窗，id=(rrid%100000)*1000+窗口序秒（6 个真人样本跨 9 月记录验证一致；
/// 旧版 App 样本为全局序号，不适用当前版本）。
fn build_windows(track: &Track, rrid: i64) -> (Vec<Value>, Vec<Value>) {
    let start_ms = track.startTime;
    let mut sp = Vec::new();
    let mut stf = Vec::new();
    for span in ten_second_spans(track) {
        let id = (rrid % 100000) * 1000 + span.end_s;
        sp.push(json!({
            "beginTime": start_ms + span.begin_s * 1000,
            "distance": span.distance_m,
            "endTime": start_ms + span.end_s * 1000,
            "flag": start_ms,
            "id": id,
            "queueNum": 0,
            "state": 0,
        }));
        stf.push(json!({
            "avgDiff": 0.0,
            "beginTime": start_ms + span.begin_s * 1000,
            "endTime": start_ms + span.end_s * 1000,
            "flag": start_ms,
            "id": id,
            "maxDiff": 0.0,
            "minDiff": 1000.0,
            "queueNum": 0,
            "state": 0,
            "stepsNum": span.steps,
        }));
    }
    (sp, stf)
}

/// 圈（每 1000m 一圈，末圈 isFullLap=false；avgStride 单位厘米）。
fn build_laps(track: &Track, start_ms: i64) -> Vec<Value> {
    let mut laps = Vec::new();
    let locs = &track.locations;
    // 手动海拔覆盖后，曲线重算出来的爬升没有意义（见 altitude_gain_override）。
    let gain_override = track.altitude_gain_override;
    let (mut prev_d, mut prev_t, mut prev_steps, mut gain) = (0.0f64, 0i64, 0i64, 0.0f64);
    let total_gain = match gain_override {
        Some(v) => v,
        None => super::altitude::total_ascent(locs),
    };
    let alt0 = locs.first().map(|p| p.bdA).unwrap_or(0.0);
    for (i, pt) in locs.iter().enumerate() {
        if i > 0 {
            let dd = pt.bdA - locs[i - 1].bdA;
            if dd > 0.0 {
                gain += super::altitude::positive_ascent_delta(dd);
            }
        }
        let d_now = pt.totalDis;
        let t_now = pt.totalTime;
        let last = i == locs.len() - 1;
        if d_now - prev_d >= 1000.0 || last {
            let lap_d = d_now - prev_d;
            let lap_t = 1i64.max(t_now - prev_t);
            let lap_steps = pt.steps - prev_steps;
            laps.push(json!({
                "avgCadence": round_to(lap_steps as f64 / (lap_t as f64 / 60.0), 2),
                "avgPace": round_to((lap_t as f64 / 60.0) / (lap_d / 1000.0).max(0.001), 2),
                "avgStride": round_to(lap_d / 1.max(lap_steps) as f64 * 100.0, 2),
                "cumulativeDuration": t_now,
                "distance": round_to(lap_d, 4),
                "duration": lap_t,
                "elevationGain": round_to(
                    match gain_override {
                        // 有覆盖值时按里程占比分摊总爬升
                        Some(_) => total_gain * (d_now / locs.last().map(|p| p.totalDis).unwrap_or(d_now).max(1.0)),
                        None => gain,
                    },
                    2,
                ),
                "endAltAbs": round_to(pt.bdA, 2),
                "endAltRel": round_to(pt.bdA - alt0, 2),
                "flag": start_ms,
                "id": laps.len() as i64 + 1,
                "isFullLap": lap_d >= 1000.0,
                "lapIndex": laps.len() as i64 + 1,
                "step": lap_steps,
            }));
            prev_d = d_now;
            prev_t = t_now;
            prev_steps = pt.steps;
            gain = 0.0;
        }
    }
    laps
}

/// 组装 10 键 OBS 对象（值均 gzip+base64；segment_json/runFaceCheck 为空串 gzip）。
pub fn build_obs_object(
    track: &Track,
    rrid: i64,
    uuid: &str,
    uid: i64,
    live_points: &[Value],
) -> Value {
    build_obs_object_with_area(track, rrid, uuid, uid, live_points, &RunAreaMeta::default())
}

/// Build the OBS payload with the same area metadata used by submission.
pub fn build_obs_object_with_area(
    track: &Track,
    rrid: i64,
    uuid: &str,
    uid: i64,
    live_points: &[Value],
    area: &RunAreaMeta,
) -> Value {
    let start_ms = track.startTime;
    let pts: Vec<Value> = track
        .locations
        .iter()
        .map(|p| conv_point(p, start_ms))
        .collect();
    let run_wrap = json!({ "allLocJson": Value::Array(pts).to_string(), "useZip": false });
    let (sp, stf) = build_windows(track, rrid);
    let laps = build_laps(track, start_ms);
    let five = five_point_payload(live_points, start_ms);
    let fx = json!({
        "fivePointJson": Value::Array(five).to_string(),
        "freedomShowFence": area.freedom_show_fence,
        "geoFencesJson": area.geo_fences_json,
        "runAreaId": area.run_area_id,
        "useZip": false,
    });
    json!({
        "rrid": gz_str(&rrid.to_string()),
        "uuid": gz_str(uuid),
        "uid": gz_str(&uid.to_string()),
        "run_data": gz_json(&run_wrap),
        "fixed_point_json": gz_json(&fx),
        "segment_json": gz_str(""),
        "speed_json": gz_json(&Value::Array(sp)),
        "step_freq_json": gz_json(&Value::Array(stf)),
        "laps_json": gz_json(&Value::Array(laps)),
        "runFaceCheck": gz_str(""),
    })
}

/// OBS objectKey（两个都要传）。
pub fn obs_keys(track: &Track, rrid: i64, uuid: &str) -> Vec<String> {
    let t0 = Local
        .timestamp_millis_opt(track.startTime)
        .single()
        .map(|t| t.format("%Y%m%d%H").to_string())
        .unwrap_or_default();
    vec![
        format!("run_data/{t0}/{uuid}.json"),
        format!("run_data/{}/{}.json", rrid / 1000000, rrid),
    ]
}
