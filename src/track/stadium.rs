//! Standard athletics-track geometry fitted to the live server checkpoints.

use serde::{Deserialize, Serialize};

use super::geom::{PointRing, MET_PER_DEG_LAT, MET_PER_DEG_LNG};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackSpec {
    #[serde(rename = "auto", alias = "Auto")]
    Auto,
    #[serde(rename = "200m", alias = "200", alias = "m200", alias = "M200")]
    M200,
    #[serde(rename = "300m", alias = "300", alias = "m300", alias = "M300")]
    M300,
    #[serde(rename = "400m", alias = "400", alias = "m400", alias = "M400")]
    M400,
    #[serde(rename = "custom")]
    Custom { total_m: u32 },
}

impl Default for TrackSpec {
    fn default() -> Self {
        Self::Auto
    }
}

impl TrackSpec {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动（服务器点位）",
            Self::M200 => "200 米标准跑道",
            Self::M300 => "300 米标准跑道",
            Self::M400 => "400 米标准跑道",
            Self::Custom { .. } => "自定义跑道",
        }
    }

    fn dimensions(self) -> Option<TrackDimensions> {
        let total_m = match self {
            Self::Auto => return None,
            Self::M200 => 200,
            Self::M300 => 300,
            Self::M400 => 400,
            Self::Custom { total_m } if (100..=1000).contains(&total_m) => total_m,
            Self::Custom { .. } => return None,
        };
        let base = TrackDimensions {
            straight_m: 84.39,
            radius_m: 36.80,
        };
        let scale = total_m as f64 / base.length_m();
        Some(TrackDimensions {
            straight_m: base.straight_m * scale,
            radius_m: base.radius_m * scale,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct TrackDimensions {
    straight_m: f64,
    radius_m: f64,
}

impl TrackDimensions {
    fn length_m(self) -> f64 {
        2.0 * self.straight_m + 2.0 * std::f64::consts::PI * self.radius_m
    }
}

/// Fit a standard oval around the live checkpoints and return it in the same
/// local metre plane used by the regular track generator.
pub fn make_stadium_ring(points: &[(f64, f64)], spec: TrackSpec) -> Result<PointRing, String> {
    if points.len() < 3 {
        return Err("服务器打卡点不足，无法定位标准跑道".into());
    }
    make_stadium_ring_relaxed(points, spec, false)
}

/// 以「让每个打卡点都落在跑道上」为目标拟合标准跑道。
///
/// 与 [`make_stadium_ring`] 的区别只有一处：不拒绝「打卡点偏差超出预设置信度」的
/// 点位。用于两种场景：
///
/// - 服务端把跑道边线、不同圈数的入口都当成打卡点，预设容差（半径 30% + 3 m）
///   会把这类真实点位判成"不匹配"；
/// - 兜底：没有任何可用几何时，以打卡点质心为中心直接摆一条标准跑道。
///
/// 校验语义仍然保留：返回的环在几何上一定是标准跑道尺寸。
pub fn make_stadium_ring_relaxed(
    points: &[(f64, f64)],
    spec: TrackSpec,
    skip_pose_check: bool,
) -> Result<PointRing, String> {
    make_stadium_ring_with_fences(points, spec, skip_pose_check, &[])
}

/// 同 [`make_stadium_ring_relaxed`]，但用电子围栏把直道方向摆正。
///
/// 真实操场的直道顺着场地长边铺；围栏就是场地的边界，因此围栏最长边的方向就是
/// 直道应有的方向。没有围栏时行为与 `make_stadium_ring_relaxed` 完全一致。
pub fn make_stadium_ring_with_fences(
    points: &[(f64, f64)],
    spec: TrackSpec,
    skip_pose_check: bool,
    fences: &[Vec<(f64, f64)>],
) -> Result<PointRing, String> {
    let dimensions = spec
        .dimensions()
        .ok_or_else(|| "请选择有效的操场规格".to_string())?;
    if points.len() < 2 {
        return Err("服务器打卡点不足，无法定位标准跑道".into());
    }
    if points
        .iter()
        .any(|(lat, lng)| !lat.is_finite() || !lng.is_finite())
    {
        return Err("服务器打卡点包含非有限坐标".into());
    }

    let center_lat = points.iter().map(|point| point.0).sum::<f64>() / points.len() as f64;
    let center_lng = points.iter().map(|point| point.1).sum::<f64>() / points.len() as f64;
    let local: Vec<_> = points
        .iter()
        .map(|(lat, lng)| {
            (
                (lng - center_lng) * MET_PER_DEG_LNG,
                (lat - center_lat) * MET_PER_DEG_LAT,
            )
        })
        .collect();
    let (angle, offset) = fit_pose(&local, dimensions, fence_long_axis(fences));
    let mut dense: Vec<_> = rounded_rectangle(dimensions)
        .into_iter()
        .map(|point| rotate((point.0 + offset.0, point.1 + offset.1), angle))
        .collect();

    // Refuse a preset that clearly does not describe the checkpoint geometry.
    let tolerance = dimensions.radius_m * 0.30 + 3.0;
    let furthest = local
        .iter()
        .map(|&point| {
            let aligned = rotate(point, -angle);
            distance_to_centerline((aligned.0 - offset.0, aligned.1 - offset.1), dimensions)
        })
        .fold(0.0, f64::max);
    if furthest > tolerance && !skip_pose_check {
        return Err(format!(
            "服务器打卡点与{}跑道不匹配：最大偏差 {:.1}m（允许 {:.1}m）",
            spec.label(),
            furthest,
            tolerance
        ));
    }

    // Put close server checkpoints directly on the route so their markers do
    // not depend on a later sample accidentally landing at the same location.
    let mut used = std::collections::HashSet::new();
    for &(lat, lng) in points {
        let target = (
            (lng - center_lng) * MET_PER_DEG_LNG,
            (lat - center_lat) * MET_PER_DEG_LAT,
        );
        let nearest = dense
            .iter()
            .enumerate()
            .map(|(index, &point)| (index, distance(point, target)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((index, distance_to_ring)) = nearest {
            if distance_to_ring <= 3.0 {
                let index = nearest_unused(index, dense.len(), &used);
                used.insert(index);
                dense[index] = target;
            }
        }
    }

    let mut arcs = vec![0.0];
    for index in 1..=dense.len() {
        arcs.push(arcs[index - 1] + distance(dense[index - 1], dense[index % dense.len()]));
    }
    if (arcs[dense.len()] - dimensions.length_m()).abs() > 4.0 {
        return Err("标准跑道采样长度偏离设计值过大".into());
    }
    Ok((dense, arcs, (center_lat, center_lng)))
}

fn nearest_unused(
    preferred: usize,
    count: usize,
    used: &std::collections::HashSet<usize>,
) -> usize {
    (0..count)
        .map(|offset| (preferred + offset) % count)
        .find(|index| !used.contains(index))
        .unwrap_or(preferred)
}

/// 围栏长边方向（弧度，模 π），用于把跑道直道摆正。
///
/// 真实操场的直道几乎总是与场地长边平行（跑道顺着场地铺）。只按"哪条中心线穿过
/// 打卡点最贴合"去搜角度，会得到一条斜放在场地里的跑道——地图上一眼就不像。
///
/// 取所有围栏边里最长的那条的方向；围栏不足 3 个点时返回 `None`（不做约束）。
pub fn fence_long_axis(fences: &[Vec<(f64, f64)>]) -> Option<f64> {
    let mut best_len = 0.0f64;
    let mut best_angle = None;
    for ring in fences {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let a = ring[i];
            let b = ring[(i + 1) % n];
            let dx = (b.1 - a.1) * MET_PER_DEG_LNG;
            let dy = (b.0 - a.0) * MET_PER_DEG_LAT;
            let len = (dx * dx + dy * dy).sqrt();
            if len > best_len {
                best_len = len;
                // 与 `rotate` 的约定一致：角度按 (x=lng, y=lat) 平面量取，模 π。
                best_angle = Some(dy.atan2(dx).rem_euclid(std::f64::consts::PI));
            }
        }
    }
    best_angle
}

/// 两个方向（模 π）之间的最小夹角，结果落在 `[0, π/2]`。
fn axis_delta(a: f64, b: f64) -> f64 {
    let mut d = (a - b).rem_euclid(std::f64::consts::PI);
    if d > std::f64::consts::FRAC_PI_2 {
        d = std::f64::consts::PI - d;
    }
    d
}

/// 拟合跑道位姿：返回 (旋转角, 中心偏移)。
///
/// 两段式：
///
/// 1. 先只按几何拟合（"哪条中心线穿过打卡点最贴合"）；
/// 2. 只有当几何本身**说不清方向**（每个打卡点的残差 RMS 超过
///    [`POSE_AMBIGUOUS_RMS_M`]）时，才用围栏长边方向做先验重新拟合。
///
/// 做成条件触发而不是简单加权：加权在任何参数下都会在两种意图之间打架——权重小了
/// 摆不正，权重大了会把数据明确说明是斜的跑道掰直。而"几何是否能定方向"本身是个
/// 可判定的量，用残差一判就分开。
fn fit_pose(
    points: &[(f64, f64)],
    dimensions: TrackDimensions,
    fence_axis: Option<f64>,
) -> (f64, (f64, f64)) {
    let n = (points.len() as f64).max(1.0);
    let (geometry_angle, geometry_offset, geometry_rms2) = search_pose(points, dimensions, None);
    let Some(axis) = fence_axis else {
        return (geometry_angle, geometry_offset);
    };
    // 残差已经很小：打卡点把跑道钉住了，不要再动它。
    if geometry_rms2 <= POSE_AMBIGUOUS_RMS_M * POSE_AMBIGUOUS_RMS_M {
        return (geometry_angle, geometry_offset);
    }
    let (angle, offset, rms2) = search_pose(points, dimensions, Some(axis));
    // 加入先验后若几何反而变得很差，说明围栏与场地几何相互矛盾（围栏不是场地
    // 边界的形状），此时宁可听几何的。
    if rms2 > geometry_rms2 + POSE_AMBIGUOUS_RMS_M * POSE_AMBIGUOUS_RMS_M {
        return (geometry_angle, geometry_offset);
    }
    let _ = n;
    (angle, offset)
}

/// 每个打卡点到跑道中心线的残差 RMS 超过这个值（米），就认为几何定不了方向。
const POSE_AMBIGUOUS_RMS_M: f64 = 5.0;

/// 在 1° 网格上搜最优位姿；`fence_axis` 为 `Some` 时加围栏对齐惩罚。
///
/// 返回 (角度, 偏移, 每点残差平方均值)。
fn search_pose(
    points: &[(f64, f64)],
    dimensions: TrackDimensions,
    fence_axis: Option<f64>,
) -> (f64, (f64, f64), f64) {
    let mut best_angle = 0.0;
    let mut best_error = f64::INFINITY;
    let mut best_offset = bbox_center(points);
    let mut best_rms2 = f64::INFINITY;
    // 对齐先验的量纲是「米」：偏离 1 rad 相当于每个打卡点被错放 `ALIGN_PENALTY_M` 米。
    // 只在几何已判定为"说不清方向"时才会走到这里，所以这个值可以取大一些。
    const ALIGN_PENALTY_M: f64 = 250.0;
    let n = (points.len() as f64).max(1.0);
    for index in 0..=180 {
        let angle = -std::f64::consts::FRAC_PI_2 + index as f64 * std::f64::consts::PI / 180.0;
        let rotated: Vec<_> = points.iter().map(|&point| rotate(point, -angle)).collect();
        let offset = bbox_center(&rotated);
        let geometry = rotated
            .iter()
            .map(|&(x, y)| {
                let d = distance_to_centerline((x - offset.0, y - offset.1), dimensions);
                d * d
            })
            .sum::<f64>();
        let mut error = geometry;
        if let Some(axis) = fence_axis {
            // 直道方向 = 旋转后的 x 轴；把它拉到围栏长边上。
            let align = axis_delta(angle, axis);
            let penalty = ALIGN_PENALTY_M * align;
            error += n * penalty * penalty;
        }
        if error < best_error {
            best_error = error;
            best_angle = angle;
            best_offset = offset;
            best_rms2 = geometry / n;
        }
    }
    (best_angle, best_offset, best_rms2)
}

fn rounded_rectangle(dimensions: TrackDimensions) -> Vec<(f64, f64)> {
    let straight = dimensions.straight_m;
    let radius = dimensions.radius_m;
    let samples = |length: f64| (length / 0.75).ceil().max(8.0) as usize;
    let straight_count = samples(straight);
    let arc_count = samples(std::f64::consts::PI * radius);
    let mut points = Vec::with_capacity(straight_count * 2 + arc_count * 2);
    for index in 0..straight_count {
        let t = index as f64 / straight_count as f64;
        points.push((-straight / 2.0 + straight * t, radius));
    }
    for index in 0..arc_count {
        let angle =
            std::f64::consts::FRAC_PI_2 - std::f64::consts::PI * index as f64 / arc_count as f64;
        points.push((straight / 2.0 + radius * angle.cos(), radius * angle.sin()));
    }
    for index in 0..straight_count {
        let t = index as f64 / straight_count as f64;
        points.push((straight / 2.0 - straight * t, -radius));
    }
    for index in 0..arc_count {
        let angle =
            -std::f64::consts::FRAC_PI_2 - std::f64::consts::PI * index as f64 / arc_count as f64;
        points.push((-straight / 2.0 + radius * angle.cos(), radius * angle.sin()));
    }
    points
}

fn bbox_center(points: &[(f64, f64)]) -> (f64, f64) {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for &(x, y) in points {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0)
}

fn rotate(point: (f64, f64), angle: f64) -> (f64, f64) {
    let (sin, cos) = angle.sin_cos();
    (point.0 * cos - point.1 * sin, point.0 * sin + point.1 * cos)
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn distance_to_centerline(point: (f64, f64), dimensions: TrackDimensions) -> f64 {
    let half_straight = dimensions.straight_m / 2.0;
    let radius = dimensions.radius_m;
    if point.0.abs() <= half_straight {
        (point.1.abs() - radius).abs()
    } else {
        let center_x = if point.0.is_sign_negative() {
            -half_straight
        } else {
            half_straight
        };
        ((point.0 - center_x).hypot(point.1) - radius).abs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoints(spec: TrackSpec, lat: f64, lng: f64, angle: f64) -> Vec<(f64, f64)> {
        let dimensions = spec.dimensions().unwrap();
        rounded_rectangle(dimensions)
            .iter()
            .step_by((rounded_rectangle(dimensions).len() / 8).max(1))
            .map(|&(x, y)| {
                let point = rotate((x, y), angle);
                (
                    lat + point.1 / MET_PER_DEG_LAT,
                    lng + point.0 / MET_PER_DEG_LNG,
                )
            })
            .collect()
    }

    #[test]
    fn presets_have_expected_nominal_lengths() {
        for (spec, expected) in [
            (TrackSpec::M200, 200.0),
            (TrackSpec::M300, 300.0),
            (TrackSpec::M400, 400.0),
        ] {
            assert!((spec.dimensions().unwrap().length_m() - expected).abs() < 0.01);
        }
        assert!(TrackSpec::Custom { total_m: 99 }.dimensions().is_none());
    }

    #[test]
    fn fitted_ring_keeps_nominal_length_and_checkpoint_orientation() {
        let spec = TrackSpec::M400;
        let points = checkpoints(spec, 38.9, 121.54, 0.35);
        let (ring, arcs, _) = make_stadium_ring(&points, spec).unwrap();
        assert!((arcs.last().unwrap() - 400.0).abs() < 4.0);
        assert!(ring.iter().any(|point| point.0.abs() > 70.0));
    }

    #[test]
    fn generated_stadium_track_keeps_totals_consistent() {
        let spec = TrackSpec::M400;
        let points = checkpoints(spec, 38.9, 121.54, 0.35);
        let track = crate::track::generator::build_stadium(
            1200.0,
            600,
            7,
            (38.9, 121.54),
            1_700_000_000_000,
            &points,
            spec,
        )
        .unwrap();
        assert!(track.validate_consistency().is_ok());
    }

    #[test]
    fn rejects_points_that_do_not_fit_the_selected_preset() {
        let points = [(38.9, 121.54), (38.92, 121.54), (38.9, 121.56)];
        assert!(make_stadium_ring(&points, TrackSpec::M400).is_err());
    }

    /// 围栏长边方向：取最长边，模 π。
    #[test]
    fn fence_long_axis_follows_the_longest_edge() {
        // 南北向的长方形场地（长边沿经线方向）。
        let lat = 38.9;
        let lng = 121.54;
        let dlat = 200.0 / MET_PER_DEG_LAT;
        let dlng = 100.0 / MET_PER_DEG_LNG;
        let north_south = vec![vec![
            (lat + dlat, lng - dlng),
            (lat + dlat, lng + dlng),
            (lat - dlat, lng + dlng),
            (lat - dlat, lng - dlng),
        ]];
        let axis = fence_long_axis(&north_south).expect("应有长边方向");
        // 长边沿 ±lat（y 轴）⇒ 方向接近 ±π/2。
        assert!(
            axis_delta(axis, std::f64::consts::FRAC_PI_2) < 0.02,
            "南北向场地的长边方向应接近 π/2，实际 {axis}"
        );
        // 东西向场地：长边沿经线，方向接近 0。
        let east_west = vec![vec![
            (lat + dlng / 2.0, lng - 2.0 * dlng),
            (lat + dlng / 2.0, lng + 2.0 * dlng),
            (lat - dlng / 2.0, lng + 2.0 * dlng),
            (lat - dlng / 2.0, lng - 2.0 * dlng),
        ]];
        let axis = fence_long_axis(&east_west).expect("应有长边方向");
        assert!(
            axis_delta(axis, 0.0) < 0.02,
            "东西向场地的长边方向应接近 0，实际 {axis}"
        );
        // 空围栏不做约束。
        assert!(fence_long_axis(&[]).is_none());
    }

    /// 围栏把直道摆正：场地长边南北向时，拟合出的直道方向也应是南北向。
    ///
    /// 直接测 `fit_pose`（本轮改动的单元），不经过整环构造与打卡点吸附——
    /// 那两步会把角度误差和吸附误差混在一起。
    #[test]
    fn fence_aligns_straight_direction() {
        let dimensions = TrackSpec::M400.dimensions().unwrap();
        // 无围栏时，正圆对称的输入会让角度落在搜索起点附近，不保证与场地平行。
        let symmetric: Vec<(f64, f64)> = (0..8)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 8.0;
                (60.0 * a.cos(), 60.0 * a.sin())
            })
            .collect();
        let (free_angle, _) = fit_pose(&symmetric, dimensions, None);

        // 南北向场地：长边沿 y 轴 ⇒ 目标方向 π/2。
        let lat = 38.9;
        let lng = 121.54;
        let dlat = 200.0 / MET_PER_DEG_LAT;
        let dlng = 100.0 / MET_PER_DEG_LNG;
        let fences = vec![vec![
            (lat + dlat, lng - dlng),
            (lat + dlat, lng + dlng),
            (lat - dlat, lng + dlng),
            (lat - dlat, lng - dlng),
        ]];
        let axis = fence_long_axis(&fences).expect("应有长边方向");
        let (fenced_angle, _) = fit_pose(&symmetric, dimensions, Some(axis));

        let delta = axis_delta(fenced_angle, axis);
        assert!(
            delta < 0.12,
            "直道方向应与围栏长边平行，实际偏离 {:.1}°（无围栏时为 {:.1}°）",
            delta.to_degrees(),
            axis_delta(free_angle, axis).to_degrees()
        );
    }

    /// 围栏只是先验，不是硬约束：打卡点明确说明跑道是斜的时，几何项必须能赢。
    #[test]
    fn fence_prior_yields_to_clear_geometry() {
        let dimensions = TrackSpec::M400.dimensions().unwrap();
        // 沿 45° 摆放的打卡点（贴着中心线）。
        let slant = std::f64::consts::FRAC_PI_4;
        let dense = rounded_rectangle(dimensions);
        let step = (dense.len() / 16).max(1);
        let on_ring: Vec<(f64, f64)> = dense
            .iter()
            .step_by(step)
            .map(|&(x, y)| rotate((x, y), slant))
            .collect();
        // 围栏却说是东西向（0°）——与真实跑道差 45°。
        let strongest_prior = Some(0.0);
        let (angle, _) = fit_pose(&on_ring, dimensions, strongest_prior);
        assert!(
            axis_delta(angle, slant) < 0.25,
            "几何明确时不应被围栏先验掰弯，实际偏离真值 {:.1}°",
            axis_delta(angle, slant).to_degrees()
        );
    }
}
