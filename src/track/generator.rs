//! 自然轨迹生成器。
//!
//! 两种几何入口（打卡点拟合环 / 标准跑道）共用同一个组装器
//! [`assemble`]：先由各入口给出「沿场地路线的前进量」，再由 [`assemble`] 把前进量换算
//! 成带横向漂移、带进场/离场段的坐标，并同时产出协议字段。
//!
//! 速度、步频、`speed` 字段与海拔不再各自独立随机，而是由同一份逐点位移导出：
//!
//! ```text
//! 位移(m)  → 瞬时速度(m/s) → 步频(步/分) 且 速度 = 步频/60 × 步幅
//!          → speed 字段 = 位移/时间差 × (1+噪声)      （km/h）
//!          → 累计距离 / 累计步数 / 累计时间 三者同源
//! ```
//!
//! 采样点落在「场地路线 + 缓慢漂移的横向偏移」上，而不是解析闭合曲线上；起点另有
//! 进场段、终点另有离场段。真实性的机器读数由 `track::realism::audit` 给出。
#![allow(non_snake_case)]

use super::biomech::{fatigue_from_km, gait, ou_cadence_series};
use super::geom::{fmt_gain_time, make_point_ring, round_to, Rng};
use super::model::{GenPoint, Segment, TenWindow, Track};
use super::postfix::apply_post_fixes;
use super::realism::{fit_speeds_capped, sample_grid};
use super::shape::{site_ring_with_fences, ShapeTracker, ShapedPlan, SiteRing};
use super::stadium::TrackSpec;

/// 有效配速窗口（判定规则 2'21"-10'00"/km ≈ 1.667-7.092 m/s），硬边界留余量。
pub const SPEED_FLOOR: f64 = 1.90;
pub const SPEED_CEIL: f64 = 6.30;
/// 速度字段的测量噪声（相对标准差）。
///
/// 真机 `speed` 是定位芯片给的瞬时速度，与「本段位移/时间差」的比值实测落在
/// 0.64–1.26；取 5% 会让这条比值几乎恒等于 1，反而是机器特征。
pub const SPEED_FIELD_MEAN_NOISE: f64 = 0.18;
pub const SPEED_FIELD_MIN_NOISE: f64 = 0.05;
/// 速度字段读数为 0 的概率：芯片给不出速度时就是 0.0（静止 / 刚启动 / 被遮挡）。
///
/// 实测一条 918 s 的记录里 193 点有 37 个 `speed == 0`（约 19%）。
pub const SPEED_FIELD_ZERO_P: f64 = 0.16;
/// 速度字段短时掉值（隧道 / 建筑遮挡）的概率与替代值。
pub const SPEED_FIELD_DROP_P: f64 = 0.02;
pub const SPEED_FIELD_DROP_KMH: (f64, f64) = (0.9, 2.2);
/// 海拔随里程的起伏量（米 / km）。
pub const ALT_UNDULATION_M_PER_KM: f64 = 2.4;
/// 单趟跑步里跑者离开路线再回来（喝水 / 系鞋带）的概率。
pub const SIDE_TRIP_P: f64 = 0.10;

/// 等比缩放逐点速度至目标总距：越界点钳在窗口边界，剩余差量由未饱和点分摊。
fn fit_speeds(w: &mut [f64], dts: &[f64], target: f64) {
    fit_speeds_capped(w, dts, target, SPEED_FLOOR, SPEED_CEIL);
}

/// 采样时刻：`grid[0] = 0`，其余为 5 s 为主的非均匀步长，末点为 `dur`。
pub(crate) fn interior_grid(dur_f: f64, rng: &mut Rng) -> (Vec<f64>, Vec<f64>) {
    sample_grid(dur_f, rng)
}

/// 经典速度剖面：5s 主采样时间网格 + ramp×疲劳×三正弦×余弦凹陷×微噪速度曲线，
/// 逐点速度约束在有效配速窗口内并精确命中目标距离。返回 (times, dts, speeds)。
pub(crate) fn speed_profile(dist: f64, dur: f64, rng: &mut Rng) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let dur_f = dur;
    let phase_v = rng.uniform(0.0, std::f64::consts::TAU);
    let (times, dts) = interior_grid(dur_f, rng);
    let n = times.len();

    let n_dips = rng.choice(&[1, 1, 2]);
    let mut dips = Vec::new();
    for _ in 0..n_dips {
        dips.push((
            rng.uniform(0.15, 0.75) * dur_f,
            rng.uniform(25.0, 55.0),
            rng.uniform(0.08, 0.16),
        ));
    }
    let dip_factor = |t: f64| -> f64 {
        let mut f = 1.0;
        for &(c, hw, d) in &dips {
            if (t - c).abs() < hw {
                f *= 1.0 - d * 0.5 * (1.0 + std::f64::consts::PI * (t - c) / hw).cos();
            }
        }
        f
    };
    // 首点（t=0）与起点哨兵（索引 1）是「设备开始记录的那一拍」，速度与位移都为 0；
    // 真正迈出的第一步发生在索引 2。
    let base = dist / dur_f.max(1e-9);
    let mut w = Vec::with_capacity(n);
    for (i, &tt) in times.iter().enumerate() {
        if i < 2 {
            w.push(0.0);
            continue;
        }
        let ramp = if tt < 8.0 {
            (0.85 + 0.15 * (tt / 1.0f64.max(8.0f64.min(dur_f / 20.0)))).min(1.0)
        } else {
            1.0
        } * if tt > dur_f - 8.0 {
            1.0 + 0.03 * (tt - (dur_f - 8.0)) / 8.0
        } else {
            1.0
        };
        let km_done = (tt / dur_f) * dist / 1000.0;
        let fatigue = if km_done <= 0.5 {
            1.03
        } else {
            (1.03 - 0.06 * (km_done - 0.5)).max(0.80)
        };
        let wave = (1.0
            + 0.010 * (std::f64::consts::TAU * tt / 115.0 + phase_v).sin()
            + 0.035 * (std::f64::consts::TAU * tt / 47.0 + phase_v * 2.3).sin()
            + 0.015 * (std::f64::consts::TAU * tt / 19.0 + phase_v * 3.7).sin())
            * dip_factor(tt);
        let noise = 1.0 + rng.gauss(0.0, 0.008);
        w.push(base * ramp * fatigue * wave * noise);
    }
    // 末点的位移由收尾段统一给出，不参与距离分摊。
    let last = n - 1;
    let mut interior: Vec<f64> = w[2..last].to_vec();
    let interior_dt: Vec<f64> = dts[2..last].to_vec();
    fit_speeds(&mut interior, &interior_dt, dist);
    for (i, v) in interior.into_iter().enumerate() {
        w[i + 2] = v;
    }
    (times, dts, w)
}

