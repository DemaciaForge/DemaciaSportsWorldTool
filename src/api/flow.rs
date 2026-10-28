//! 全链编排：policy → 实时点位 → 轨迹生成 → 提交 → OBS → 详情验证。
//! 由 UI 后台线程调用，log 闭包回传日志。

use super::client::ApiClient;
use super::model::Session;
use super::points;
use super::policy::fetch_policy;
use super::records::fetch_one_record;
use super::submit::{submit_record, SubmitParams, SubmitResult};
use crate::location::Coordinate;
use crate::track::generator::build as gen_track;
use crate::track::generator::build_with_fences as gen_track_fenced;
use crate::track::wire::{
    build_obs_object, build_obs_object_with_area, five_point_wrapper, five_point_wrapper_with_area,
    obs_keys, validate_five_point_wrapper,
};
use rand_distr::{Distribution, Normal};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct RunParams {
    /// 距离（米）与时长（秒）已由 UI 参数解析。
    pub dist: f64,
    pub dur: i64,
    /// 开始时间（毫秒）。
    pub start_ms: i64,
    /// 提交体的 `faceCheck`。`None` = 跟随学校策略里的 `faceVerify`。
    pub face_check: Option<i64>,
    /// 用户手动填写的绝对海拔（米）；None 使用生成器海拔。
    pub manual_altitude: Option<f64>,
    /// 用户手动填写的海拔范围；与单值字段兼容，范围优先。
    pub manual_altitude_range: Option<crate::track::altitude::AltitudeRange>,
    pub seed: u64,
    /// 标准跑道规格：`Auto` = 按打卡点拟合环，其余 = 按椭圆跑道拟合（允许变形以经过点位）。
    pub track_spec: crate::track::stadium::TrackSpec,
    /// 海拔曲线参数（基准 + 震荡幅度），由本地用户填写。
    pub altitude_profile: crate::track::altitude::AltitudeProfile,
}

pub struct RunOutcome {
    pub result: SubmitResult,
    pub obs_ok: usize,
    pub detail_ok: bool,
}

/// 本地 dry-run 的摘要；不会读取会话、访问网络或上传任何数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DryRunSummary {
    pub distance_m: f64,
    pub duration_s: i64,
    pub track_points: usize,
    pub obs_keys: usize,
    pub start_latitude: f64,
    pub start_longitude: f64,
    pub min_speed_mps: f64,
    pub max_speed_mps: f64,
    /// 海拔下界 / 上界 / 带宽 / 逐点拖动中位数（米）。
    pub altitude: (f64, f64, f64, f64),
    /// 真实性互校结论（逐项读数 + 失败原因）。
    pub realism: crate::track::realism::AuthenticityReport,
}

fn dry_run_points(anchor: Coordinate) -> Vec<Value> {
    // 仅用于本地结构校验的四点小环；不会被当作实时点位或上传。
    let lat = anchor.latitude;
    let lon = anchor.longitude;
    [
        (lat - 0.0008, lon - 0.0010),
        (lat - 0.0008, lon + 0.0010),
        (lat + 0.0008, lon + 0.0010),
        (lat + 0.0008, lon - 0.0010),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (point_lat, point_lon))| {
        json!({
            "lat": point_lat,
            "lon": point_lon,
            "glat": point_lat,
            "glon": point_lon,
            "pointName": format!("dry-run-{i}"),
            "isFixed": 1,
        })
    })
    .collect()
}

