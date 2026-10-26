//! 生物力学联动：配速 v、步频 f_cadence、步幅 L_stride 的非线性耦合。
//!
//! 原先放在 `crates/route-planner` 里，随路网模式一起被引入；但它是**所有生成模式
//! 共用**的（经典/跑道/自定义都要步频-步幅自洽），因此路网删除后迁到这里。
//!
//!   硬约束：v = (f_cadence / 60) · L_stride   （v: m/s，f: 步/分，L: 米）
//!
//! 由速度与疲劳度确定 (步频, 步幅)，**始终严格满足等式**。

/// 步态结果。
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // stride 由测试断言 v = cadence/60*stride 时读取
pub struct Gait {
    /// 步频（步/分）。
    pub cadence: f64,
    /// 步幅（米）。
    #[cfg_attr(not(test), allow(dead_code))] // 步幅：测试断言 v = cadence/60*stride
    pub stride: f64,
}

/// 由速度与疲劳度计算步态，严格满足 `v = cadence/60 * stride`。
///
/// 步频随速度升高，疲劳略降步频；步幅落在 [0.5, 1.7] 包络内。
pub fn gait(v: f64, fatigue: f64) -> Gait {
    // 先定步幅、再反解步频。
    //
    // 原来是「步频 = 150 + 12v」再算步幅，慢配速下步幅会掉到 0.72 m
    // （超出真实包络）；而真实跑者慢配速时是靠**降步频**而不是
    // 随便缩步幅。实测通过审核的记录：5:30-6:07/km 时步频 120-126。
    // 步幅目标取 1.0 m 附近（真实跑步的典型值），由它反解步频：
    // 2.1 m/s -> 约 124 步/分，2.8 m/s -> 约 165 步/分。
    let stride_target = (1.00 + 0.02 * v).clamp(0.85, 1.15);
    let mut cadence = (v * 60.0 / stride_target) * (1.0 - 0.05 * fatigue);
    cadence = cadence.clamp(110.0, 220.0);
    let mut stride = v * 60.0 / cadence;
    // 步幅越界时反解步频，保持等式成立。
    if stride > 1.7 {
        stride = 1.7;
        cadence = v * 60.0 / stride;
    } else if stride < 0.78 {
        stride = 0.78;
        cadence = v * 60.0 / stride;
    }
    Gait { cadence, stride }
}

/// 由步频与步幅计算速度（m/s）。
#[cfg_attr(not(test), allow(dead_code))] // 仅供测试使用
pub fn speed_from(cadence: f64, stride: f64) -> f64 {
    (cadence / 60.0) * stride
}

/// 疲劳度（0..0.6）：由已跑里程（公里）线性抬升，0.5km 后开始，封顶 0.6。
pub fn fatigue_from_km(km: f64) -> f64 {
    if km <= 0.5 {
        0.0
    } else {
        ((km - 0.5) * 1.5).min(0.6)
    }
}

/// 步频 OU 耦合：在目标步频序列上做均值回归随机波动，返回逐点步频（步/分）。
///
///   c_{t+1} = c_t + θ(target_i − c_t)·Δt + σ·√Δt·Z,  Z~N(0,1)
///
/// 用于「步频-速度」耦合的随机部分，使步频既跟踪速度又不机械等值。
pub fn ou_cadence_series(
    targets: &[f64],
    dts: &[f64],
    seed: u64,
    theta: f64,
    sigma: f64,
    floor: f64,
    ceil: f64,
) -> Vec<f64> {
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use rand_distr::{Distribution, Normal};

    let mut rng = StdRng::seed_from_u64(seed);
    let normal = Normal::<f64>::new(0.0, 1.0).unwrap();
    let mut out = Vec::with_capacity(targets.len());
    let mut c = targets.first().copied().unwrap_or(150.0);
    for i in 0..targets.len() {
        let dt = dts[i];
        c = c + theta * (targets[i] - c) * dt + sigma * dt.sqrt() * normal.sample(&mut rng);
        c = c.clamp(floor, ceil);
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gait_coupling_equation_holds() {
        for v in [1.9f64, 2.5, 3.5, 5.0, 6.3] {
            for fat in [0.0f64, 0.3, 0.6] {
                let g = gait(v, fat);
                let back = speed_from(g.cadence, g.stride);
                assert!((back - v).abs() < 1e-6, "v={v} back={back}");
                assert!((0.5..=1.7).contains(&g.stride), "stride={}", g.stride);
                assert!((110.0..=230.0).contains(&g.cadence), "cad={}", g.cadence);
            }
        }
    }

    #[test]
    fn cadence_ou_stays_in_envelope() {
        let n = 200;
        let targets: Vec<f64> = (0..n).map(|i| 170.0 + 0.1 * i as f64).collect();
        let dts = vec![5.0; n];
        let cad = ou_cadence_series(&targets, &dts, 42, 0.25, 0.5, 110.0, 220.0);
        assert_eq!(cad.len(), n);
        for c in cad {
            assert!((110.0..=220.0).contains(&c), "cadence={c}");
        }
    }

    #[test]
    fn fatigue_monotonic_capped() {
        assert_eq!(fatigue_from_km(0.4), 0.0);
        assert!(fatigue_from_km(1.0) > 0.0);
        assert_eq!(fatigue_from_km(5.0), 0.6);
    }
}