/// 轨迹生成主入口。points_bd 为 BD 系打卡点。
pub fn build(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
) -> Track {
    build_with_fences(dist, dur, seed, _center, start_ms, points_bd, &[])
}

#[cfg_attr(not(test), allow(dead_code))] // 仅供测试断言使用
pub const MAX_EXCEPTION_GPS_SPEED_KMH: f64 = 6.0;
/// Maximum coordinate drift for a protocol exception point.
///
/// These points may still carry their original type marker, but they must not
/// create a map-scale jump that the server interprets as an invalid route.
#[cfg_attr(not(test), allow(dead_code))] // 仅供测试断言使用
pub const MAX_EXCEPTION_DRIFT_M: f64 = 6.0;
/// Generate a route along a selected standard athletics track.
#[cfg_attr(not(test), allow(dead_code))] // 仅供测试使用
pub fn build_stadium(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
    spec: TrackSpec,
) -> Result<Track, String> {
    build_stadium_with_fences(dist, dur, seed, _center, start_ms, points_bd, spec, &[])
}

/// 同 [`build_stadium`]，额外用电子围栏把跑道直道摆正到场地长边方向。
pub fn build_stadium_with_fences(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
    spec: TrackSpec,
    fences: &[Vec<(f64, f64)>],
) -> Result<Track, String> {
    let mut rng = Rng::new(seed);
    let mut ring = site_ring_with_fences(points_bd, spec, fences)?;
    // `Auto` 时按目标距离选场地规模：短距离用 400 m 标准跑道，长距离换更大的外围
    // 路线。让人在 400 m 跑道上跑 20 圈既不像真人，也会把一圈挤进上百个采样点。
    if spec == TrackSpec::Auto {
        let wanted = if dist > 3000.0 {
            ((dist / 4.5).clamp(800.0, 2000.0) / 100.0).round() * 100.0
        } else {
            400.0
        };
        if (ring.ring_m - wanted).abs() > wanted * 0.25 {
            let center = ring.center;
            if let Ok((dense, arcs, c)) = super::stadium::make_stadium_ring_with_fences(
                &[center, (center.0 + 1e-6, center.1)],
                TrackSpec::Custom {
                    total_m: wanted.round().clamp(200.0, 1000.0) as u32,
                },
                true,
                fences,
            ) {
                let rm = arcs.last().copied().unwrap_or(wanted);
                ring = SiteRing {
                    dense,
                    arcs,
                    center: c,
                    ring_m: rm,
                    genuine: true,
                };
            }
        }
    }
    // 跑道语义：距离按整圈取（真人跑操场跑的是整圈，不会停在弯道中间）。
    let ring_m = ring.ring_m;
    let laps = (dist / ring_m).max(1.0).round();
    let dist = laps * ring_m;
    let (times, dts, w) = speed_profile(dist, dur as f64, &mut rng);
    Ok(assemble(Assembly {
        dist,
        dur_s: dur as f64,
        start_ms,
        seed,
        ring,
        times,
        dts,
        speeds: w,
        snap_points_bd: points_bd,
        snap_radius_m: 40.0,
        // 有围栏 = 知道场地边界 = 人已经在场内，直接从路线原点起跑，
        // 不再造「进场段 / 离场段」（旧实现会让首尾点跑到围栏外十几米）。
        start_at_ring_origin: !fences.is_empty(),
        loop_m: None,
        rng,
    }))
}

/// 生成轨迹；`fences` 为电子围栏，用于把跑道直道摆正到场地长边方向。
pub fn build_with_fences(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
    fences: &[Vec<(f64, f64)>],
) -> Track {
    let mut rng = Rng::new(seed);
    let mut ring = site_ring_with_fences(points_bd, TrackSpec::Auto, fences)
        .unwrap_or_else(|_| fallback_ring(points_bd));
    if dist > 3000.0 {
        let wanted = ((dist / 4.5).clamp(800.0, 2000.0) / 100.0).round() * 100.0;
        if (ring.ring_m - wanted).abs() > wanted * 0.25 {
            let center = ring.center;
            if let Ok((dense, arcs, c)) = super::stadium::make_stadium_ring_with_fences(
                &[center, (center.0 + 1e-6, center.1)],
                TrackSpec::Custom {
                    total_m: wanted.round().clamp(200.0, 1000.0) as u32,
                },
                true,
                fences,
            ) {
                let rm = arcs.last().copied().unwrap_or(wanted);
                ring = SiteRing {
                    dense,
                    arcs,
                    center: c,
                    ring_m: rm,
                    genuine: true,
                };
            }
        }
    }
    let (times, dts, w) = speed_profile(dist, dur as f64, &mut rng);
    assemble(Assembly {
        dist,
        dur_s: dur as f64,
        start_ms,
        seed,
        ring,
        times,
        dts,
        speeds: w,
        snap_points_bd: points_bd,
        snap_radius_m: 40.0,
        // 同上：有围栏就从环线原点起跑，全程留在场内。
        start_at_ring_origin: !fences.is_empty(),
        loop_m: None,
        rng,
    })
}

