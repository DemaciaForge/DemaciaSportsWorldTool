//! 自然轨迹生成器。
//!
//! 四种几何入口（打卡点拟合环 / 标准跑道 / 真实道路 / 自定义路线）共用同一个组装器
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

use super::geom::{fmt_gain_time, make_point_ring, round_to, wgs84_to_bd09, Rng};
use super::model::{GenPoint, Segment, TenWindow, Track};
use super::postfix::apply_post_fixes;
use super::realism::{fit_speeds_capped, sample_grid};
use super::shape::{site_ring, ShapeTracker, ShapedPlan, SiteRing};
use super::stadium::TrackSpec;
use crate::api::model::TrackColorMode;
use route_planner::{fatigue_from_km, gait, ou_cadence_series};

/// 有效配速窗口（判定规则 2'21"-10'00"/km ≈ 1.667-7.092 m/s），硬边界留余量。
pub const SPEED_FLOOR: f64 = 1.90;
pub const SPEED_CEIL: f64 = 6.30;
pub const MAX_EXCEPTION_GPS_SPEED_KMH: f64 = 6.0;
/// Maximum coordinate drift for a protocol exception point.
///
/// These points may still carry their original type marker, but they must not
/// create a map-scale jump that the server interprets as an invalid route.
pub const MAX_EXCEPTION_DRIFT_M: f64 = 6.0;

/// 速度字段的测量噪声（相对标准差）：真机 `speed` 与「位移/时间差」只差这一点。
pub const SPEED_FIELD_MEAN_NOISE: f64 = 0.05;
pub const SPEED_FIELD_MIN_NOISE: f64 = 0.01;
/// 速度字段短时丢失（隧道 / 建筑遮挡）的概率与替代值。
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
    build_with_color_mode(
        dist,
        dur,
        seed,
        _center,
        start_ms,
        points_bd,
        TrackColorMode::FullGreen,
    )
}

/// Generate a route along a selected standard athletics track.
pub fn build_stadium(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
    spec: TrackSpec,
) -> Result<Track, String> {
    let mut rng = Rng::new(seed);
    let mut ring = site_ring(points_bd, spec)?;
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
            if let Ok((dense, arcs, c)) = super::stadium::make_stadium_ring_relaxed(
                &[center, (center.0 + 1e-6, center.1)],
                TrackSpec::Custom {
                    total_m: wanted.round().clamp(200.0, 1000.0) as u32,
                },
                true,
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
        color_mode: TrackColorMode::FullGreen,
        snap_points_bd: points_bd,
        snap_radius_m: 40.0,
        start_at_ring_origin: false,
        loop_m: None,
        rng,
    }))
}

