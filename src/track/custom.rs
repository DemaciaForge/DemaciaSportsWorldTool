//! 自定义路线轨迹生成器（模式 C）。
//!
//! 几何源为高德步行路线（gcj02 → BD-09 对齐打卡点后走点）：
//! - 单程：A→B 一条腿；
//! - 绕圈：A→B→A 去程+返程合成自然闭环（返程取高德 B→A 路线，失败时回退去程反向）；
//! - 圈数：0 = 按目标距离自动（走点引擎自动绕环，含不满一圈的零头）；>0 = 精确圈数。
//!
//! 自 `feat/realistic-record-authenticity` 起，本模式不再自带一套速度/步频计算，而是
//! 与经典/跑道/路网三种模式共用 `generator::assemble`：速度剖面由 `speed_profile`
//! 给出，步频由「速度 × 疲劳」的生物力学耦合反解，速度字段由「位移/时间差」给出，
//! 采样点落在「路线 + 缓慢漂移的横向偏移」上。旧实现在这里用**全程平均速度**算步频，
//! 与逐点速度完全解耦，是可被统计出来的形态。
//!
//! 起点固定为路线起点（不随机偏移弧长），保证「从哪里跑」即用户填的起点。

#![allow(non_snake_case)]

use super::generator::{assemble, speed_profile, Assembly};
use super::geom::{Rng, MET_PER_DEG_LAT, MET_PER_DEG_LNG};
use super::model::Track;
use super::shape::SiteRing;
use crate::api::model::TrackColorMode;

/// 自定义路线参数（GUI 跑步页 / CLI run 共用）。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CustomRouteSpec {
    /// 起点：地址文本或坐标（支持 经度,纬度 / 纬度,经度 自动识别）。
    pub origin: String,
    /// 终点：同上。
    pub destination: String,
    /// true = 绕圈（A→B→A 闭环）；false = 单程 A→B。
    pub loop_mode: bool,
    /// 圈数：0 = 按目标距离自动；>0 = 精确圈数（距离=圈数×闭环长度）。
    pub laps: i64,
}

impl Default for CustomRouteSpec {
    fn default() -> Self {
        Self {
            origin: String::new(),
            destination: String::new(),
            loop_mode: true,
            laps: 0,
        }
    }
}

impl CustomRouteSpec {
    pub fn is_valid(&self) -> bool {
        !self.origin.trim().is_empty() && !self.destination.trim().is_empty()
    }
}

/// 解析好的闭环路线（BD-09，与打卡点同系）。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CustomRing {
    /// 环点（BD-09）；绕圈模式为 A→B→A 闭环，单程模式为 A→B 开链。
    pub points_bd: Vec<(f64, f64)>,
    /// 去程 A→B 长度（米）。
    pub leg_length_m: f64,
    /// 闭环总长（米）：绕圈 = 去程+返程；单程 = 去程。
    pub total_length_m: f64,
    /// 是否绕圈闭环。
    pub loop_mode: bool,
}

/// 按绕圈/单程/圈数计算本次实际距离与时长，返回 (距离, 时长, 说明日志)。
///
/// 平均速度沿用调用方已校正到有效配速窗口内的 dist/dur 比值，
/// 只对距离做裁剪/圈数换算，时长等比换算。
pub fn effective_dist_dur(
    spec: &CustomRouteSpec,
    ring: &CustomRing,
    dist: f64,
    dur: i64,
) -> Result<(f64, i64, String), String> {
    if ring.total_length_m <= 0.0 {
        return Err("路线长度无效（0 米）".into());
    }
    let avg_v = dist / (dur.max(1) as f64);
    if avg_v <= 0.0 {
        return Err("平均速度无效".into());
    }
    let (d, note) = if spec.loop_mode {
        if spec.laps > 0 {
            (
                spec.laps as f64 * ring.total_length_m,
                format!(
                    "绕圈 {:.0} m × {} 圈 = {:.0} m",
                    ring.total_length_m,
                    spec.laps,
                    spec.laps as f64 * ring.total_length_m
                ),
            )
        } else {
            (
                dist,
                format!(
                    "绕圈自动（{:.0} m/圈，目标 {:.0} m ≈ {:.1} 圈）",
                    ring.total_length_m,
                    dist,
                    dist / ring.total_length_m
                ),
            )
        }
    } else if dist > ring.total_length_m {
        (
            ring.total_length_m,
            format!(
                "单程路线 {:.0} m < 目标 {:.0} m，按路线全长提交",
                ring.total_length_m, dist
            ),
        )
    } else {
        (dist, format!("单程 {:.0} m", dist))
    };
    let dur_new = (d / avg_v).round() as i64;
    Ok((d, dur_new, note))
}