/// 所有几何入口共用的组装器入参。
pub(crate) struct Assembly<'a> {
    pub dist: f64,
    pub dur_s: f64,
    pub start_ms: i64,
    pub seed: u64,
    pub ring: SiteRing,
    /// 采样时刻（秒，从 0 起）。
    pub times: Vec<f64>,
    /// 每个点的前进时长（秒）。
    pub dts: Vec<f64>,
    /// 逐点目标速度（米/秒），长度与 `times` 一致。
    pub speeds: Vec<f64>,
    /// 需要吸附的打卡点（BD）。
    pub snap_points_bd: &'a [(f64, f64)],
    /// 吸附半径（米）。
    #[allow(dead_code)] // 装配参数留档：当前吸附半径由常量 SNAP_RADIUS_M 给出
    pub snap_radius_m: f64,
    /// true = 起点固定在场路线原点（跑道语义）。
    pub start_at_ring_origin: bool,
    pub rng: Rng,
    /// 本次实际沿路线跑到多远（米）。`None` = 由形态参数按目标距离换算。
    pub loop_m: Option<f64>,
}

/// 兜底环：打卡点不足 3 个时用质心处的标准跑道。
fn fallback_ring(points_bd: &[(f64, f64)]) -> SiteRing {
    let center = if points_bd.is_empty() {
        (0.0, 0.0)
    } else {
        let n = points_bd.len() as f64;
        (
            points_bd.iter().map(|p| p.0).sum::<f64>() / n,
            points_bd.iter().map(|p| p.1).sum::<f64>() / n,
        )
    };
    let (dense, arcs, center) = make_point_ring(&[
        (center.0 + 0.0009, center.1 - 0.0011),
        (center.0 + 0.0009, center.1 + 0.0011),
        (center.0 - 0.0009, center.1 + 0.0011),
        (center.0 - 0.0009, center.1 - 0.0011),
    ]);
    SiteRing {
        ring_m: arcs.last().copied().unwrap_or(400.0),
        dense,
        arcs,
        center,
        genuine: false,
    }
}

fn dir(p: &(f64, f64), q: &(f64, f64)) -> f64 {
    ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt()
}