/// 构造并校验一条完整本地记录，不创建 ApiClient，也不触发任何网络请求。
pub fn run_dry_run(params: &RunParams, anchor: Coordinate) -> Result<DryRunSummary, String> {
    anchor.validate()?;
    if params.dist <= 0.0 || !params.dist.is_finite() {
        return Err("dry-run 距离必须是正数".into());
    }
    if params.dur <= 0 {
        return Err("dry-run 时长必须是正数".into());
    }
    if params.start_ms <= 0 {
        return Err("dry-run 开始时间无效".into());
    }

    let live_points = dry_run_points(anchor);
    let point_coords: Vec<(f64, f64)> = live_points
        .iter()
        .map(|p| {
            let lat = p["lat"].as_f64().ok_or("dry-run 点位缺纬度")?;
            let lon = p["lon"].as_f64().ok_or("dry-run 点位缺经度")?;
            Coordinate::new(lat, lon, 0.0)?;
            Ok((lat, lon))
        })
        .collect::<Result<_, String>>()?;
    // 必须把海拔 profile 传进去：否则 dry-run 永远用默认基准高度，
    // 打印出来的海拔与真正提交时不一致，自检就没意义了。
    // 和真提交一样做距离自洽重试：弧长与弦长差 ~2%，
    // 单次生成约有 10% 概率超出容差。干跑不重试就会误报失败，
    // 而真正提交时会重试并成功——两边结论不一致。
    let mut track = {
        let mut picked = None;
        for attempt in 0..5u64 {
            let seed = params.seed.wrapping_add(attempt * 7919);
            let candidate = gen_track_fenced(
                params.dist,
                params.dur,
                seed,
                (anchor.latitude, anchor.longitude),
                params.start_ms,
                &point_coords,
                &[],
                params.altitude_profile,
            );
            let measured = chord_path_len(&candidate);
            let ratio = measured / candidate.totalDistance.max(1.0);
            if (ratio - 1.0).abs() <= 0.018 || attempt == 4 {
                picked = Some(candidate);
                break;
            }
        }
        picked.ok_or("轨迹生成失败")?
    };
    if let Some(range) = params.manual_altitude_range {
        crate::track::altitude::override_bd_a_range(&mut track, range)?;
    } else if let Some(altitude_m) = params.manual_altitude {
        crate::track::altitude::override_bd_a(&mut track, altitude_m)?;
    }
    track.validate_consistency()?;

    let five = five_point_wrapper(&live_points, track.startTime);
    validate_five_point_wrapper(&five)?;
    let obj = build_obs_object(&track, 0, "dry-run", 0, &live_points);
    let keys = obs_keys(&track, 0, "dry-run");
    if keys.len() != 2 {
        return Err(format!("dry-run OBS key 数量异常: {}", keys.len()));
    }
    for key in [
        "rrid",
        "uuid",
        "uid",
        "run_data",
        "fixed_point_json",
        "segment_json",
        "speed_json",
        "step_freq_json",
        "laps_json",
        "runFaceCheck",
    ] {
        if obj
            .get(key)
            .and_then(Value::as_str)
            .map(|value| value.is_empty())
            .unwrap_or(true)
        {
            return Err(format!("dry-run OBS 字段为空: {key}"));
        }
    }

    // 真实性互校：把这条记录当成服务端那样重算一遍。任何一项不通过都直接失败，
    // 不生成"看起来像但物理上对不上"的记录。
    let realism = crate::track::realism::assert_authentic(&track)?;

    let speed_samples: Vec<f64> = track
        .locations
        .iter()
        // avgSpeed 是逐段位移/时长；speed 是协议中的 GPS 瞬时速度。
        // 校验用 avgSpeed，speed 的噪声由 realism 模块单独检查。
        .map(|p| p.avgSpeed)
        .filter(|speed| speed.is_finite() && *speed > 0.05)
        .collect();
    if speed_samples.is_empty() {
        return Err("dry-run 没有可校验的有效速度样本".into());
    }
    let (min_speed_mps, max_speed_mps) = speed_samples
        .into_iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), speed| {
            (min.min(speed), max.max(speed))
        });

    let altitude = {
        let alt: Vec<f64> = track.locations.iter().map(|p| p.bdA).collect();
        let mn = alt.iter().cloned().fold(f64::INFINITY, f64::min);
        let mx = alt.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mut d: Vec<f64> = alt.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        if !d.is_empty() {
            d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        }
        (mn, mx, mx - mn, d.get(d.len() / 2).copied().unwrap_or(0.0))
    };

    Ok(DryRunSummary {
        distance_m: track.totalDistance,
        duration_s: track.totalTime,
        track_points: track.locations.len(),
        obs_keys: keys.len(),
        start_latitude: track.startLatitude,
        start_longitude: track.startLongitude,
        min_speed_mps,
        max_speed_mps,
        altitude,
        realism,
    })
}

fn sleep_secs(s: u64) {
    std::thread::sleep(std::time::Duration::from_secs(s));
}