///
/// `snap_points_bd` 为打卡点（BD-09），与路线距离 <40m 的打卡点会被吸附。
/// 起点固定为路线起点（用户填的起点），因此不启用进场段；离场段保留，用来表达
/// 「跑完走出路线」——这既是真人的行为，也让末点不落在解析曲线的精确原点上。
pub fn build_custom(
    dist: f64,
    dur: i64,
    seed: u64,
    start_ms: i64,
    ring: &CustomRing,
    snap_points_bd: &[(f64, f64)],
    pace: crate::track::pace::PaceWindow,
) -> Track {
    let mut rng = Rng::new(seed);
    let site = custom_site_ring(ring);
    let (times, dts, speeds) = speed_profile(dist, dur as f64, &mut rng, pace);
    // 路线点本身算出的周长才是「跑几圈」的依据；客户端给的 total_length_m 只兜底。
    let loop_m = site.ring_m;
    assemble(Assembly {
        dist,
        dur_s: dur as f64,
        start_ms,
        seed,
        ring: site,
        times,
        dts,
        speeds,
        color_mode: TrackColorMode::FullGreen,
        snap_points_bd,
        snap_radius_m: 40.0,
        start_at_ring_origin: true,
        rng,
        loop_m: Some(loop_m),
        pace,
    })
}