/// 把「逐点前进速度」组装成一条完整轨迹。
pub(crate) fn assemble(a: Assembly<'_>) -> Track {
    let Assembly {
        dist,
        dur_s,
        start_ms,
        seed,
        ring,
        times,
        dts,
        mut speeds,
        snap_points_bd,
        snap_radius_m: _,
        start_at_ring_origin,
        loop_m,
        mut rng,
    } = a;

    let n = times.len();
    let target_time = dur_s.round() as i64;
    // ① 逐点前进距离精确命中目标总距（越界点钳在有效窗口内）。
    //    `dts[i]` 是「从 i-1 到 i」的时长。
    speeds.resize(n, 1.0);
    for w in speeds.iter_mut() {
        if !w.is_finite() || *w <= 0.0 {
            *w = dist / dur_s.max(1.0);
        }
    }
    let step_dt = dts.clone();
    let mut interior: Vec<f64> = speeds[2..n - 1].to_vec();
    let mut interior_dt: Vec<f64> = step_dt[2..n - 1].to_vec();
    fit_speeds_capped(
        &mut interior,
        &mut interior_dt,
        dist,
        SPEED_FLOOR,
        SPEED_CEIL,
    );
    for (i, v) in interior.into_iter().enumerate() {
        speeds[i + 2] = v;
    }
    speeds[0] = 0.0;
    speeds[1] = 0.0;
    let step_m: Vec<f64> = (0..n)
        .map(|i| if i < 2 { 0.0 } else { speeds[i] * step_dt[i] })
        .collect();

    // ② 形态：进场段 + 绕圈 + 离场段（跑道语义下全程都在路线上）
    let mut plan =
        ShapedPlan::with_access(dist, dur_s, ring.ring_m, &mut rng, !start_at_ring_origin);
    // `loop_m` 必须等于「实际要跑的距离」：形态参数按周长估的圈数在这里没意义
    // （打卡点折线可能只有几十米一圈），否则绕圈段会把速度剖面整体拉低。
    plan.loop_m = loop_m.unwrap_or(dist - plan.entry_m - plan.exit_m).max(1.0);
    // 起点选在「离最近的必过打卡点最近」的位置：真人从最近的方向走向打卡点，
    // 而不是从场地中央出发。
    let start_s = if start_at_ring_origin {
        0.0
    } else {
        nearest_arc(&ring, snap_points_bd, &mut rng)
    };
    if plan.entry_m > 0.0 && !start_at_ring_origin {
        plan.entry_m = plan.entry_m.min(ring.ring_m * 0.10).max(15.0);
    }
    let mut tracker = ShapeTracker::new(
        plan.clone(),
        ring.dense.clone(),
        ring.arcs.clone(),
        &mut rng,
        start_at_ring_origin,
        Some(start_s),
    );
    // 环线弧长与前进量是同一把尺子：tracker 按 `step_m` 推进弧长，坐标由弧长插值，
    // 因此「实际走过的距离」永远等于 `step_m` 之和，不需要再做几何拉伸校准。
    // ③ 点位类型：路线颜色固定全绿色，所有点归一为普通点（type 0 / state 1）。
    //    官方便携详情页据此把整条路线画成绿色。
    let kinds: Vec<(i64, i64)> = vec![(0, 1); n];

    // ④ 生物力学：步频在「由速度与疲劳算出的目标值」附近 OU 波动，步幅由
    //    v = cad/60 × stride 反解，因此速度、步频、步幅三者恒等自洽。
    let targets: Vec<f64> = (0..n)
        .map(|i| {
            let km_done = (times[i] / dur_s.max(1.0)) * dist / 1000.0;
            gait(speeds[i], fatigue_from_km(km_done)).cadence
        })
        .collect();
    let cadence_seed = seed.wrapping_add(0xC6A4_A793_5BD1_E995);
    let cadences = ou_cadence_series(&targets, &step_dt, cadence_seed, 0.25, 0.5, 110.0, 220.0);

    // ⑤ 逐点组装坐标（累计量在 ⑥ 里按实测几何统一重算）
    let mut locs: Vec<GenPoint> = Vec::with_capacity(n);
    let alt0 = ground_altitude(ring.center);
    let mut side_trip_left = if rng.random() < SIDE_TRIP_P && n > 12 {
        rng.randint(2, 4)
    } else {
        0
    };
    let mut prev_raw = tracker.ring_origin();
    // 定位精度：围绕本次会话基准缓慢游走（相邻点几乎相同）。
    let radius_mean = rng.uniform(1.5, 2.3);
    let mut radius_now = radius_mean;
    // 卫星可见性是**位置的函数**，不是时间的函数：同一段路第二次跑过时读数应当相似，
    // 因为挡住天空的还是那几栋楼。用几个不同波长的正弦叠加成一个确定性空间场，
    // 逐点独立随机（或纯时间 OU）都做不到"绕回同一处读数又回来"这个性质。
    // 波长取 270–570 m，对 400 m 一圈来说是每圈起伏数次，符合建筑间距的量级。
    let sat_field = {
        let p1 = rng.uniform(0.0, std::f64::consts::TAU);
        let p2 = rng.uniform(0.0, std::f64::consts::TAU);
        let p3 = rng.uniform(0.0, std::f64::consts::TAU);
        move |x: f64, y: f64| -> f64 {
            let f = 0.9 * (x * 0.011 + p1).sin()
                + 0.7 * (y * 0.017 + p2).sin()
                + 0.5 * ((x + y) * 0.023 + p3).sin();
            (f / 2.1).clamp(-1.0, 1.0)
        }
    };
    let count_mean = rng.uniform(20.0, 30.0);
    let count_span = rng.uniform(5.0, 9.0);
    for i in 0..n {
        let mut point = if i <= 1 {
            tracker.advance(0.0, 0.0, &mut rng)
        } else {
            tracker.advance(step_m[i], step_dt[i], &mut rng)
        };
        if side_trip_left > 0 && i > 3 && i + 3 < n {
            // 短暂离开路线再回来（喝水 / 系鞋带）：只做**横向**微偏移（垂直于前进
            // 方向），位移增量就是偏移量本身，不会在相邻两点之间制造瞬移。
            side_trip_left -= 1;
            let off = rng.uniform(0.35, 0.8);
            let (dx, dy) = (point.0 - prev_raw.0, point.1 - prev_raw.1);
            let norm = (dx * dx + dy * dy).sqrt().max(1e-9);
            if norm > 1e-6 {
                let side = if rng.random() < 0.5 { 1.0 } else { -1.0 };
                point = (
                    point.0 + side * off * (-dy / norm),
                    point.1 + side * off * (dx / norm),
                );
            }
        }
        prev_raw = point;
        let (lat, lng) = ring.to_bd(point.0, point.1);
        let (typ, lt) = kinds[i];
        // 定位精度缓慢游走；卫星数由空间场决定（同一段路读数可复现）。
        radius_now = (radius_now - radius_mean) * 0.85 + radius_mean + rng.gauss(0.0, 0.22);
        radius_now = radius_now.clamp(1.2, if typ == 3 { 6.5 } else { 3.2 });
        let count_now = count_mean + count_span * sat_field(point.0, point.1) + rng.gauss(0.0, 1.2);
        let count_now = count_now.clamp(8.0, 44.0);
        let rad = round_to(radius_now, 2);
        let count = round_to(count_now, 0) as i64;
        let state = 1;

        locs.push(GenPoint {
            id: i as i64 + 1,
            flag: start_ms,
            lat: -1.0,
            lng: -1.0,
            gLat: round_to(lat, 7),
            gLng: round_to(lng, 7),
            speed: 0.0,
            avgSpeed: 0.0,
            radius: rad,
            accuracy: rad,
            ptype: typ,
            locType: lt,
            hasAltitude: true,
            totalTime: times[i].round() as i64,
            totalDis: 0.0,
            validDis: 0.0,
            validTime: times[i].round() as i64,
            steps: 0,
            stepDistance: 0.0,
            gainTime: fmt_gain_time(start_ms + (times[i] * 1000.0) as i64),
            gainTimeMs: start_ms + (times[i] * 1000.0) as i64,
            queueNum: 0,
            coorType: "gcj02".into(),
            bdA: round_to(alt0, 2),
            bdD: 0.0,
            bdS: round_to(rng.uniform(0.05, 0.35), 3),
            bdG: 1,

            count,
            dtr: 0.0,
            state,
            locationId: String::new(),
        });
    }

    // ⑥ 以**实测几何**为准派生累计量：横向漂移、进场绕行与吸附都会让实际位移与规划
    //    值有出入；这里让 totalDis / avgSpeed / speed 字段 / 累计步数 / 海拔全部由同
    //    一条位移序列导出。步数与位移必须同源：逐段位移 ÷ 全局步幅，小数部分进
    //    carry，所以「少了个采样点」不会凭空少几十步。
    // ⑥ 派生累计量：位移、步数、速度字段、海拔全部由同一条「逐段前进量」导出。
    //
    //    前进量取 `step_m[i]`（沿路线真实走过的距离），不是「两点坐标直线距离」——
    //    后者在拐弯处会比实际走过的短，累积下来会让某几分钟的累计距离停滞，服务端
    //    按 10 秒窗一读就是「配速突然慢一倍」。步数按「位移 ÷ 全局步幅」分配，
    //    小数部分进 carry，因此少一个采样点不会凭空少几十步。
    let raw_path: f64 = (2..n).map(|i| step_m[i]).sum();
    let steps_total = cadences
        .iter()
        .zip(step_dt.iter())
        .skip(2)
        .map(|(c, dt)| c / 60.0 * dt)
        .sum::<f64>()
        .max(1.0);
    let mean_stride = if raw_path > 1.0 {
        (raw_path / steps_total).clamp(0.70, 1.35)
    } else {
        1.10
    };
    let mut acc_steps = 0.0f64;
    let mut carry = 0.0f64;
    locs[0].steps = 0;
    if n > 1 {
        locs[1].steps = 0;
    }
    // 计步器口径的累计步幅距离：Σ(本段步数 × 全局步幅估计)。
    //
    // `stepDistance` 在官方点结构里是**累计量**（`OooO00o.java` 用两点相减得到
    // 一个窗口的步幅距离），不是每段值。它约等于 `totalDis`，但因为用的是全局步幅
    // 估计而不是本段实测步幅，逐点会有几百米的缓慢漂移——这正是计步器读数的样子。
    // 全 0 是最容易被抓的破绽之一：真人跑 2 km，计步器不可能一米都没记。
    let mut sd_acc = 0.0f64;
    for i in 2..n {
        let exact = step_m[i] / mean_stride + carry;
        let ds = exact.floor();
        carry = exact - ds;
        acc_steps += ds;
        locs[i].steps = acc_steps as i64;
        sd_acc += ds * mean_stride;
        locs[i].stepDistance = round_to(sd_acc, 4);
    }
    let mut cum = 0.0f64;
    let mut alt = alt0;
    for i in 0..n {
        if i >= 2 {
            cum += step_m[i];
        }
        // 起点哨兵与首点同址：累计距离必须为 0（协议语义）。
        let cum_i = if i <= 1 { 0.0 } else { cum };
        let dt = if i <= 1 {
            1.0
        } else {
            (locs[i].totalTime - locs[i - 1].totalTime).max(1) as f64
        };
        let seg = if i < 2 { 0.0 } else { step_m[i] };
        locs[i].totalDis = round_to(cum_i, 4);
        locs[i].validDis = round_to(cum_i, 4);
        locs[i].avgSpeed = if i <= 1 {
            round_to(speeds[2.min(n - 1)], 4)
        } else {
            round_to(seg / dt, 4)
        };
        // `speed` 是**GPS 瞬时速度**（km/h），不是本段平均速度的精确换算：
        // 真机读数有三件事必须体现，否则和位移/时间差的相关性会"过于完美"。
        //
        // * 约 18% 的点读数为 0——定位芯片给不出速度（静止、刚开始、被遮挡）时
        //   就是 0.0，而不是"慢"；
        // * 其余点带 ±18% 的相对噪声（实测带宽约 0.64–1.26）；
        // * 偶发极低的掉值（建筑遮挡）。
        let kmh = seg / dt * 3.6;
        locs[i].speed = if i == 0 {
            round_to(kmh, 4)
        } else if rng.random() < SPEED_FIELD_ZERO_P {
            0.0
        } else if rng.random() < SPEED_FIELD_DROP_P {
            round_to(
                rng.uniform(SPEED_FIELD_DROP_KMH.0, SPEED_FIELD_DROP_KMH.1),
                4,
            )
        } else {
            let sigma = (kmh * SPEED_FIELD_MEAN_NOISE).max(SPEED_FIELD_MIN_NOISE);
            round_to((kmh + rng.gauss(0.0, sigma)).max(0.3), 4)
        };
        // 海拔：起伏必须按**圈**周期，闭环每圈回到同一高度、全程净漂移为 0。
        //
        // 旧实现用 `cum_i / 1000 × 1.6` 做累积爬升（每公里 +3.84 m）：跑 5 圈 400 m
        // 会凭空升高 8 m，海拔曲线是一条单调上升的斜坡——操场是平的，这是明显的假。
        // 现在主项按 `ring_m`（一圈周长）取正弦，次项用两倍频补细节，起点与终点等高。
        let lap_len = ring.ring_m.max(1.0);
        let lap_phase = (cum_i / lap_len) * std::f64::consts::TAU;
        let phase = (lap_phase * 2.0 + 0.7 * speeds[i]).sin();
        let target_alt = alt0 + ALT_UNDULATION_M_PER_KM * (0.45 * lap_phase.sin() + 0.20 * phase);
        alt += 0.35 * (target_alt - alt) + rng.gauss(0.0, 0.12);
        locs[i].bdA = round_to(alt, 2);
        // 航向：本段前进方向 + 抖动
        let (dx, dy) = if i + 1 < n {
            (
                locs[i + 1].gLng - locs[i].gLng,
                locs[i + 1].gLat - locs[i].gLat,
            )
        } else {
            (0.0, 0.0)
        };
        locs[i].bdD = round_to(
            (dx.atan2(dy).to_degrees() + rng.gauss(0.0, 6.0)).rem_euclid(360.0),
            2,
        );
    }
    // 轨迹点坐标与累计距离必须同源：`step_m` 是沿路线走过的长度，坐标弦长通常略短，
    // 一致性校验用它自己的容差（见 `realism::audit`）。

    // ⑦a 收尾：真人冲线后会再走几步才停下。末点的位移按「收尾配速 × 末段时长」
    //     给出（末段时长直接取采样网格给的 dt，不改时间轴），因此末段仍然是
    //     「人真的走了这么远、花了这么久」。
    if n > 3 {
        let last = n - 1;
        let stop_speed = rng.uniform(
            super::postfix::STOP_SPEED_RANGE.0,
            super::postfix::STOP_SPEED_RANGE.1,
        );
        let dt = (locs[last].totalTime - locs[last - 1].totalTime).clamp(1, 30) as f64;
        let p = tracker.advance(stop_speed * dt, dt, &mut rng);
        let (lat, lng) = ring.to_bd(p.0, p.1);
        locs[last].gLat = round_to(lat, 7);
        locs[last].gLng = round_to(lng, 7);
        locs[last].speed = round_to(stop_speed * 3.6, 4);
    }

    // ⑦b 哨兵点 / 首末点协议（时间轴由采样网格给出，这里只管类型位）
    apply_post_fixes(&mut locs, &mut rng, start_ms);
    {
        let last_index = locs.len().saturating_sub(1);
        for (i, point) in locs.iter_mut().enumerate() {
            point.ptype = if i == 1 {
                5
            } else if i == last_index {
                6
            } else {
                0
            };
            point.locType = 1;
            point.state = 1;
            point.bdG = 1;
        }
    }

    // ⑧ 点位吸附：仅在「本来就在几何上贴着这个点」时补齐末位精度。
    //
    //    不做大范围吸附——把 39 m 外的采样点直接搬到打卡点上，必然在相邻两点之间
    //    造出一次 19 m/s 的瞬移。必过点已由 `site_ring` 把打卡点连成路线覆盖，
    //    这里只负责把「本来就在点上」的采样点对齐到协议精度。
    let _ = snap_points_with_cost(&mut locs, snap_points_bd, 2.5);

    // ⑨ 速度字段收口：`avgSpeed` 严格等于「本段位移 / 本段时间差」，
    //    `speed` 是 GPS 瞬时速度（km/h），按真机读数特征给出。
    //
    //    这是 `speed` 唯一的写入点——前面若也算一遍，后面的收口会把它覆盖掉，
    //    掉零与噪声就白设了。
    let n_len = locs.len();
    for i in 2..n_len {
        let dt = (locs[i].totalTime - locs[i - 1].totalTime).max(1) as f64;
        let seg = dir(&locs[i - 1].coords(), &locs[i].coords());
        locs[i].avgSpeed = round_to(seg / dt, 4);
        let kmh = seg / dt * 3.6;
        locs[i].speed = if rng.random() < SPEED_FIELD_ZERO_P {
            // 定位芯片给不出速度时读数为 0.0（静止 / 刚启动 / 被遮挡）。
            0.0
        } else if rng.random() < SPEED_FIELD_DROP_P {
            round_to(
                rng.uniform(SPEED_FIELD_DROP_KMH.0, SPEED_FIELD_DROP_KMH.1),
                4,
            )
        } else {
            let sigma = (kmh * SPEED_FIELD_MEAN_NOISE).max(SPEED_FIELD_MIN_NOISE);
            round_to((kmh + rng.gauss(0.0, sigma)).max(0.3), 4)
        };
    }

    let mut track = Track {
        totalTime: target_time,
        totalDistance: 1.0,
        validDistance: 1.0,
        validTime: target_time,
        startTime: start_ms,
        startLatitude: locs[0].gLat,
        startLongitude: locs[0].gLng,
        totalSteps: locs[n_len - 1].steps.max(1),
        locations: locs,
        speedPerTenSec: Vec::new(),
        stepsPerTenSec: Vec::new(),
        segments: Vec::new(),
    };
    finalize_totals(&mut track);
    track
}