/// 跑步全链。
pub fn run_full_flow(
    client: &mut ApiClient,
    params: &RunParams,
    log: &mut dyn FnMut(&str),
) -> Result<RunOutcome, String> {
    let sess: Session = client.login.clone().ok_or("未登录")?;

    // ① policy
    log("[policy] 拉取跑步策略…");
    let pol = fetch_policy(client)?;
    // 冻结期门禁：真实客户端在 freezeRunTime > 0 时禁用开始按钮，账号在冻结期内
    // 不会有任何跑步提交。这里必须在生成轨迹、拉点位、上传 OBS 之前就拦住。
    log(&format!(
        "OK [policy] target {}m | mode {} (sequenceRule={} strength={})",
        pol.min_distance,
        pol.sequence_mode(),
        pol.sequence_rule,
        pol.sequence_strength
    ));
    if pol.run_rule_absent() {
        log("⚠ [policy] 服务端未下发 runRuleModel，本次目标距离/配速区间等均为兜底值");
    }
    // 冻结门禁必须在生成轨迹、拉点位、上传 OBS 之前。
    if let Some(secs) = pol.frozen_for_secs() {
        log(&format!(
            "× [policy] 跑步冻结剩余 {secs} 秒（freezeRunTime={secs}）"
        ));
    }
    crate::api::policy::ensure_not_frozen(&pol)?;
    crate::api::policy::ensure_distance_ok(&pol, params.dist)?;
    log(&format!(
        "√ [policy] ts={} policy={} minDistance={} validTime={}",
        pol.timestamp, pol.policy, pol.min_distance, pol.valid_time
    ));
    sleep_secs(2);

    // ② 实时点位（拒绝本地样本兜底）
    log("[points] 拉取实时点位…");
    if client.identity.has_unconfigured_default_location() {
        return Err("请先在设备信息页填写本次跑步所在城市和定位锚点，不能使用大连默认配置".into());
    }
    let anchor: Coordinate = client.identity.anchor_coordinate()?;
    let points_ctx = points::fetch_points_context(client, anchor, log)?;
    let mut area = points_ctx.area.clone();
    // 部分学校把围栏放在策略响应，另一些学校放在点位响应；优先使用
    // 明确返回的有效围栏，两个接口都没有时保持自由跑默认值。
    if pol.area.freedom_show_fence && pol.area.geo_fences_json.trim() != "[]" {
        area = pol.area.clone();
    } else if area.run_area_id < 0 && pol.area.run_area_id >= 0 {
        area.run_area_id = pol.area.run_area_id;
    }
    // 围栏来源优先级：策略内嵌 > 独立接口 > 点位缓存兜底。
    //
    // 跑道摆正要用围栏（真实操场的直道顺着场地长边铺），所以三条来源都要走到；
    // 原来的写法只在「策略没给围栏」时才调独立接口，导致策略自带围栏的学校
    // 反而拿不到围栏来摆正——这里一并修掉。
    //
    // 三者都拿不到时 `layout_fences` 为空，跑道退化为纯几何拟合（轨迹仍合法）。
    let mut layout_fences: Vec<Vec<(f64, f64)>> = if area.freedom_show_fence {
        crate::api::fence::parse_fences_json(&area.geo_fences_json)
    } else {
        Vec::new()
    };
    if !layout_fences.is_empty() {
        log(&format!(
            "√ [fence] 策略内嵌围栏 {} 个",
            layout_fences.len()
        ));
    }

    // 策略没给（或给了但解析不出多边形）时才请求独立接口；成功即由提交末尾的
    // save_points_cache_context 连同 area 一起落盘，供下次接口故障时兜底。
    if layout_fences.is_empty() {
        match crate::api::fence::fetch_geo_fence_with_area(client) {
            Ok(response) => {
                if response.area.freedom_show_fence && response.area.geo_fences_json.trim() != "[]"
                {
                    area = response.area;
                    log(&format!(
                        "√ [fence] 独立接口返回绿色围栏 runAreaId={}（{} 字节）",
                        area.run_area_id,
                        area.geo_fences_json.len(),
                    ));
                } else {
                    log("[fence] 独立接口未返回可用围栏，继续自由跑");
                }
                layout_fences = response.polygons;
            }
            Err(e) => log(&format!("⚠ [fence] 独立接口获取失败: {e}")),
        }
    }

    // 兜底：用上一次成功提交时缓存下来的围栏（同一场地才用）。
    if layout_fences.is_empty() {
        match crate::api::model::load_fence_cache_for(anchor) {
            Some((ts, fences)) => {
                log(&format!(
                    "√ [fence] 回退缓存围栏 {} 个（缓存于 {}）",
                    fences.len(),
                    crate::track::geom::fmt_gain_time(ts)
                ));
                layout_fences = fences;
            }
            None => log("⚠ [fence] 无可用围栏，跑道直道退化为纯几何拟合"),
        }
    }
    if !layout_fences.is_empty() {
        log(&format!(
            "√ [track] {} 个围栏用于摆正跑道直道",
            layout_fences.len()
        ));
    }
    // 本函数内所有经典轨迹生成都走带围栏的版本（含各路开销回退分支），
    // 保证回退时摆正行为一致。闭包遮蔽同名别名，调用点无需改动。
    let gen_track = |dist: f64,
                     dur: i64,
                     seed: u64,
                     center: (f64, f64),
                     start_ms: i64,
                     points_bd: &[(f64, f64)]| {
        gen_track_fenced(
            dist,
            dur,
            seed,
            center,
            start_ms,
            points_bd,
            &layout_fences,
            params.altitude_profile,
        )
    };
    let pts = points_ctx.points.clone();
    if pts.is_empty() {
        return Err("实时点位为空 —— 拒绝本地样本兜底".into());
    }
    log(&format!(
        "√ [points] {} 个点位，runAreaId={}，绿色围栏={}（{} 字节）",
        pts.len(),
        area.run_area_id,
        area.freedom_show_fence,
        area.geo_fences_json.len(),
    ));
    // 顺序跑学校（policy=1）要求按点位编号依次经过。服务端下发数组不保证有序
    // （实测缓存顺序是 4、5、2、3、1），必须排回编号顺序再生成轨迹与载荷。
    let pts: Vec<serde_json::Value> = if pol.is_sequence() {
        let sorted = points::sorted_by_ordinal(&pts);
        log(&format!(
            "√ [points] 顺序跑模式：点位已按编号排序 → {}",
            sorted
                .iter()
                .map(|p| p.get("pointName").and_then(|v| v.as_str()).unwrap_or(""))
                .collect::<Vec<_>>()
                .join(" → ")
        ));
        sorted
    } else {
        pts
    };
    for p in pts.iter().take(5) {
        log(&format!(
            "  [points] {} BD=({:.6},{:.6}) GCJ=({},{})",
            p.get("pointName").and_then(|v| v.as_str()).unwrap_or(""),
            p.get("lat").and_then(|v| v.as_f64()).unwrap_or(0.0),
            p.get("lon").and_then(|v| v.as_f64()).unwrap_or(0.0),
            p.get("glat").map(|v| v.to_string()).unwrap_or_default(),
            p.get("glon").map(|v| v.to_string()).unwrap_or_default(),
        ));
    }

    // ③ 轨迹生成：经典打卡点环
    let pts_bd = points::points_bd(&pts);
    log(&format!(
        "√ [track] 打卡点 {} 个（其中 {} 个为普通点，提交时全部标为已通过）",
        pts_bd.len(),
        crate::track::wire::ordinary_point_count(&pts)
    ));
    // 提交/OBS 只报实际经过的硬点（Road 模式），避免地图出现不在轨迹上的普通点；
    // Legacy 走全部点位，保持原行为。
    let submitted_pts: Vec<Value> = pts.clone();

    // 用打卡点随机偏移更新锚点并持久化：下次拉点位即学校真实坐标，摆脱写死的默认值
    if !pts_bd.is_empty() {
        let idx = (rand::random::<f64>() * pts_bd.len() as f64) as usize;
        let (clat, clng) = pts_bd[idx];
        let mut rng = rand::thread_rng();
        let normal = Normal::<f64>::new(0.0, 120.0).unwrap();
        let dlat =
            normal.sample(&mut rng).clamp(-200.0, 200.0) / crate::track::geom::MET_PER_DEG_LAT;
        let dlng =
            normal.sample(&mut rng).clamp(-200.0, 200.0) / crate::track::geom::MET_PER_DEG_LNG;
        client.identity.anchor_lat = clat + dlat;
        client.identity.anchor_lon = clng + dlng;
        if let Err(e) = super::model::save_identity(&client.identity) {
            log(&format!("⚠ 锚点持久化失败: {e}"));
        }
        // 用漂移后的新锚点重存点位缓存，使缓存锚点与持久化锚点一致，避免预览锚点失配。
        if let Ok(new_anchor) = client.identity.anchor_coordinate() {
            let _ = super::model::save_points_cache_context(new_anchor, &pts, &area);
        }
    }
    // 平均配速须落在有效窗口内（否则逐点速度无法全窗内），越界时修正时长
    let mut params = params.clone();
    let avg = params.dist / params.dur as f64;
    let fixed_avg = avg.clamp(
        crate::track::generator::SPEED_FLOOR + 0.1,
        crate::track::generator::SPEED_CEIL - 0.1,
    );
    if (fixed_avg - avg).abs() > 1e-6 {
        let fixed_dur = (params.dist / fixed_avg).round() as i64;
        log(&format!(
            "[track] 平均配速 {} m/s 超出有效窗口，时长 {} -> {}s",
            (avg * 100.0).round() / 100.0,
            params.dur,
            fixed_dur
        ));
        params.dur = fixed_dur;
    }
    log(&format!(
        "[track] 生成轨迹 {:.0}m / {}s（{} 点位）…",
        params.dist,
        params.dur,
        pts_bd.len()
    ));
    // 起跑时刻由调用方给定，恒等于「现在 − 用时」。这里不再加随机偏移，
    // 否则终点会落到未来，与「刚跑完就提交」矛盾。
    let start_ms = params.start_ms;
    // 顺序跑学校（policy=1）必须按点位编号依次经过，强制走点位环；
    // 椭圆拟合只保证路过点位附近，不保证经过次序。
    let effective_spec = if pol.is_sequence() {
        log("[track] 顺序跑策略 → 使用点位环（按编号依次经过，不做椭圆拟合）");
        crate::track::stadium::TrackSpec::Ordered
    } else {
        params.track_spec
    };
    // 生成 + 距离自洽校正：内部按曲线弧长给出 totalDis，而服务端按 GPS 弦长
    // 换算里程，两者相差约 2%。超出保守阈值就换种子重生成（最多 3 次），
    // 免得把一条注定被互校拦下的轨迹交给后面。最多试 5 个种子。
    let build_with_seed = |seed: u64| -> Result<crate::track::model::Track, String> {
        if effective_spec != crate::track::stadium::TrackSpec::Auto {
            crate::track::generator::build_stadium_with_fences(
                params.dist,
                params.dur,
                seed,
                (anchor.latitude, anchor.longitude),
                start_ms,
                &pts_bd,
                effective_spec,
                &layout_fences,
                params.altitude_profile,
            )
        } else {
            Ok(gen_track(
                params.dist,
                params.dur,
                seed,
                (anchor.latitude, anchor.longitude),
                start_ms,
                &pts_bd,
            ))
        }
    };
    let mut track = {
        let mut picked = build_with_seed(params.seed)?;
        for attempt in 1..=4u64 {
            let measured = chord_path_len(&picked);
            let ratio = measured / picked.totalDistance.max(1.0);
            if (ratio - 1.0).abs() <= 0.018 {
                break;
            }
            log(&format!(
                "[track] 第 {} 次生成距离自洽度 {:.3} 偏低，换种子重试",
                attempt, ratio
            ));
            picked = build_with_seed(params.seed.wrapping_add(attempt * 7919))?;
        }
        picked
    };
    if let Some(range) = params.manual_altitude_range {
        crate::track::altitude::override_bd_a_range(&mut track, range)?;
        log(&format!(
            "√ [track] 已将海拔曲线映射到 {:.2}-{:.2}m，覆盖 {} 个点，爬升/圈数据将按覆盖值计算",
            range.min_m,
            range.max_m,
            track.locations.len()
        ));
    } else if let Some(altitude_m) = params.manual_altitude {
        crate::track::altitude::override_bd_a(&mut track, altitude_m)?;
        log(&format!(
            "√ [track] 已用手动海拔 {:.2}m 覆盖 {} 个点，爬升/圈数据将按覆盖值计算",
            altitude_m,
            track.locations.len()
        ));
    }
    // 海拔形态直接打出来：它是判 201 的疑似特征，提交前必须能看到。
    {
        let alt: Vec<f64> = track.locations.iter().map(|p| p.bdA).collect();
        if !alt.is_empty() {
            let mn = alt.iter().cloned().fold(f64::INFINITY, f64::min);
            let mx = alt.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let mut d: Vec<f64> = alt.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            if !d.is_empty() {
                d.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            }
            let med = d.get(d.len() / 2).copied().unwrap_or(0.0);
            log(&format!(
                "√ [track] 海拔 {mn:.2}-{mx:.2} m（带宽 {:.2}，拖动中位 {med:.3}）",
                mx - mn
            ));
        }
    }
    log(&format!(
        "√ [track] {} 点 totalDis={:.0}m steps={} 起点={}",
        track.locations.len(),
        track.totalDistance,
        track.totalSteps,
        chrono::Local
            .timestamp_millis_opt(params.start_ms)
            .single()
            .map(|t| t.format("%H:%M:%S").to_string())
            .unwrap_or_default(),
    ));

    // ③b 真实性闸门：真正发出去之前，把这条轨迹当成服务端重算一遍。
    //
    // 这一步以前只有 dry-run 做，正式流程直接提交——也就是说几何或时序一旦出问题，
    // 是"发出去了才知道"。放在这里，宁可本地报错也不发一条会被判无效的记录。
    {
        let report = crate::track::realism::audit(&track);
        log(&format!("[realism] {}", report.summary()));
        for f in &report.failures {
            log(&format!("  ✗ [realism] {f}"));
        }
        if !report.passed() {
            return Err(format!(
                "轨迹未通过真实性互校（{}）：{}\n已中止提交，避免被服务端判为无效记录。\n                 可换一个随机版本重试（GUI 点「换一版」）。",
                report.summary(),
                report.failed_brief()
            ));
        }
    }

    // ④ 五点 wrapper（跑完态，仅报实际经过的点并保留区域元数据）
    let five = five_point_wrapper_with_area(&submitted_pts, track.startTime, &area);
    let _ = &five;

    // ⑤ 提交（sportType=1）
    log("[record] 提交跑步记录（sportType=1）…");
    let sp = SubmitParams {
        track,
        uid: sess.uid,
        selected_unid: sess.unid.parse().unwrap_or(0),
        policy: pol.policy,
        policy_ts: pol.timestamp,
        min_distance: pol.min_distance,
        weight: if sess.weight > 0.0 { sess.weight } else { 68.0 },
        face_check: params
            .face_check
            .unwrap_or(if pol.face_verify { 1 } else { 0 }),
        five_point_json: five,
        address: client.identity.city.clone(),
    };
    let result = submit_record(client, &sp, log)?;
    sleep_secs(1);

    // ⑤b 提交台账：服务端列表只保留最近 10 条，被挤出后 rrid/uuid 就再也拿不到，
    //     那条记录的 OBS 也就无法回读。在"提交成功"这一刻先把标识落盘。
    let ledger_entry = crate::ledger::SubmittedRecord {
        submitted_at: chrono::Utc::now().timestamp_millis(),
        rrid: result.rrid,
        uuid: result.uuid.clone(),
        start_ms: result.start_ms,
        total_dis: result.total_dis,
        total_time: result.total_time,
        total_steps: result.total_steps,
        target_dist: params.dist,
        target_dur: params.dur,
        points: sp.track.locations.len(),
        route_mode: "legacy".to_string(),
        seed: params.seed,
        realism_passed: true,
        obs_ok: None,
    };
    match crate::ledger::append(&ledger_entry) {
        Ok(path) => log(&format!(
            "√ [ledger] 已记账 rrid={} → {}",
            result.rrid,
            path.display()
        )),
        Err(e) => log(&format!("⚠ [ledger] 记账失败（不影响本次提交）: {e}")),
    }

    // ⑥ OBS 上传（双 key）
    log("[obs] 上传 OBS 对象（gzip+base64，10 键）…");
    // 从提交结果回填 track.startTime（含随机秒偏移），保证 body/OBS/flag 全链一致
    let mut track_for_obs = sp.track.clone();
    track_for_obs.startTime = result.start_ms;
    let obj = build_obs_object_with_area(
        &track_for_obs,
        result.rrid,
        &result.uuid,
        sess.uid,
        &submitted_pts,
        &area,
    );
    let payload = obj.to_string().into_bytes();
    let keys = obs_keys(&track_for_obs, result.rrid, &result.uuid);
    let obs_ok = super::obs::upload_both_keys(client, &keys, &payload, log);
    if obs_ok == 2 {
        log("√ [obs] 双 key 上传成功");
    } else {
        log(&format!("⚠ [obs] 上传成功 {obs_ok}/2"));
    }
    // 回填 OBS 结果；失败只记日志，台账里 rrid 已经在上面落盘了。
    if let Err(e) = crate::ledger::set_obs_ok(result.rrid, obs_ok) {
        log(&format!("⚠ [ledger] OBS 结果回填失败: {e}"));
    }

    // ⑦ 详情验证
    sleep_secs(2);
    log("[verify] 拉取详情验证…");
    let detail_ok = match fetch_one_record(client, result.rrid) {
        Ok(d) => {
            log(&format!(
                "√ [verify] rrid={} complete={:?} dis={:?} time={:?}",
                result.rrid,
                d.get("complete").and_then(|v| v.as_bool()),
                d.get("totalDis"),
                d.get("totalTime"),
            ));
            if let Ok(mut slot) = VERIFY_DETAIL.lock() {
                slot.replace(d.clone());
            }
            true
        }
        Err(e) => {
            log(&format!(
                "⚠ [verify] 详情拉取失败（提交已成功 rrid={}）：{e}",
                result.rrid
            ));
            false
        }
    };
    Ok(RunOutcome {
        result,
        obs_ok,
        detail_ok,
    })
}

