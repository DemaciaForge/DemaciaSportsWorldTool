//! 轨迹形态层：把采样点放到「真实的一次跑步」上，而不是一条解析闭合曲线上。
//!
//! 服务端看到的是一条带时间戳的坐标序列。旧实现在闭合曲线上以精确弧长取值，
//! 得到的是**解析圆**：圆拟合度稳定超过 0.999、径向离散接近 0、每次跑满一圈都回到
//! 同一点。真人跑步的偏离来自三件事，本模块逐条建模：
//!
//! 1. **横向漂移**（`LaneTracker`）：步道宽度、让路、超车，稳态 1–2 m，随里程缓慢回归；
//! 2. **场地形态**（`TravelPlan`）：从场外走到起点 → 沿同一条路线绕整数圈 → 收尾走开；
//! 3. **进出不同侧**（`same_side = false` 时）：进入与离开走场地的两侧，路径不重合。
//!
//! 本模块不决定速度：调用方先给出逐点前进距离 `step_m[i]`，这里只负责把它换算成坐
//! 标。因此配速剖面、生物力学耦合与协议字段都可以独立替换，互不牵连。

use super::geom::{to_bd, Rng, MET_PER_DEG_LAT, MET_PER_DEG_LNG};
use super::realism::{LaneTracker, TravelPlan};
use super::stadium::TrackSpec;

/// 一次跑步在场地上的分段计划。
#[derive(Clone, Debug)]
pub struct ShapedPlan {
    pub travel: TravelPlan,
    /// 进入段（走向起点）的位移（米）。
    pub entry_m: f64,
    /// 收尾段（离开场地）的位移（米）。
    pub exit_m: f64,
    /// 沿场地路线前进的位移（米）。
    pub loop_m: f64,
    /// 场地路线周长（米）。
    #[cfg_attr(not(test), allow(dead_code))] // 几何留档：单圈长度
    pub ring_m: f64,
}

impl ShapedPlan {
    /// 按目标距离与场地周长拆分三段。
    #[cfg_attr(not(test), allow(dead_code))] // 测试构造用
    pub fn new(dist_m: f64, dur_s: f64, ring_m: f64, rng: &mut Rng) -> Self {
        Self::with_access(dist_m, dur_s, ring_m, rng, true)
    }

    /// `allow_access = false` 时不拆进出段：全程都在场地路线上（自定义路线 / 跑道）。
    pub fn with_access(
        dist_m: f64,
        dur_s: f64,
        ring_m: f64,
        rng: &mut Rng,
        allow_access: bool,
    ) -> Self {
        let travel = TravelPlan::plan(dist_m, dur_s, ring_m, rng);
        let travel = if allow_access {
            travel
        } else {
            travel.without_access()
        };
        let access = dist_m * (travel.entry_frac + travel.exit_frac);
        let entry_m = if travel.entry_frac + travel.exit_frac > 1e-9 {
            access * travel.entry_frac / (travel.entry_frac + travel.exit_frac)
        } else {
            0.0
        };
        let exit_m = access - entry_m;
        Self {
            travel,
            entry_m,
            exit_m,
            loop_m: (dist_m - access).max(ring_m * 0.25),
            ring_m: ring_m.max(1.0),
        }
    }

    /// 是否为"只跑环线、不带进出段"的退化形态（距离太短时）。
    #[cfg_attr(not(test), allow(dead_code))] // 测试断言用
    pub fn is_pure_loop(&self) -> bool {
        self.entry_m < 1.0 && self.exit_m < 1.0
    }

    /// 把进场/离场段清零（起点即环线原点）。
    #[cfg_attr(not(test), allow(dead_code))] // 测试构造用
    pub fn without_access(&self) -> Self {
        Self {
            travel: self.travel.without_access(),
            entry_m: 0.0,
            exit_m: 0.0,
            loop_m: self.loop_m + self.entry_m + self.exit_m,
            ring_m: self.ring_m,
        }
    }
}