impl GenPoint {
    /// 平面坐标（米，相对量均可）——仅用于两点距离，坐标系一致即可。
    fn coords(&self) -> (f64, f64) {
        use super::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};
        (self.gLng * MET_PER_DEG_LNG, self.gLat * MET_PER_DEG_LAT)
    }
}

/// 起点在环线上的弧长位置：离「最近的必过打卡点」最近的那一段路线。
fn nearest_arc(ring: &SiteRing, points_bd: &[(f64, f64)], rng: &mut Rng) -> f64 {
    if points_bd.is_empty() || ring.dense.is_empty() {
        return rng.uniform(0.0, ring.ring_m.max(1.0));
    }
    let mut best = (f64::INFINITY, 0.0f64);
    for (i, &p) in ring.dense.iter().enumerate() {
        let mut nearest = f64::INFINITY;
        for &(plat, plng) in points_bd {
            let d = ring.distance_to(p.0, p.1, (plat, plng));
            if d < nearest {
                nearest = d;
            }
        }
        if nearest < best.0 {
            best = (nearest, *ring.arcs.get(i).unwrap_or(&0.0));
        }
    }
    // 在最近点附近留 ±8 m 的随机（真人不会每天踩同一点）。
    let total = ring.ring_m.max(1.0);
    (best.1 + rng.uniform(-8.0, 8.0)).rem_euclid(total)
}