#[cfg(test)]
mod tests {
    use super::{run_dry_run, RunParams};
    use crate::location::Coordinate;

    fn params() -> RunParams {
        RunParams {
            dist: 2_000.0,
            dur: 800,
            start_ms: 1_700_000_000_000,
            face_check: None,
            manual_altitude: None,
            manual_altitude_range: None,
            seed: 7,
            track_spec: crate::track::stadium::TrackSpec::Auto,
            altitude_profile: crate::track::altitude::AltitudeProfile::default(),
        }
    }

    #[test]
    fn dry_run_builds_and_validates_protocol_payloads() {
        let summary = run_dry_run(&params(), Coordinate::new(39.9, 116.4, 0.0).unwrap()).unwrap();
        assert_eq!(summary.duration_s, 800);
        assert_eq!(summary.obs_keys, 2);
        assert!(summary.track_points > 10);
        // 声明距离取自实测位移，允许 ±3% 的真实几何偏差
        assert!(
            (summary.distance_m / 2_000.0 - 1.0).abs() < 0.03,
            "dist={}",
            summary.distance_m
        );
        // 真实性互校必须全部通过（这正是 dry-run 的意义）
        assert!(
            summary.realism.passed(),
            "{}",
            summary.realism.failures.join("; ")
        );
        assert!(summary.realism.checks.len() >= 10);
    }