/// 生成轨迹并选择详情页路线颜色模式。
pub fn build_with_color_mode(
    dist: f64,
    dur: i64,
    seed: u64,
    _center: (f64, f64),
    start_ms: i64,
    points_bd: &[(f64, f64)],
    color_mode: TrackColorMode,
) -> Track {
    let mut rng = Rng::new(seed);
    let mut ring =
        site_ring(points_bd, TrackSpec::Auto).unwrap_or_else(|_| fallback_ring(points_bd));
    if dist > 3000.0 {
        let wanted = ((dist / 4.5).clamp(800.0, 2000.0) / 100.0).round() * 100.0;
        if (ring.ring_m - wanted).abs() > wanted * 0.25 {
            let center = ring.center;
            if let Ok((dense, arcs, c)) = super::stadium::make_stadium_ring_relaxed(
                &[center, (center.0 + 1e-6, center.1)],
                TrackSpec::Custom {
                    total_m: wanted.round().clamp(200.0, 1000.0) as u32,
                },
                true,
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
        color_mode,
        snap_points_bd: points_bd,
        snap_radius_m: 40.0,
        start_at_ring_origin: false,
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
    pub color_mode: TrackColorMode,
    /// 需要吸附的打卡点（BD）。
    pub snap_points_bd: &'a [(f64, f64)],
    /// 吸附半径（米）。
    pub snap_radius_m: f64,
    /// true = 起点固定在场路线原点（自定义路线 / 跑道语义）。
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
        color_mode,
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

    // ② 形态：进场段 + 绕圈 + 离场段（自定义路线 / 跑道语义下全程都在路线上）
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
    // ③ 点位类型（与旧实现同分布；FullGreen 全部归一为普通点）
    let kinds: Vec<(i64, i64)> = match color_mode {
        TrackColorMode::FullGreen => vec![(0, 1); n],
        TrackColorMode::HalfGreenGray => {
            let tl_w = [
                ((-1i64, 4i64), 54u32),
                ((-1, 1), 27),
                ((-1, 12), 13),
                ((-1, 5), 3),
                ((-1, 6), 2),
            ];
            let mut kinds: Vec<(i64, i64)> = Vec::with_capacity(n);
            for _ in 0..n {
                let u = rng.random();
                if u < 0.39 {
                    kinds.push((3, 1));
                } else if u < 0.93 {
                    kinds.push((0, 1));
                } else if u < 0.96 {
                    kinds.push(rng.weighted(&tl_w));
                } else {
                    kinds.push((rng.choice(&[1, 1, 1, 1, 2, 2]), 1));
                }
            }
            for i in 1..kinds.len() {
                let prev_drift = kinds[i - 1].0 == -1 || kinds[i - 1].0 == 5 || kinds[i - 1].0 == 6;
                if kinds[i].0 != -1 && prev_drift && rng.random() < 0.08 {
                    kinds[i] = (if rng.random() < 0.75 { 7 } else { 8 }, 1);
                }
            }
            for i in 1..n {
                if kinds[i].0 == -1 && kinds[i - 1].0 == -1 {
                    kinds[i] = (rng.choice(&[3, 0]), 1);
                }
            }
            kinds
        }
    };

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
        if std::env::var("NSPROBE").is_ok() && i >= 5 && i <= 12 {
            let pr = tracker_plan_debug(&locs);
            eprintln!(
                "DBG i={i} step={:.2} dt={:.1} pt=({:.2},{:.2}) {pr}",
                step_m[i], step_dt[i], point.0, point.1
            );
        }
        let (lat, lng) = ring.to_bd(point.0, point.1);
        let (typ, lt) = kinds[i];
        let rad = round_to(
            if typ == 3 {
                rng.uniform(1.4, 5.1)
            } else {
                rng.uniform(1.4, 2.4)
            },
            2,
        );
        let state = if color_mode == TrackColorMode::FullGreen {
            1
        } else if typ == 0 {
            rng.weighted(&[(1, 145), (2, 45), (3, 164)])
        } else {
            rng.weighted(&[(1, 145), (2, 256), (3, 151)])
        };
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
            bdG: if color_mode == TrackColorMode::FullGreen {
                1
            } else {
                rng.choice(&[1, 1, 1, -1])
            },
            count: rng.randint(20, 88),
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
    for i in 2..n {
        let exact = step_m[i] / mean_stride + carry;
        let ds = exact.floor();
        carry = exact - ds;
        acc_steps += ds;
        locs[i].steps = acc_steps as i64;
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
        // 速度字段 = 本段位移 / 本段时间差（m/s → km/h）+ 测量噪声
        let kmh = seg / dt * 3.6;
        locs[i].speed = if i == 0 {
            round_to(kmh, 4)
        } else if rng.random() < SPEED_FIELD_DROP_P {
            round_to(
                rng.uniform(SPEED_FIELD_DROP_KMH.0, SPEED_FIELD_DROP_KMH.1),
                4,
            )
        } else {
            let sigma = (kmh * SPEED_FIELD_MEAN_NOISE).max(SPEED_FIELD_MIN_NOISE);
            round_to((kmh + rng.gauss(0.0, sigma)).max(0.0), 4)
        };
        // 海拔：每米计起伏 + 与配速同相
        let km_done = cum_i / 1000.0;
        let phase = (km_done * std::f64::consts::TAU / 0.9 + 0.7 * speeds[i]).sin();
        let target_alt = alt0 + ALT_UNDULATION_M_PER_KM * (km_done * 1.6 + 0.35 * phase);
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
    if color_mode == TrackColorMode::FullGreen {
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
    //    「本段位移 / 本段时间差」。
    let n_len = locs.len();
    for i in 2..n_len {
        let dt = (locs[i].totalTime - locs[i - 1].totalTime).max(1) as f64;
        let seg = dir(&locs[i - 1].coords(), &locs[i].coords());
        locs[i].avgSpeed = round_to(seg / dt, 4);
        let kmh = seg / dt * 3.6;
        let sigma = (kmh * SPEED_FIELD_MEAN_NOISE).max(SPEED_FIELD_MIN_NOISE);
        locs[i].speed = round_to((kmh + rng.gauss(0.0, sigma)).max(0.0), 4);
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

fn steps_at(speeds: &[f64], dts: &[f64], i: usize) -> f64 {
    if i < 2 {
        0.0
    } else {
        speeds.get(i).copied().unwrap_or(0.0) * dts.get(i).copied().unwrap_or(0.0)
    }
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

/// 删掉末尾「坐标没动」的多余采样点（真人不会在终点原地静默十几秒才开始采样）。
///
/// 删点时把被删点的步数并入保留的末点，避免「少了一个采样点、步数却凭空少了」。
fn drop_duplicate_tail(locs: &mut Vec<GenPoint>) {
    while locs.len() > 4 {
        let m = locs.len();
        let a = &locs[m - 2];
        let b = &locs[m - 1];
        let gap = ((a.gLat - b.gLat) * super::geom::MET_PER_DEG_LAT)
            .hypot((a.gLng - b.gLng) * super::geom::MET_PER_DEG_LNG);
        if gap < 0.5 {
            let dropped = locs.remove(m - 1);
            let k = locs.len();
            locs[k - 1].steps = locs[k - 1].steps.max(dropped.steps);
        } else {
            break;
        }
    }
}

/// 按真实采样网格重排时间轴。
///
/// 点数是设备此刻真实产生的采样点数，把它们铺到 `0..target` 上：主间隔 5 s（±1.5 s
/// 抖动），首点 0、末点正好 `target`。这样采样间隔不会被「末尾补点」压到 1–2 s，
/// 末段拿到该有的时长，10 秒窗也不会被读成掉速。
pub(crate) fn relayout_timeline(locs: &mut [GenPoint], target: i64, rng: &mut Rng) {
    let n = locs.len();
    if n < 3 || target <= 2 {
        return;
    }
    let interior = (n - 2).max(1) as f64;
    let slot = target as f64 / (interior + 1.0);
    let mut t = 0.0f64;
    for i in 1..n {
        if i == 1 {
            t = 0.0;
        } else if i == n - 1 {
            t = target as f64;
        } else {
            let jitter = if rng.random() < 0.15 {
                rng.uniform(-1.5, 1.5)
            } else {
                0.0
            };
            t = (t + slot + jitter).min(target as f64 - 1.0);
        }
        locs[i].totalTime = (t.round() as i64).max(locs[i - 1].totalTime);
    }
    for i in 2..n - 1 {
        if locs[i].totalTime <= locs[i - 1].totalTime {
            locs[i].totalTime = locs[i - 1].totalTime + 1;
        }
    }
    locs[n - 1].totalTime = target.max(locs[n - 2].totalTime + 1);
}

/// 停下之后补 1–2 个采样点（真人冲过终点会走两步再站着喘气）。
///
/// 这些点的坐标只做厘米级 GPS 抖动、速度为步行级，因此末段不再是「几秒跑完最后几米」，
/// 10 秒窗不会读成掉速；累计距离只小幅增加，总距离与轨迹几何仍然自洽。
pub(crate) fn append_cool_down(locs: &mut Vec<GenPoint>, target_time: i64, rng: &mut Rng) {
    if locs.len() < 4 {
        return;
    }
    let last = locs.len() - 1;
    let mut t = locs[last].totalTime;
    let mut anchor = (locs[last].gLat, locs[last].gLng);
    let mut steps = locs[last].steps;
    let mut extra = 0;
    while t < target_time - 1 && extra < 2 {
        extra += 1;
        let dt = 5.0f64.min((target_time - t) as f64);
        if dt < 1.0 {
            break;
        }
        t += dt as i64;
        // 站着不动，只有 GPS 抖动（±1 m）
        anchor = (
            anchor.0 + rng.gauss(0.0, 1.0) / super::geom::MET_PER_DEG_LAT,
            anchor.1 + rng.gauss(0.0, 1.0) / super::geom::MET_PER_DEG_LNG,
        );
        // 走走停停：约 0.6 m/s 的步行位移折算成步数
        let walk_m = rng.uniform(0.0, 1.2) * dt;
        steps += (walk_m / 0.72).round() as i64;
        let mut p = locs[last].clone();
        p.id = locs[last].id + extra as i64;
        p.totalTime = t;
        p.validTime = t;
        p.gainTimeMs = locs[last].gainTimeMs + extra as i64 * 1000;
        p.gainTime = fmt_gain_time(p.gainTimeMs);
        p.gLat = round_to(anchor.0, 7);
        p.gLng = round_to(anchor.1, 7);
        p.steps = steps;
        p.ptype = 6;
        p.locType = 1;
        p.speed = round_to(rng.uniform(0.0, 1.6), 4);
        p.avgSpeed = round_to(walk_m / dt, 4);
        p.bdA = locs[last].bdA;
        p.bdD = round_to((locs[last].bdD + rng.gauss(0.0, 25.0)).rem_euclid(360.0), 2);
        p.bdS = round_to(rng.uniform(0.1, 0.5), 3);
        locs.push(p);
    }
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

/// 坐标被改写后重算逐点累计距离（保留累计时间与步数）。
///
/// 起点哨兵（索引 1）与首点同址，累计距离按协议语义固定为 0。
fn recompute_geometry_cumulative(locs: &mut [GenPoint]) {
    use super::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};
    if locs.len() > 2 && (locs[0].gLat - locs[1].gLat).abs() >= 1e-9 {
        // 首点与哨兵统一（哨兵只是零值标记）
        locs[0].gLat = locs[1].gLat;
        locs[0].gLng = locs[1].gLng;
    }
    let mut acc = 0.0f64;
    for i in 1..locs.len() {
        let d = ((locs[i].gLat - locs[i - 1].gLat) * MET_PER_DEG_LAT)
            .hypot((locs[i].gLng - locs[i - 1].gLng) * MET_PER_DEG_LNG);
        acc += d;
        let val = if i <= 1 { 0.0 } else { acc };
        locs[i].totalDis = round_to(val, 4);
        locs[i].validDis = round_to(val, 4);
    }
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

fn cumulative_at(locs: &[GenPoint], t: f64, pick: impl Fn(&GenPoint) -> f64) -> f64 {
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

fn rebuild_windows(track: &Track) -> (Vec<TenWindow>, Vec<TenWindow>) {
    let locs = &track.locations;
    let total = track.totalTime as f64;
    // 窗口切分：整除的 10 s 窗，余下的残窗并入上一窗。
    //
    // 协议里 `speedPerTenSec[].time` 恒为 10，服务端按「距离 / 10 秒」读配速。
    // 若末窗只有 3 s 却被当成 10 s，就会读成「1 公里 20 分钟」的假掉速，因此残窗
    // 必须并进前一个窗口，而不是自成一项。
    let full = (total / 10.0).floor().max(0.0);
    let n_win = if full < 1.0 { 1 } else { full as usize };
    let span = total / n_win as f64;
    let mut sp: Vec<TenWindow> = Vec::new();
    let mut stf: Vec<TenWindow> = Vec::new();
    let mut lo = 0.0f64;
    for k in 0..n_win {
        let hi = if k + 1 == n_win { total } else { lo + span };
        let d = cumulative_at(locs, hi, |p| p.totalDis) - cumulative_at(locs, lo, |p| p.totalDis);
        let s = cumulative_at(locs, hi, |p| p.steps as f64)
            - cumulative_at(locs, lo, |p| p.steps as f64);
        sp.push(TenWindow {
            time: 10,
            value: round_to(d.max(0.0), 2),
        });
        stf.push(TenWindow {
            time: 10,
            value: round_to(s.max(0.0), 0),
        });
        lo = hi;
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
        let d = cumulative_at(locs, hi, |p| p.totalDis) - cumulative_at(locs, lo, |p| p.totalDis);
        let s = cumulative_at(locs, hi, |p| p.steps as f64)
            - cumulative_at(locs, lo, |p| p.steps as f64);
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

/// 把 WGS84 坐标转成 BD-09（真实道路入口用）。
pub(crate) fn align_wgs84(lat: f64, lng: f64) -> (f64, f64) {
    wgs84_to_bd09(lat, lng)
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

fn tracker_plan_debug(_locs: &[GenPoint]) -> String {
    String::new()
}