/// 地面海拔：由纬度粗估，作为海拔曲线的基线。
fn ground_altitude(_center: (f64, f64)) -> f64 {
    // 校园地面海拔与纬度没有解析关系；这里给一个中性基线，真实值由用户手动海拔覆盖。
    82.0
}

/// 把打卡点吸附到最近采样点，但**只有当走过去的代价可以接受时才落位**。
///
/// 旧实现无条件改写坐标：最近点离打卡点 39 m、而该点的采样间隔只有 2 s 时，相邻段
/// 瞬时速度会到 19 m/s。这里把「改写后多出来的位移」沿前后两个采样间隔摊开。
fn snap_points_with_cost(locs: &mut [GenPoint], points: &[(f64, f64)], radius_m: f64) -> usize {
    use super::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};
    let seg_to = |locs: &[GenPoint], k: usize| -> f64 {
        ((locs[k].gLat - locs[k - 1].gLat) * MET_PER_DEG_LAT)
            .hypot((locs[k].gLng - locs[k - 1].gLng) * MET_PER_DEG_LNG)
    };
    let mut snapped = 0usize;
    for &(plat, plng) in points {
        let mut best: Option<(usize, f64)> = None;
        for (i, q) in locs.iter().enumerate() {
            if i < 2 || i + 2 >= locs.len() {
                continue;
            }
            let d = ((q.gLat - plat) * MET_PER_DEG_LAT).hypot((q.gLng - plng) * MET_PER_DEG_LNG);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((i, d));
            }
        }
        let Some((i, gap)) = best else { continue };
        if gap > radius_m {
            continue;
        }
        let d_prev_old = seg_to(locs, i);
        let d_next_old = seg_to(locs, i + 1);
        let shift_prev = ((plat - locs[i - 1].gLat) * MET_PER_DEG_LAT)
            .hypot((plng - locs[i - 1].gLng) * MET_PER_DEG_LNG);
        let shift_next = ((locs[i + 1].gLat - plat) * MET_PER_DEG_LAT)
            .hypot((locs[i + 1].gLng - plng) * MET_PER_DEG_LNG);
        let extra = (shift_prev - d_prev_old).max(0.0) + (shift_next - d_next_old).max(0.0);
        // 这段位移能多走的预算：按相邻速度能在一分钟内多走的距离。
        let budget = locs[i].avgSpeed.max(2.0) * 12.0 * 0.6;
        if extra > budget {
            // 走不过去：保留原坐标。跑步页会提示该点未被命中，这是可解释的结果。
            continue;
        }
        locs[i].gLat = round_to(plat, 7);
        locs[i].gLng = round_to(plng, 7);
        if extra > 0.0 {
            let half = extra * 0.5;
            for (k, dv) in [(i, half), (i + 1, half)] {
                let d = seg_to(locs, k);
                if d > 1e-6 && dv > 0.4 {
                    let ratio = (d + dv) / d;
                    let (lat0, lng0) = (locs[k - 1].gLat, locs[k - 1].gLng);
                    locs[k].gLat = round_to(lat0 + (locs[k].gLat - lat0) * ratio, 7);
                    locs[k].gLng = round_to(lng0 + (locs[k].gLng - lng0) * ratio, 7);
                }
            }
        }
        snapped += 1;
    }
    snapped
}