    #[test]
    fn dry_run_rejects_invalid_parameters() {
        let mut invalid = params();
        invalid.dur = 0;
        let error = run_dry_run(&invalid, Coordinate::new(39.9, 116.4, 0.0).unwrap()).unwrap_err();
        assert!(error.contains("时长"));
    }
}

/// 记录列表（UI 线程用）。
pub fn run_records(
    client: &mut ApiClient,
    log: &mut dyn FnMut(&str),
) -> Result<Vec<super::records::RecordRow>, String> {
    log("[records] 拉取跑步记录…");
    let rows = super::records::fetch_records(client)?;
    log(&format!("√ [records] {} 条记录", rows.len()));
    Ok(rows)
}

use chrono::TimeZone as _;

/// 最近一次详情验证的完整响应（达标判定明细在 reasonList）。
pub static VERIFY_DETAIL: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// 按相邻定位点弦长累加的路径长度（米）。
///
/// 服务端拿提交的经纬度换算里程用的就是这个量，而不是我们内部生成时沿曲线
/// 走过的弧长。两者在 2 km 上差 2% 左右，用它可以提前判断这条轨迹的距离
/// 自洽度会不会被互校拦下。
fn chord_path_len(track: &crate::track::model::Track) -> f64 {
    use crate::track::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};
    let locs = &track.locations;
    if locs.len() < 2 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 1..locs.len() {
        let (plat, plng) = (locs[i - 1].gLat, locs[i - 1].gLng);
        let (lat, lng) = (locs[i].gLat, locs[i].gLng);
        acc += ((lat - plat) * MET_PER_DEG_LAT).hypot((lng - plng) * MET_PER_DEG_LNG);
    }
    acc
}