/// 高德闭环 → `SiteRing`（平面折线 + 弧长表 + BD 质心）。
///
/// 闭合处理：末点补回首点，`ring_point_at` 因此可以无缝绕圈；弧长表按实际折线长度
/// 累计，周长就是高德给的路线长度（误差来自 BD-09 换算，校园尺度上 <1 m）。
pub fn custom_site_ring(ring: &CustomRing) -> SiteRing {
    let n_pts = ring.points_bd.len().max(1);
    let (c_lat, c_lng) = (
        ring.points_bd.iter().map(|q| q.0).sum::<f64>() / n_pts as f64,
        ring.points_bd.iter().map(|q| q.1).sum::<f64>() / n_pts as f64,
    );
    let mut dense: Vec<(f64, f64)> = ring
        .points_bd
        .iter()
        .map(|&(la, lo)| {
            (
                (lo - c_lng) * MET_PER_DEG_LNG,
                (la - c_lat) * MET_PER_DEG_LAT,
            )
        })
        .collect();
    let closed = dense.len() < 2
        || ((dense[0].0 - dense[dense.len() - 1].0).abs() < 1e-9
            && (dense[0].1 - dense[dense.len() - 1].1).abs() < 1e-9);
    if dense.len() >= 2 && !closed {
        dense.push(dense[0]);
    }
    let mut arcs = vec![0.0f64];
    for i in 1..dense.len() {
        let a = dense[i - 1];
        let b = dense[i];
        arcs.push(arcs[i - 1] + ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt());
    }
    let ring_m = arcs.last().copied().unwrap_or(0.0);
    SiteRing {
        dense,
        arcs,
        center: (c_lat, c_lng),
        ring_m: if ring_m > 1.0 {
            ring_m
        } else {
            ring.total_length_m.max(1.0)
        },
        genuine: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试统一用一个宽松的配速窗口，避免各用例被学校规则耦合。
    const TEST_PACE: crate::track::pace::PaceWindow = crate::track::pace::PaceWindow::FALLBACK;

    fn sample_ring() -> CustomRing {
        // 校园内一个 1km 左右的闭环（BD-09，与打卡点同系）
        let c = (38.90f64, 121.54f64);
        let mut pts = Vec::new();
        for k in 0..40 {
            let a = k as f64 / 40.0 * std::f64::consts::TAU;
            pts.push((c.0 + 0.0045 * a.sin(), c.1 + 0.0045 * a.cos()));
        }
        CustomRing {
            leg_length_m: 500.0,
            total_length_m: 1000.0,
            points_bd: pts,
            loop_mode: true,
        }
    }

    fn sample_checkpoints() -> Vec<(f64, f64)> {
        vec![(38.900678, 121.540241), (38.902564, 121.541233)]
    }

    /// 距离与目标一致（±3%：横向漂移与吸附带来的真实几何差异）、结构合法。
    #[test]
    fn test_build_custom_structure() {
        let ring = sample_ring();
        let track = build_custom(
            2200.0,
            900,
            7,
            1_788_958_186_123,
            &ring,
            &sample_checkpoints(),
            TEST_PACE,
        );
        assert!(
            (track.totalDistance / 2200.0 - 1.0).abs() < 0.03,
            "dist={}",
            track.totalDistance
        );
        assert_eq!(track.totalTime, 900);
        assert!(!track.locations.is_empty());
        assert!([0, 7].contains(&track.locations[0].ptype));
        assert_eq!(track.locations[1].ptype, 5);
        assert_eq!(track.locations.last().unwrap().ptype, 6);
        assert!(track.totalSteps > 500);
        // 累计距离单调不减
        let mut prev = 0.0;
        for p in &track.locations {
            assert!(p.totalDis >= prev - 1e-6);
            prev = p.totalDis;
        }
    }

    /// 起点必须是用户填的起点。
    ///
    /// 起点吸附与「进场段」几何互相约束：进场段从场外走来，因此首点不可能是路线
    /// 原点的精确值；这里固定的是「起点落在路线起点附近」，进场段的长度受
    /// `TravelPlan::MAX_ACCESS_FRAC`（路线长度的 25%/2）约束。
    #[test]
    fn test_start_is_ring_origin() {
        let ring = sample_ring();
        let track = build_custom(1200.0, 480, 3, 1_788_958_186_123, &ring, &[], TEST_PACE);
        let first = &track.locations[0];
        let (want_lat, want_lng) = ring.points_bd[0];
        let d = ((first.gLat - want_lat) * MET_PER_DEG_LAT).powi(2)
            + ((first.gLng - want_lng) * MET_PER_DEG_LNG).powi(2);
        assert!(d.sqrt() < 300.0, "起点偏离路线起点 {:.1} m", d.sqrt());
    }

    /// 圈数换算：精确圈数 / 单程裁剪 / 自动圈数。
    #[test]
    fn test_effective_dist_dur() {
        let ring = sample_ring();
        // 绕圈自动
        let (d, _, _) = effective_dist_dur(
            &CustomRouteSpec {
                loop_mode: true,
                laps: 0,
                ..Default::default()
            },
            &ring,
            2500.0,
            1000,
        )
        .unwrap();
        assert_eq!(d, 2500.0);
        // 绕圈 2 圈
        let (d, dur, _) = effective_dist_dur(
            &CustomRouteSpec {
                loop_mode: true,
                laps: 2,
                ..Default::default()
            },
            &ring,
            2500.0,
            1000,
        )
        .unwrap();
        assert!((d - 2000.0).abs() < 0.01);
        assert!((dur - 800).abs() <= 1);
        // 单程超长裁剪
        let (d, _, _) = effective_dist_dur(
            &CustomRouteSpec {
                loop_mode: false,
                laps: 0,
                ..Default::default()
            },
            &ring,
            2500.0,
            1000,
        )
        .unwrap();
        assert!((d - 1000.0).abs() < 0.01);
    }

    /// 10 秒窗配速全部落在有效窗口内（与经典模式同一判定规则；首窗同经典模式放宽）。
    #[test]
    fn test_custom_speeds_within_window() {
        let ring = sample_ring();
        for seed in 0..8u64 {
            let t = build_custom(2100.0, 900, seed, 1_788_958_186_123, &ring, &[], TEST_PACE);
            for (i, w) in t.speedPerTenSec.iter().enumerate() {
                let pace = 1000.0 / (w.value / 10.0);
                let ceil = if i == 0 { 1200.0 } else { 600.0 };
                assert!(
                    (141.0..=ceil).contains(&pace),
                    "seed={seed} 窗{i} 配速越界 {pace}"
                );
            }
        }
    }
}