/// 收尾：把顶层总计与逐点累计对齐，并重建 10s 窗 / 60s 段。
///
/// 生成器与 `postfix` 会分别改写坐标、时间与步数；这里做一次统一收口，保证
/// 「提交体、OBS、详情页」三方读到同一组数。
pub(crate) fn finalize_totals(track: &mut Track) {
    if track.locations.is_empty() {
        return;
    }
    // 累计距离在生成阶段已按「沿路线前进量」给出，这里不再按坐标弦长重算。
    let Some(last) = track.locations.last().cloned() else {
        return;
    };
    track.totalTime = last.totalTime.max(1);
    track.validTime = track.totalTime;
    track.totalSteps = last.steps.max(1);
    // 声明距离 = 按坐标实测的路径长度：与逐点累计同源，容差 0。
    track.totalDistance = round_to(last.totalDis.max(1.0), 3);
    track.validDistance = track.totalDistance;
    if let Some(first) = track.locations.first().cloned() {
        track.startLatitude = first.gLat;
        track.startLongitude = first.gLng;
    }
    // 10s 窗按累计量差分重建（边界线性插值），保证窗内距离之和 = 总距离。
    let (sp, stf) = rebuild_windows(track);
    track.speedPerTenSec = sp;
    track.stepsPerTenSec = stf;
    track.segments = rebuild_segments(track);
}

fn rebuild_windows(track: &Track) -> (Vec<TenWindow>, Vec<TenWindow>) {
    // 窗口只由 `wire::ten_second_spans` 定义一次，提交体和 OBS 都读它。
    //
    // 协议里 `speedPerTenSec[].time` 恒为 10，但末窗可能就是不足 10 秒的残窗；
    // 服务端按 `endTime - beginTime` 读实际时长，因此这里如实保留残窗，而不是
    // 把它并进上一窗——并窗会让窗口总时长与 `totalTime` 对不上。
    let spans = super::wire::ten_second_spans(track);
    let mut sp: Vec<TenWindow> = Vec::with_capacity(spans.len());
    let mut stf: Vec<TenWindow> = Vec::with_capacity(spans.len());
    for span in spans {
        sp.push(TenWindow {
            time: 10,
            value: round_to(span.distance_m, 2),
        });
        stf.push(TenWindow {
            time: 10,
            value: span.steps as f64,
        });
    }
    (sp, stf)
}