/// 环线弧长 → 坐标（线性插值），按弧长表**二分**定位。
///
/// 不能复用 `geom::ring_point_at`：它用「首个 arcs[mid] >= s」的二分，在**逆序**累积
/// 的弧长表上会取到错误区间；而 `custom_site_ring` 这类按折线顺序累积、跑者反向
/// 前进的组合正好会产生这种表。
pub fn polyline_at(dense: &[(f64, f64)], arcs: &[f64], s: f64) -> (f64, f64) {
    if dense.is_empty() {
        return (0.0, 0.0);
    }
    if dense.len() == 1 || arcs.len() < 2 {
        return dense[0];
    }
    let total = *arcs.last().unwrap_or(&1.0);
    let s = if total > 0.0 {
        s.rem_euclid(total)
    } else {
        0.0
    };
    let mut lo = 0usize;
    let mut hi = arcs.len() - 1;
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if arcs[mid] <= s {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let a = dense[lo % dense.len()];
    let b = dense[(lo + 1) % dense.len()];
    let span = (arcs[lo + 1] - arcs[lo]).abs();
    let t = if span > 1e-9 {
        ((s - arcs[lo]) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// 几何追踪器：把逐点位移序列映射为平面坐标（米，以质心为原点）。
///
/// 内部状态只有「沿环线的弧长」与「进入/离开段的进度」两件事，横向偏移由
/// `LaneTracker` 逐点推进。
pub struct ShapeTracker {
    plan: ShapedPlan,
    dense: Vec<(f64, f64)>,
    arcs: Vec<f64>,
    lane: LaneTracker,
    /// 累计已用位移（米），含进出段。
    used_m: f64,
    /// 沿环线的弧长（米）。
    s: f64,
    /// 方向（+1 / -1）。
    direction: f64,
    /// 起点（环上的弧长位置）。
    s0: f64,
    /// 上一次推进的瞬时速度（m/s），仅用于推算「下一步大概在哪」。
    last_speed_mps: f64,
}

impl ShapeTracker {
    /// `start_at_ring_origin = true` 时起点固定在环线原点（自定义路线/跑道语义）。
    pub fn new(
        plan: ShapedPlan,
        dense: Vec<(f64, f64)>,
        arcs: Vec<f64>,
        rng: &mut Rng,
        start_at_ring_origin: bool,
        start_s: Option<f64>,
    ) -> Self {
        let ring_m = arcs.last().copied().unwrap_or(1.0).max(1.0);
        let s0 = if start_at_ring_origin {
            start_s.unwrap_or(0.0)
        } else {
            match start_s {
                Some(v) => v,
                None => rng.uniform(0.0, ring_m),
            }
        };
        let direction = if start_at_ring_origin {
            1.0
        } else if rng.random() < 0.5 {
            1.0
        } else {
            -1.0
        };
        Self {
            plan,
            dense,
            arcs,
            lane: LaneTracker::new(0.05, 1.1, rng),
            used_m: 0.0,
            s: s0,
            direction,
            s0,
            last_speed_mps: 3.0,
        }
    }

    /// 复制一份独立状态（用于预估路径长度，不影响原追踪器）。
    #[cfg_attr(not(test), allow(dead_code))] // 探针克隆
    pub fn clone_for_probe(&self) -> Self {
        Self {
            plan: self.plan.clone(),
            dense: self.dense.clone(),
            arcs: self.arcs.clone(),
            lane: self.lane.clone(),
            used_m: self.used_m,
            s: self.s,
            direction: self.direction,
            s0: self.s0,
            last_speed_mps: self.last_speed_mps,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))] // 按需读取计划
    pub fn plan(&self) -> &ShapedPlan {
        &self.plan
    }

    /// 环线原点坐标。
    pub fn ring_origin(&self) -> (f64, f64) {
        polyline_at(&self.dense, &self.arcs, self.s0)
    }

    /// 推算「再前进一个采样周期大概会在哪」（不改变内部状态）。
    ///
    /// 用于把绕行写成连续位移：偏移量乘以「本步位移 / 本步时长」，因此不会在
    /// 一帧之内跳到几秒之后的位置。
    #[cfg_attr(not(test), allow(dead_code))] // 按需取点
    pub fn next_point(&self) -> (f64, f64) {
        let bonus = self.last_speed_mps.max(0.5) * 2.0;
        let approx = self.s + self.direction * bonus;
        self.ring_at(approx).0
    }

    /// 环线在弧长 `s` 处的坐标与切线单位向量。
    fn ring_at(&self, s: f64) -> ((f64, f64), (f64, f64)) {
        let p = polyline_at(&self.dense, &self.arcs, s);
        let q = polyline_at(&self.dense, &self.arcs, s + 1.0);
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let n = (dx * dx + dy * dy).sqrt().max(1e-9);
        (p, (dx / n, dy / n))
    }

    /// 前进 `step_m` 米，返回该点的平面坐标。
    ///
    /// 三个分段（进场 / 绕圈 / 离场）各自用**参数 t** 表示进度，坐标由 `geom` 纯函数
    /// 给出，不做任何"跨帧混合"。这样位移的连续性只依赖 t 的连续性：
    ///
    /// ```text
    /// 进场:   P(t) = A · (1 - t)             A = 场地路线在起点处的外法向偏移点
    /// 绕圈:   P(t) = C(s0 + dir·L·t) + lane   C = 场地路线，lane = 横向漂移（差分累加）
    /// 离场:   P(t) = C(s_end)·(1-t) + B·t     B = 终点侧外法向偏移点
    /// ```
    ///
    /// 横向漂移只以**增量**形式叠加，保证「轨迹上量到的距离」与「上报的累计距离」
    /// 是同一件事。
    pub fn advance(&mut self, step_m: f64, dt_s: f64, rng: &mut Rng) -> (f64, f64) {
        let lateral_prev = self.lane.offset();
        let lateral = self.lane.step(dt_s, rng);
        let dl = lateral - lateral_prev;
        self.last_speed_mps = if dt_s > 0.0 { step_m / dt_s } else { 0.0 };
        let (entry_span, exit_span, loop_span) =
            (self.plan.entry_m, self.plan.exit_m, self.plan.loop_m);
        let here = self.used_m;
        self.used_m += step_m;
        let after = self.used_m;

        // ① 进场段
        if entry_span > 0.0 && here < entry_span {
            if step_m <= 0.0001 {
                // 起点哨兵：停在进场段的起点（场地外）
                let (ax, ay) = self.entry_point();
                return (ax + dl, ay);
            }
            let frac = after / entry_span;
            let t = 1.0 - (1.0 - frac).clamp(0.0, 1.0);
            let (ax, ay) = self.entry_point();
            let (cx, cy) = self.ring_at(self.s0).0;
            self.s = self.s0;
            return (ax * (1.0 - t) + cx * t + dl, ay * (1.0 - t) + cy * t);
        }

        // ② 绕圈：弧长按环线周长回绕，绕够 `loop_span` 之后转入离场段。
        let loop_end = entry_span + loop_span;
        if here < loop_end {
            self.s += self.direction * step_m;
            let (bx, by) = self.ring_at(self.s).0;
            return (bx + dl, by);
        }
        // 收尾：把剩余位移按比例落在环线终点上（一旦进入离场段就停在那里）。
        if exit_span <= 0.0 {
            self.s = self.s0 + self.direction * loop_span;
            let (bx, by) = self.ring_at(self.s).0;
            return (bx + dl, by);
        }

        // ③ 离场段
        if step_m <= 0.0001 {
            let (bx, by) = self.ring_at(self.s0 + self.direction * loop_span).0;
            return (bx + dl, by);
        }
        let frac = (after - loop_end) / exit_span;
        let t = frac.clamp(0.0, 1.0);
        let s_end = self.s0 + self.direction * loop_span;
        let (bx, by) = self.ring_at(s_end).0;
        let (ex, ey) = self.exit_point();
        self.s = s_end;
        (
            bx * (1.0 - t) + ex * (1.0 + 0.08 * lateral) * t,
            by * (1.0 - t) + ey * t,
        )
    }

    /// 进场段起点：场地路线在起点处的外法向偏移点。
    fn entry_point(&self) -> (f64, f64) {
        let (p, t) = self.ring_at(self.s0);
        let side = self.plan.travel.side;
        let (nx, ny) = (-t.1 * side, t.0 * side);
        (p.0 + nx * self.plan.entry_m, p.1 + ny * self.plan.entry_m)
    }

    /// 离场段终点：场地路线在终点处的外法向偏移点（错侧时取反方向）。
    fn exit_point(&self) -> (f64, f64) {
        let s_end = self.s0 + self.direction * self.plan.loop_m;
        let (p, t) = self.ring_at(s_end);
        let side = if self.plan.travel.same_side {
            self.plan.travel.side
        } else {
            -self.plan.travel.side
        };
        let (nx, ny) = (-t.1 * side, t.0 * side);
        (p.0 + nx * self.plan.exit_m, p.1 + ny * self.plan.exit_m)
    }
}

/// 场地路线：返回 (dense, arcs, ring_m)。
///
/// 三档优先级：显式跑道预设 → 打卡点拟合环 → 打卡点质心处的标准 400 m 跑道。
/// 最后一项是"没有 OSM、没有跑道信息"时的兜底：校园里最可能跑的地方就是操场。
#[cfg_attr(not(test), allow(dead_code))] // 无围栏时的兜底场地环，测试在用
pub fn site_ring(points_bd: &[(f64, f64)], spec: TrackSpec) -> Result<SiteRing, String> {
    site_ring_with_fences(points_bd, spec, &[])
}

/// 同 [`site_ring`]，额外用电子围栏把跑道直道摆正。
///
/// 围栏 = 场地边界，操场直道顺着场地长边铺。围栏为空时行为与 `site_ring` 一致。
pub fn site_ring_with_fences(
    points_bd: &[(f64, f64)],
    spec: TrackSpec,
    fences: &[Vec<(f64, f64)>],
) -> Result<SiteRing, String> {
    use super::geom::make_point_ring;
    use super::stadium::make_stadium_ring_with_fences;

    if spec != TrackSpec::Auto {
        let (dense, arcs, center) = make_stadium_ring_with_fences(points_bd, spec, true, fences)?;
        let ring_m = arcs.last().copied().unwrap_or(400.0);
        return Ok(SiteRing {
            dense,
            arcs,
            center,
            ring_m,
            genuine: true,
        });
    }
    if points_bd.len() >= 3 {
        // 打卡点本身就是路线：用 Catmull-Rom 把它们连成闭合环。轨迹**必然经过**
        // 每个必过点（与服务端打卡判定一致），而曲线化后的拐角是圆滑的——
        // 折线急拐角会让「弧长前进量」和「实际位移」严重脱节，进而把某几段的速度
        // 压到正常配速的一半。
        let (dense, arcs, center) = make_point_ring(points_bd);
        let ring_m = arcs.last().copied().unwrap_or(400.0);
        return Ok(SiteRing {
            dense,
            arcs,
            center,
            ring_m,
            genuine: true,
        });
    }
    // 兜底：以打卡点质心（不足 3 点时用点本身 / 原点）为中心的标准跑道。
    let center = centroid(points_bd);
    let (dense, arcs, center) = make_stadium_ring_with_fences(
        &[center, (center.0 + 1e-6, center.1)],
        TrackSpec::M400,
        true,
        fences,
    )?;
    let ring_m = arcs.last().copied().unwrap_or(400.0);
    Ok(SiteRing {
        dense,
        arcs,
        center,
        ring_m,
        genuine: false,
    })
}

fn centroid(points: &[(f64, f64)]) -> (f64, f64) {
    if points.is_empty() {
        return (0.0, 0.0);
    }
    let n = points.len() as f64;
    (
        points.iter().map(|p| p.0).sum::<f64>() / n,
        points.iter().map(|p| p.1).sum::<f64>() / n,
    )
}

/// 场地路线（平面坐标 + 弧长表 + BD 质心）。
pub struct SiteRing {
    pub dense: Vec<(f64, f64)>,
    pub arcs: Vec<f64>,
    pub center: (f64, f64),
    pub ring_m: f64,
    /// true = 路线来自真实打卡点几何；false = 兜底标准跑道。
    #[cfg_attr(not(test), allow(dead_code))] // 计划留档：是否真实场地
    pub genuine: bool,
}

impl SiteRing {
    /// 平面坐标 → BD-09。
    pub fn to_bd(&self, x: f64, y: f64) -> (f64, f64) {
        to_bd(x, y, self.center.0, self.center.1)
    }

    /// 平面上离 BD 点最近的点（米），用于"吸附成本"判断。
    pub fn distance_to(&self, x: f64, y: f64, bd: (f64, f64)) -> f64 {
        let px = (bd.1 - self.center.1) * MET_PER_DEG_LNG;
        let py = (bd.0 - self.center.0) * MET_PER_DEG_LAT;
        ((px - x).powi(2) + (py - y).powi(2)).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::realism::{audit, MAX_CIRCULARITY};

    fn square_points() -> Vec<(f64, f64)> {
        vec![
            (39.9000, 116.4000),
            (39.9000, 116.4020),
            (39.9020, 116.4020),
            (39.9020, 116.4000),
        ]
    }

    #[test]
    fn access_segments_are_short_and_inside_budget() {
        let mut rng = Rng::new(9);
        for ring in [400.0f64, 1200.0, 2400.0] {
            let plan = ShapedPlan::new(2000.0, 720.0, ring, &mut rng);
            let access = plan.entry_m + plan.exit_m;
            assert!(access >= 0.0 && access <= 2000.0 * TravelPlan::MAX_ACCESS_FRAC + 1e-6);
            assert!((plan.entry_m + plan.exit_m + plan.loop_m - 2000.0).abs() < 1e-6);
        }
    }

    #[test]
    fn tracker_traverses_loop_and_leaves() {
        let mut rng = Rng::new(21);
        let ring = site_ring(&square_points(), TrackSpec::Auto).unwrap();
        let plan = ShapedPlan::new(2000.0, 720.0, ring.ring_m, &mut rng);
        let ring_m = ring.ring_m;
        let budget: f64 = plan.entry_m + plan.exit_m + plan.loop_m;
        let mut tracker = ShapeTracker::new(plan, ring.dense, ring.arcs, &mut rng, false, None);
        let mut pts = Vec::new();
        let mut remaining: f64 = budget;
        while remaining > 0.05 {
            let step = remaining.min(3.4 * 5.0);
            pts.push(tracker.advance(step, 5.0, &mut rng));
            remaining -= step;
        }
        assert!(pts.len() > 50);
        // 起点与终点的距离应体现"进出不同侧"：不是零。
        let (a, b) = (pts[0], *pts.last().unwrap());
        let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        assert!(d > 1.0, "起终点几乎重合: {d}");
        assert!(ring_m > 100.0);
    }

    #[test]
    fn site_ring_falls_back_to_standard_track() {
        let ring = site_ring(&[(39.9, 116.4)], TrackSpec::Auto).unwrap();
        assert!(!ring.genuine);
        assert!((ring.ring_m - 400.0).abs() < 6.0, "ring={}", ring.ring_m);
    }

    #[test]
    fn circularity_ceiling_is_below_analytic_circle() {
        // 直接验证阈值的意义：解析圆 > 阈值。
        assert!(MAX_CIRCULARITY < 1.0);
        let _ = audit;
    }
}