fn rebuild_segments(track: &Track) -> Vec<Segment> {
    let locs = &track.locations;
    let total = track.totalTime as f64;
    let mut out: Vec<Segment> = Vec::new();
    let mut lo = 0.0f64;
    while lo < total {
        let hi = (lo + 60.0).min(total);
        let d = super::wire::cumulative_at(locs, hi, |p| p.totalDis)
            - super::wire::cumulative_at(locs, lo, |p| p.totalDis);
        let s = super::wire::cumulative_at(locs, hi, |p| p.steps as f64)
            - super::wire::cumulative_at(locs, lo, |p| p.steps as f64);
        let span = (hi - lo).max(1.0);
        out.push(Segment {
            totalTime: round_to(span, 0) as i64,
            distance: round_to(d.max(0.0), 0) as i64,
            startTime: round_to(lo * 1000.0, 0) as i64,
            endTime: round_to(hi * 1000.0, 0) as i64,
            avgSpeed: round_to(d.max(0.0) / span, 3),
            avgStep: round_to(s.max(0.0) / span * 60.0, 0) as i64,
            state: 0,
        });
        lo = hi;
    }
    // 末段太短（<10 s）时并入上一段：真人收尾不会出现「5 秒走了 20 米」的独立段。
    if out.len() >= 2 {
        let last = out.len() - 1;
        if out[last].totalTime < 10 {
            let tail = out.remove(last);
            if let Some(prev) = out.last_mut() {
                let span = (prev.totalTime + tail.totalTime).max(1) as f64;
                let dist = prev.distance + tail.distance;
                let step = (prev.avgStep + tail.avgStep) as f64 / 2.0;
                prev.totalTime = span as i64;
                prev.distance = dist;
                prev.endTime = tail.endTime;
                prev.avgSpeed = round_to(dist as f64 / span, 3);
                prev.avgStep = round_to(step, 0) as i64;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{build, build_stadium, MAX_EXCEPTION_DRIFT_M, MAX_EXCEPTION_GPS_SPEED_KMH};
    use crate::track::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};
    use crate::track::stadium::TrackSpec;

    fn site_points() -> Vec<(f64, f64)> {
        vec![
            (39.9000, 116.4000),
            (39.9000, 116.4020),
            (39.9020, 116.4020),
            (39.9020, 116.4000),
        ]
    }

    #[test]
    fn exception_points_cannot_create_map_scale_jumps_or_speed_spikes() {
        for seed in 0..32 {
            let points = site_points();
            let track = build(2_000.0, 800, seed, points[0], 1_700_000_000_000, &points);
            for (index, point) in track.locations.iter().enumerate() {
                if point.ptype != -1 {
                    continue;
                }
                assert!(point.radius <= 10.0);
                assert!(point.speed <= MAX_EXCEPTION_GPS_SPEED_KMH + 1e-9);
                if let Some(previous) = index.checked_sub(1).and_then(|i| track.locations.get(i)) {
                    let dx = (point.gLng - previous.gLng) * MET_PER_DEG_LNG;
                    let dy = (point.gLat - previous.gLat) * MET_PER_DEG_LAT;
                    assert!(
                        (dx * dx + dy * dy).sqrt() <= 80.0,
                        "exception jump at seed {seed}: {}m",
                        (dx * dx + dy * dy).sqrt()
                    );
                }
            }
        }
        assert!(MAX_EXCEPTION_DRIFT_M < 10.0);
    }

    #[test]
    fn every_sample_gap_is_within_device_period() {
        let points = site_points();
        let track = build(3_000.0, 1_100, 3, points[0], 1_700_000_000_000, &points);
        // 首点与起点哨兵是同一点（协议要求），因此只有这一对的间隔为 0。
        let mut zero_gaps = 0;
        for pair in track.locations.windows(2) {
            let gap = pair[1].totalTime - pair[0].totalTime;
            if gap == 0 {
                zero_gaps += 1;
                assert!(zero_gaps <= 1, "只允许起点哨兵这一处零间隔");
                continue;
            }
            assert!(gap <= 30, "gap={gap}");
        }
    }

    #[test]
    fn stadium_build_uses_standard_lap_geometry() {
        let points: Vec<(f64, f64)> = (0..8)
            .map(|k| {
                let a = k as f64 / 8.0 * std::f64::consts::TAU;
                (39.9 + 0.0009 * a.sin(), 116.4 + 0.0011 * a.cos())
            })
            .collect();
        let track = build_stadium(
            2_000.0,
            720,
            5,
            (39.9, 116.4),
            1_700_000_000_000,
            &points,
            TrackSpec::M400,
        )
        .expect("stadium ring");
        // 跑道语义：距离取整圈，标准 400 m 跑道跑 5 圈 = 2000 m（±3% 几何容差）
        assert!((track.totalDistance / 2_000.0 - 1.0).abs() < 0.03);
        assert!((track.totalDistance / 400.0).round() >= 4.0);
        assert!(track.locations.len() > 100);
    }

    /// 卫星数是**位置的函数**：绕回同一处时读数应当回来，而不是随机重掷。
    ///
    /// 逐点独立随机或纯时间 OU 都过不了这条——它们把"同一段路第二次跑过"当成
    /// 全新的抽样。真机上是同一批楼挡着同一片天。
    #[test]
    fn satellite_count_is_a_function_of_place() {
        let points = site_points();
        // 跑 8 圈，同一条跑道会被反复经过。
        let track = build(3_200.0, 1_180, 11, points[0], 1_700_000_000_000, &points);
        let locs = &track.locations;

        // 1) 整个环上要有明显起伏（不是常数）。
        let cs: Vec<f64> = locs.iter().map(|p| p.count as f64).collect();
        let lo = cs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = cs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(hi - lo >= 6.0, "卫星数在整圈上几乎不变（{lo}..{hi}）");

        // 2) 空间上邻近的点读数应当接近：取 3 m 内的点对，平均差要远小于极差。
        let mut near_sum = 0.0;
        let mut near_n = 0usize;
        for i in 0..locs.len() {
            for j in (i + 1)..locs.len() {
                let dx = (locs[j].gLng - locs[i].gLng) * MET_PER_DEG_LNG;
                let dy = (locs[j].gLat - locs[i].gLat) * MET_PER_DEG_LAT;
                if (dx * dx + dy * dy).sqrt() <= 3.0 {
                    near_sum += (locs[j].count - locs[i].count).abs() as f64;
                    near_n += 1;
                }
            }
        }
        assert!(near_n >= 20, "样本不足以判断空间相关性（{near_n} 对）");
        let near_mean = near_sum / near_n as f64;
        assert!(
            near_mean < 3.5,
            "同一处的卫星数应相近，实测邻近点平均差 {near_mean:.1}（极差 {:.0}）",
            hi - lo
        );
    }

    #[test]
    fn realism_audit_passes_across_distances_and_seeds() {
        for (dist, dur) in [(2_000.0f64, 760i64), (3_300.0, 1_220), (5_000.0, 1_800)] {
            for seed in [0u64, 3, 42] {
                let points = site_points();
                let track = build(dist, dur, seed, points[0], 1_700_000_000_000, &points);
                let report = crate::track::realism::audit(&track);
                assert!(
                    report.passed(),
                    "dist={dist} seed={seed}: {}",
                    report.failures.join("; ")
                );
            }
        }
    }
}
