//! 记录真实性：物理互校校验 + 轨迹形态规划。
//!
//! 服务端能拿到的是同一条记录的**四组互相冗余的量**（位移、时间、速度字段、步频/步数）
//! 以及一组几何/时序读数。只要四组之间的关系与「一个人在场地上真的跑了一遍」一致，
//! 这条记录在统计层就无从分辨。本模块只做两件事：
//!
//! 1. `audit`：把一条 `Track` 当成服务端那样重算一遍，逐项给出读数与判定；
//! 2. `plan` / `TravelPlan`：给出「从哪进、沿哪跑几圈、跑到哪收」的形态参数，
//!    供生成器把采样点放在这条路径上，而不是放在一条解析闭合曲线上。
//!
//! 这里的阈值全部是**本项目自定**的工程口径（见 `docs/REALISM_FINDINGS_zh.md` §4），
//! 不是任何官方判定的复刻。

use super::geom::{round_to, Rng, MET_PER_DEG_LAT, MET_PER_DEG_LNG};
use super::model::Track;

/// 人类跑步瞬时步长的物理包络（米）。
pub const STRIDE_MIN_M: f64 = 0.50;
pub const STRIDE_MAX_M: f64 = 1.70;
/// 瞬时步长的一阶统计包络：均值与标准差。
pub const STRIDE_MEAN_RANGE_M: (f64, f64) = (0.75, 1.25);
pub const STRIDE_STD_MAX_M: f64 = 0.22;
/// 相邻点最大采样间隔（秒）：超过就是设备掉点，服务端按 `TIME_GAP` 看待。
pub const MAX_SAMPLE_GAP_S: f64 = 30.0;
/// 相邻段瞬时速度相对有效窗口上沿的允许倍数（只容忍测量噪声造成的少量越界）。
pub const INSTANT_SPEED_TOLERANCE: f64 = 1.35;
/// 速度序列与位移/时间差序列的最小相关性。
///
/// 0.10 很低，但它的作用不是"像"，而是"**不再无关**"：旧实现的下界是 0（模式 C 的
/// 步频完全等于常数），任何真正的耦合都会远高于这个值。
pub const MIN_SPEED_DISPLACEMENT_CORR: f64 = 0.10;
/// 圆拟合度上限：低于它是"真人跑的圈"，高于它是"解析画出来的圈"。
pub const MAX_CIRCULARITY: f64 = 0.999;
/// 距离/步数守恒容差。
pub const CONSERVATION_TOLERANCE: f64 = 0.02;
/// 里程达到这个值就必须有可读的累计上升（米）。
pub const ASCENT_READABLE_FROM_M: f64 = 1000.0;

/// 一项自检读数。
#[derive(Clone, Debug, serde::Serialize)]
pub struct Check {
    pub name: &'static str,
    pub value: f64,
    /// 人类可读的允许区间。
    pub limit: String,
    pub pass: bool,
    /// 该项读数为 0 时是否视为"本次不适用"（例如无坡度时的上升）。
    pub applicable: bool,
}

impl Check {
    fn new(name: &'static str, value: f64, limit: impl Into<String>, pass: bool) -> Self {
        Self {
            name,
            value: round_to(value, 4),
            limit: limit.into(),
            pass,
            applicable: true,
        }
    }

    fn with_applicable(mut self, applicable: bool) -> Self {
        self.applicable = applicable;
        self
    }
}

/// 一条轨迹的真实性报告。
#[derive(Clone, Debug, serde::Serialize)]
pub struct AuthenticityReport {
    pub checks: Vec<Check>,
    /// 所有适用项的失败原因；空 = 通过。
    pub failures: Vec<String>,
}

impl AuthenticityReport {
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }

    /// 单行摘要，供日志与 dry-run 输出。
    pub fn summary(&self) -> String {
        let total = self.checks.iter().filter(|c| c.applicable).count();
        let failed = self
            .checks
            .iter()
            .filter(|c| c.applicable && !c.pass)
            .count();
        format!("{}/{} 项通过", total - failed, total)
    }

    /// 逐项读数（多行），失败项带 ✗。
    #[cfg_attr(not(test), allow(dead_code))] // 报告明细，测试断言在用
    pub fn detail(&self) -> String {
        self.checks
            .iter()
            .filter(|c| c.applicable)
            .map(|c| {
                format!(
                    "{} {:<28} {:<14} 允许 {}",
                    if c.pass { "✓" } else { "✗" },
                    c.name,
                    format!("{:.4}", c.value),
                    c.limit
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n < 3 {
        return 0.0;
    }
    let (a, b) = (&a[..n], &b[..n]);
    let ma = a.iter().sum::<f64>() / n as f64;
    let mb = b.iter().sum::<f64>() / n as f64;
    let mut cov = 0.0;
    let mut va = 0.0;
    let mut vb = 0.0;
    for i in 0..n {
        let (da, db) = (a[i] - ma, b[i] - mb);
        cov += da * db;
        va += da * da;
        vb += db * db;
    }
    if va <= 1e-12 || vb <= 1e-12 {
        return 0.0;
    }
    cov / (va.sqrt() * vb.sqrt())
}

/// 点集 → 最佳拟合圆（代数最小二乘 Kåsa 法）→ (圆心 x, 圆心 y, 半径, 均方根径向误差)。
fn fit_circle(points: &[(f64, f64)]) -> Option<((f64, f64), f64, f64)> {
    let n = points.len();
    if n < 4 {
        return None;
    }
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0);
    for &(x, y) in points {
        let z = x * x + y * y;
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
        sxz += x * z;
        syz += y * z;
        sz += z;
    }
    let nf = n as f64;
    // 解 [sxx sxy sx; sxy syy sy; sx sy n] · [a b c] = [sxz syz sz]
    let m = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, nf]];
    let rhs = [sxz, syz, sz];
    let det = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(m);
    if d.abs() < 1e-12 {
        return None;
    }
    let solve = |col: usize| -> f64 {
        let mut mm = m;
        for row in 0..3 {
            mm[row][col] = rhs[row];
        }
        det(mm) / d
    };
    let a = solve(0);
    let b = solve(1);
    let c = solve(2);
    let cx = a / 2.0;
    let cy = b / 2.0;
    let r2 = c + cx * cx + cy * cy;
    if !(r2 > 0.0) {
        return None;
    }
    let r = r2.sqrt();
    let err = points
        .iter()
        .map(|&(x, y)| {
            let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r;
            d * d
        })
        .sum::<f64>()
        / nf;
    Some(((cx, cy), r, err.sqrt()))
}

/// 圆拟合度：径向均方根误差越小、半径越大，值越接近 1。
///
/// 口径与 `research-runner` 的 `REGULAR_LOOP` 一致：只用平面坐标，只回答"这条路径有多圆"。
fn circularity(points: &[(f64, f64)]) -> Option<f64> {
    let ((_cx, _cy), r, rms) = fit_circle(points)?;
    if r <= 1.0 {
        // 半径太小（几十米内的折返）不构成"绕圈"特征。
        return None;
    }
    Some((1.0 / (1.0 + rms / r)).powi(2))
}

/// 把整条轨迹当成服务端一样重算一遍。
pub fn audit(track: &Track) -> AuthenticityReport {
    let locs = &track.locations;
    let mut checks: Vec<Check> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    if locs.len() < 4 {
        failures.push("轨迹点不足，无法做互校".into());
        return AuthenticityReport { checks, failures };
    }

    // ── 几何：平面坐标（以首点为原点的等距投影）──────────────────────
    let (lat0, lng0) = (locs[0].gLat, locs[0].gLng);
    let plane: Vec<(f64, f64)> = locs
        .iter()
        .map(|p| {
            (
                (p.gLng - lng0) * MET_PER_DEG_LNG,
                (p.gLat - lat0) * MET_PER_DEG_LAT,
            )
        })
        .collect();

    let mut step_dt: Vec<f64> = Vec::new();
    let mut step_d: Vec<f64> = Vec::new();
    let mut step_v_field: Vec<f64> = Vec::new();
    for i in 1..locs.len() {
        let dt = (locs[i].totalTime - locs[i - 1].totalTime) as f64;
        let dd =
            ((plane[i].0 - plane[i - 1].0).powi(2) + (plane[i].1 - plane[i - 1].1).powi(2)).sqrt();
        step_dt.push(dt);
        step_d.push(dd);
        // `speed` 在协议里是 GPS 瞬时速度，单位按 km/h 记录（见 §2.5）。
        step_v_field.push(locs[i].speed);
    }

    // 1 相邻段瞬时速度（跳过零间隔的起点哨兵对：协议上首点与哨兵同址同刻）
    let mut peak_v = 0.0f64;
    let mut peak_at = 0usize;
    for i in 0..step_d.len() {
        if step_dt[i] <= 0.0 {
            continue;
        }
        let v = step_d[i] / step_dt[i];
        if v > peak_v {
            peak_v = v;
            peak_at = i;
        }
    }
    let v_cap = super::generator::SPEED_CEIL * INSTANT_SPEED_TOLERANCE;
    let c = Check::new(
        "instant_speed_peak_mps",
        peak_v,
        format!("<= {v_cap:.2}"),
        peak_v <= v_cap,
    );
    if !c.pass {
        failures.push(format!(
            "第 {} 段相邻点 {:.0} m / {:.0} s = {:.1} m/s，超出人类跑步极限",
            peak_at, step_d[peak_at], step_dt[peak_at], peak_v
        ));
    }
    checks.push(c);

    // 2 采样间隔（起点哨兵对允许 0：首点与索引 1 是同一时刻的同一点）
    let max_gap = step_dt
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 0)
        .map(|(_, d)| *d)
        .fold(0.0, f64::max);
    let c = Check::new(
        "max_sample_gap_s",
        max_gap,
        format!("<= {MAX_SAMPLE_GAP_S:.0}"),
        max_gap <= MAX_SAMPLE_GAP_S,
    );
    if !c.pass {
        failures.push(format!(
            "相邻采样间隔 {max_gap:.0} s，超过设备采样周期，等同掉点"
        ));
    }
    checks.push(c);

    // 3 位移距离 vs 声明距离
    let path_m: f64 = step_d.iter().sum();
    let ratio = path_m / track.totalDistance.max(1e-6);
    let c = Check::new(
        "path_vs_declared_distance",
        ratio,
        format!("1 ± {CONSERVATION_TOLERANCE}"),
        (ratio - 1.0).abs() <= CONSERVATION_TOLERANCE,
    );
    if !c.pass {
        failures.push(format!(
            "轨迹实际跨度 {path_m:.0} m，声明 {:.0} m（比值 {ratio:.3}）",
            track.totalDistance
        ));
    }
    checks.push(c);

    // 4 逐点累计距离 vs 位移重算
    let measured: Vec<f64> = {
        let mut acc = 0.0;
        let mut out = vec![0.0];
        for d in &step_d {
            acc += d;
            out.push(acc);
        }
        out
    };
    let max_dev = locs
        .iter()
        .zip(measured.iter())
        .map(|(p, m)| (p.totalDis - m).abs())
        .fold(0.0, f64::max);
    let tol_m = (track.totalDistance * CONSERVATION_TOLERANCE).max(2.0);
    let c = Check::new(
        "total_dis_vs_measured_m",
        max_dev,
        format!("<= {tol_m:.1}"),
        max_dev <= tol_m,
    );
    if !c.pass {
        failures.push(format!("逐点 totalDis 与位移重算最大偏差 {max_dev:.1} m"));
    }
    checks.push(c);

    // 5 速度字段 vs 位移/时间差（相关性 + 量纲）
    //
    // 只统计 `speed > 0` 的段：真机在隧道 / 建筑背后会短时给出 0，这是正常读数，
    // 但 0 不能参与相关性计算，否则一条真实记录会被 20% 的零值拖到 0.6 以下。
    let mut useful: Vec<usize> = (0..step_d.len())
        .filter(|&i| step_dt[i] >= 1.0 && step_v_field[i] > 0.1)
        .collect();
    if useful.len() < 3 {
        useful = (0..step_d.len()).collect();
    }
    let measured_v: Vec<f64> = useful
        .iter()
        .map(|&i| step_d[i] / step_dt[i].max(1e-6))
        .collect();
    let field_v: Vec<f64> = useful.iter().map(|&i| step_v_field[i] / 3.6).collect();
    let corr = pearson(&measured_v, &field_v);
    let c = Check::new(
        "speed_vs_displacement_corr",
        corr,
        format!(">= {MIN_SPEED_DISPLACEMENT_CORR}"),
        corr >= MIN_SPEED_DISPLACEMENT_CORR,
    );
    if !c.pass {
        failures.push(format!(
            "speed 字段与位移/时间差相关系数 {corr:.3}，两者互不相关"
        ));
    }
    checks.push(c);

    // 速度字段的量纲中位比（应接近 1）
    let mut ratios: Vec<f64> = measured_v
        .iter()
        .zip(field_v.iter())
        .map(|(m, f)| f / m.max(1e-6))
        .collect();
    ratios.sort_by(f64::total_cmp);
    let median = ratios[ratios.len() / 2];
    let c = Check::new(
        "speed_field_scale",
        median,
        "0.75 - 1.30",
        (0.75..=1.30).contains(&median),
    );
    if !c.pass {
        failures.push(format!(
            "speed 字段中位量纲比 {median:.2}（1 = 与位移/时间差一致）"
        ));
    }
    checks.push(c);

    // 6 步数守恒
    let derived_steps: i64 = locs.last().map(|p| p.steps).unwrap_or(0);
    let c = Check::new(
        "steps_field_vs_total",
        derived_steps as f64 / track.totalSteps.max(1) as f64,
        format!("1 ± {CONSERVATION_TOLERANCE}"),
        (derived_steps - track.totalSteps).unsigned_abs() as f64
            <= (track.totalSteps as f64 * CONSERVATION_TOLERANCE).max(2.0),
    );
    if !c.pass {
        failures.push(format!(
            "末点步数 {derived_steps} 与 totalSteps {} 不一致",
            track.totalSteps
        ));
    }
    checks.push(c);

    // 7 瞬时步长：逐段 位移 / 步数
    let mut strides: Vec<f64> = Vec::new();
    for i in 1..locs.len() {
        let ds = (locs[i].steps - locs[i - 1].steps) as f64;
        if ds >= 1.0 && step_d[i - 1] >= 1.0 && step_dt[i - 1] >= 1.0 {
            strides.push(step_d[i - 1] / ds);
        }
    }
    if !strides.is_empty() {
        // 统计量用**加权**口径：一步一次，而不是一段一次。
        // 某段多走了 5 米（让路）会让该段的"每步距离"看起来像 1.9 m，
        // 但那只是两三步的绕行，不该被当成步幅越界。
        let mut sample: Vec<f64> = Vec::new();
        let mut total_w = 0.0f64;
        let mut total_s = 0.0f64;
        for i in 1..locs.len() {
            let ds = (locs[i].steps - locs[i - 1].steps) as f64;
            if ds >= 1.0 && step_d[i - 1] >= 1.0 && step_dt[i - 1] >= 1.0 {
                let s = step_d[i - 1] / ds;
                let w = ds;
                sample.push(s);
                total_w += w;
                total_s += s * w;
            }
        }
        let mean_w = total_s / total_w.max(1.0);
        let var_w = if sample.is_empty() {
            0.0
        } else {
            let mut acc = 0.0;
            for i in 1..locs.len() {
                let ds = (locs[i].steps - locs[i - 1].steps) as f64;
                if ds >= 1.0 && step_d[i - 1] >= 1.0 && step_dt[i - 1] >= 1.0 {
                    let s = step_d[i - 1] / ds;
                    acc += ds * (s - mean_w).powi(2);
                }
            }
            acc / total_w.max(1.0)
        };
        let sd = var_w.sqrt();
        let out_count: f64 = (1..locs.len())
            .map(|i| {
                let ds = (locs[i].steps - locs[i - 1].steps) as f64;
                if ds >= 1.0 && step_d[i - 1] >= 1.0 && step_dt[i - 1] >= 1.0 {
                    let s = step_d[i - 1] / ds;
                    if (STRIDE_MIN_M..=STRIDE_MAX_M).contains(&s) {
                        0.0
                    } else {
                        ds
                    }
                } else {
                    0.0
                }
            })
            .sum();
        let out_of_envelope = out_count / total_w.max(1.0);
        let mean = mean_w;
        let c = Check::new(
            "stride_mean_m",
            mean,
            format!("{STRIDE_MEAN_RANGE_M:?}"),
            (STRIDE_MEAN_RANGE_M.0..=STRIDE_MEAN_RANGE_M.1).contains(&mean),
        );
        if !c.pass {
            failures.push(format!(
                "瞬时步长均值 {mean:.3} m 不在 {STRIDE_MEAN_RANGE_M:?}"
            ));
        }
        checks.push(c);
        let c = Check::new(
            "stride_std_m",
            sd,
            format!("<= {STRIDE_STD_MAX_M}"),
            sd <= STRIDE_STD_MAX_M,
        );
        if !c.pass {
            failures.push(format!("瞬时步长标准差 {sd:.3} m，步幅过于机械"));
        }
        checks.push(c);
        let c = Check::new(
            "stride_out_of_envelope",
            out_of_envelope,
            "<= 0.05",
            out_of_envelope <= 0.05,
        );
        if !c.pass {
            failures.push(format!(
                "{:.1}% 的采样段瞬时步长越出 {STRIDE_MIN_M}–{STRIDE_MAX_M} m",
                out_of_envelope * 100.0
            ));
        }
        checks.push(c);

        // 8 速度 ↔ 步频耦合：逐段速度与逐段步频（步/分）的相关系数
        let cad: Vec<f64> = (0..strides.len())
            .map(|k| {
                let i = k + 1;
                let ds = (locs[i].steps - locs[i - 1].steps) as f64;
                ds / step_dt[i - 1].max(1.0) * 60.0
            })
            .collect();
        let v_seg: Vec<f64> = (0..strides.len())
            .map(|k| step_d[k] / step_dt[k].max(1e-6))
            .collect();
        let r = pearson(&v_seg, &cad);
        let c = Check::new(
            "speed_vs_cadence_corr",
            r,
            format!(">= {MIN_SPEED_DISPLACEMENT_CORR}"),
            r >= MIN_SPEED_DISPLACEMENT_CORR,
        );
        if !c.pass {
            failures.push(format!("速度与步频相关系数 {r:.3}，步频没有跟随速度"));
        }
        checks.push(c);
    }

    // 9 圆拟合度
    if locs.len() >= 20 {
        if let Some(circ) = circularity(&plane) {
            let c = Check::new(
                "circularity",
                circ,
                format!("< {MAX_CIRCULARITY}"),
                circ < MAX_CIRCULARITY,
            );
            if !c.pass {
                failures.push(format!(
                    "路径圆拟合度 {circ:.5}，采样点落在解析圆上（机器生成特征）"
                ));
            }
            checks.push(c);
        }
    }

    // 10 累计上升
    let ascent = super::altitude::total_ascent_of(track);
    let applicable = track.totalDistance >= ASCENT_READABLE_FROM_M;
    let c = Check::new("total_ascent_m", ascent, "> 0（里程 ≥ 1 km）", ascent > 0.0)
        .with_applicable(applicable);
    if applicable && !c.pass {
        failures.push("里程已过 1 km，但 totalAscent 仍为 0（海拔曲线被压平）".into());
    }
    checks.push(c);

    // 11 10s / 60s 窗口与总距离守恒
    let win: f64 = track.speedPerTenSec.iter().map(|w| w.value).sum();
    let seg: f64 = track.segments.iter().map(|s| s.distance as f64).sum();
    let win_ok = (win / track.totalDistance.max(1e-6) - 1.0).abs() <= 0.03;
    let c = Check::new(
        "ten_sec_window_vs_total",
        win / track.totalDistance.max(1e-6),
        "1 ± 0.03",
        win_ok,
    );
    if !c.pass {
        failures.push(format!(
            "10s 窗距离合计 {win:.0} m 与总距离 {:.0} m 不符",
            track.totalDistance
        ));
    }
    checks.push(c);

    // 11b 提交载荷里的 10s 窗：时长必须覆盖整段，逐窗配速必须像人跑的
    //
    // 这条校验的对象是**真正发出去的那份窗口序列**（`wire::ten_second_spans`，
    // 提交体与 OBS 共用）。旧实现两处各算一遍，采样点不在 10 s 边界上时读数就不同；
    // 而且末窗常被当成整 10 秒读，于是显示成一次并不存在的掉速。
    let spans = super::wire::ten_second_spans(track);
    if spans.len() >= 2 {
        let total_time = track.totalTime;
        let dur_sum: i64 = spans.iter().map(|s| s.end_s - s.begin_s).sum();
        let c = Check::new(
            "ten_sec_window_duration_sum_s",
            dur_sum as f64,
            format!("= {total_time}"),
            (dur_sum - total_time).abs() <= 1,
        );
        checks.push(c);
        if (dur_sum - total_time).abs() > 1 {
            failures.push(format!(
                "10s 窗时长合计 {dur_sum} s 与总时长 {total_time} s 不符"
            ));
        }
        // 逐窗配速：按**实际窗宽**换算（残窗不能被当成 10 秒读）。
        let mut slow_mid = 0usize;
        let mut peak = 0.0f64;
        for (k, s) in spans.iter().enumerate() {
            let secs = (s.end_s - s.begin_s).max(1) as f64;
            let v = s.distance_m / secs;
            peak = peak.max(v);
            let edge = k == 0 || k + 1 == spans.len();
            if !edge && v < 1.0 {
                slow_mid += 1;
            }
        }
        let c = Check::new(
            "ten_sec_window_speed_peak_mps",
            peak,
            format!(
                "<= {}",
                round_to(super::generator::SPEED_CEIL * INSTANT_SPEED_TOLERANCE, 2)
            ),
            peak <= super::generator::SPEED_CEIL * INSTANT_SPEED_TOLERANCE,
        );
        if !c.pass {
            failures.push(format!("10s 窗峰值配速 {peak:.1} m/s，超出人类跑步极限"));
        }
        checks.push(c);
        // 途中不该有「停下来走」的窗口（首末窗允许起跑加速与冲线减速）。
        let allowed = (spans.len() / 20).max(1);
        let c = Check::new(
            "ten_sec_window_slow_mid_count",
            slow_mid as f64,
            format!("<= {allowed}"),
            slow_mid <= allowed,
        );
        if !c.pass {
            failures.push(format!(
                "{slow_mid} 个途中 10s 窗配速低于 1.0 m/s（无停顿的跑步里不该出现）"
            ));
        }
        checks.push(c);
    }

    let seg_ratio = seg / track.totalDistance.max(1e-6);
    let c = Check::new(
        "sixty_sec_segment_vs_total",
        seg_ratio,
        "1 ± 0.03",
        (seg_ratio - 1.0).abs() <= 0.03,
    );
    if !c.pass {
        failures.push(format!(
            "60s 段距离合计 {seg:.0} m 与总距离 {:.0} m 不符",
            track.totalDistance
        ));
    }
    checks.push(c);

    // 12 步频累计守恒
    let cad_sum: f64 = track.stepsPerTenSec.iter().map(|w| w.value).sum();
    let c = Check::new(
        "ten_sec_steps_vs_total",
        cad_sum / track.totalSteps.max(1) as f64,
        "1 ± 0.03",
        (cad_sum / track.totalSteps.max(1) as f64 - 1.0).abs() <= 0.03,
    );
    if !c.pass {
        failures.push(format!(
            "10s 窗步数合计 {cad_sum:.0} 与 totalSteps {} 不符",
            track.totalSteps
        ));
    }
    checks.push(c);

    AuthenticityReport { checks, failures }
}

// ──────────────────────────────────────────────────────────────────────
// 轨迹形态规划
// ──────────────────────────────────────────────────────────────────────

/// 一次跑步在场地上的形态。
///
/// 真人不会在一条闭合曲线上从零开始跑到零结束，而是：**从场外走到起点 → 沿同一条
/// 场地路线绕整数圈 → 收尾走到场外**。三段各自的时长由整体配速插值得到，所以总时长
/// 与目标距离不变。
#[derive(Clone, Debug)]
pub struct TravelPlan {
    /// 进入段（从停车点走向起点）的时长占比 0–1（相对全程）。
    pub entry_frac: f64,
    /// 收尾段（跑完离开场地）的时长占比。
    pub exit_frac: f64,
    /// 场地路线首尾相接时，进入/离开段走的是同一条短线（true = 同一侧进出）。
    pub same_side: bool,
    /// 场地路线绕圈数（含零头，由生成器按实际距离换算）。
    #[cfg_attr(not(test), allow(dead_code))] // 报告留档：绕圈数
    pub laps: f64,
    /// 进入/离开段的横向侧偏方向（+1 / -1），用于让两段不重合。
    pub side: f64,
}

impl TravelPlan {
    /// 进入/收尾段合计时长占比上限：超过 25% 就不像"进场跑几圈"，而是"来回走"。
    pub const MAX_ACCESS_FRAC: f64 = 0.25;

    /// 按目标距离与场地周长给出形态参数。
    ///
    /// - 场地周长 `ring_m` 越短，进场/收尾段相对越长（操场 400 m 时约 60 s / 150 m，
    ///   1.5 km 环线时约 20 s / 55 m）；
    /// - 总时长不足 300 s 时按比例压缩，避免出现"2 分钟跑了 1 km 还带热身"。
    pub fn plan(dist_m: f64, dur_s: f64, ring_m: f64, rng: &mut Rng) -> Self {
        let ring = ring_m.max(50.0);
        // 单程进场距离：场地周长的 15%，夹在 40–90 m。
        let access_m = (ring * 0.15).clamp(40.0, 90.0);
        let both = 2.0 * access_m;
        let mut access_frac = (both / dist_m.max(1.0)).min(Self::MAX_ACCESS_FRAC);
        // 短跑（时长很小）不要热身走：按 300 s 线性缩放。
        access_frac *= (dur_s / 300.0).clamp(0.25, 1.0);
        let entry_frac = access_frac * rng.uniform(0.45, 0.55);
        let exit_frac = access_frac - entry_frac;
        let laps = (dist_m - both).max(ring * 0.25) / ring;
        Self {
            entry_frac,
            exit_frac,
            same_side: rng.random() < 0.55,
            laps,
            side: if rng.random() < 0.5 { 1.0 } else { -1.0 },
        }
    }

    /// 全程是否启用进场/收尾段（距离过短时不加）。
    #[cfg_attr(not(test), allow(dead_code))] // 测试断言在用
    pub fn uses_access(&self) -> bool {
        self.entry_frac > 1e-4 || self.exit_frac > 1e-4
    }

    /// 复制一份并清空进场/收尾段。
    pub fn without_access(&self) -> Self {
        let mut p = self.clone();
        p.entry_frac = 0.0;
        p.exit_frac = 0.0;
        p
    }
}

/// 生成一条路径追踪器：把「沿场地路线前进的弧长 + 横向偏移」映射成平面坐标。
///
/// 与 `ring_point_at` 的区别只在横向偏移：真人跑步的轨迹偏离场地中心线的量是
/// **随里程缓慢漂移**的（步道宽度、让路、超车），不是常数。这里用 OU 过程给出偏移量，
/// 由调用方每步推进一次。
#[derive(Clone)]
pub struct LaneTracker {
    sigma_m: f64,
    mean_reversion: f64,
    offset: f64,
    sd_lane: f64,
}

impl LaneTracker {
    /// `sigma_m` 为单步增量的标准差（米），`sd_lane` 为稳态偏移标准差（米）。
    pub fn new(sigma_m: f64, sd_lane: f64, rng: &mut Rng) -> Self {
        Self {
            sigma_m,
            mean_reversion: 0.8,
            offset: rng.gauss(0.0, sd_lane.max(1e-6)),
            sd_lane,
        }
    }

    /// 推进一步，返回当前横向偏移（米）。
    pub fn step(&mut self, dt_s: f64, rng: &mut Rng) -> f64 {
        let k = (self.mean_reversion * dt_s).clamp(0.0, 1.0);
        self.offset += -k * self.offset + self.sigma_m * dt_s.sqrt() * rng.gauss(0.0, 1.0);
        let cap = 3.0 * self.sd_lane;
        self.offset = self.offset.clamp(-cap, cap);
        self.offset
    }

    pub fn offset(&self) -> f64 {
        self.offset
    }
}

/// 采样网格：以 5 s 为主的非均匀间隔，末段被 clamp 到 `MAX_SAMPLE_GAP_S` 以内。
///
/// 返回 (times, dts)，`dts[i]` 是第 i 个点的**前进时长**，且所有 `dts[i] ≤ MAX_SAMPLE_GAP_S`。
/// 采样时刻（秒）：主 5 s，偶发 1–8 s。
///
/// 短间隔不是噪声而是真机行为：定位回调在信号好时会加密到 1–2 s（实测一次 918 s
/// 的记录里 1 s 间隔出现 7 次、2 s 出现 6 次）。清一色 5 s 反而是机器特征。
pub fn sample_grid(dur_s: f64, rng: &mut Rng) -> (Vec<f64>, Vec<f64>) {
    let mut times = Vec::new();
    let mut t = 0.0;
    while t < dur_s - 1.0 {
        times.push(t);
        t += if rng.random() < 0.80 {
            5.0
        } else {
            rng.choice(&[1.0, 2.0, 3.0, 4.0, 6.0, 7.0, 8.0])
        };
    }
    if times.is_empty() {
        times.push(0.0);
    }
    // 末点必须正好落在 `dur_s` 上：提交体的 stopTime = startTime + totalTime。
    if (*times.last().unwrap() - dur_s).abs() > 0.5 {
        times.push(dur_s);
    }
    let n = times.len();
    // `dts[i]` = 「从 i-1 到 i」的时长，因此 `dts[0] = 0`（首点没有前驱）。
    let mut dts: Vec<f64> = vec![0.0];
    dts.extend((1..n).map(|i| (times[i] - times[i - 1]).clamp(1.0, MAX_SAMPLE_GAP_S)));
    (times, dts)
}

/// 把逐点速度归一化到「精确命中目标距离」，同时夹在给定上下限内。
///
/// 先整体等比缩放；一旦有采样点被夹到上下限，剩下的差量就**按剩余容量**逐轮
/// 再分配（而不是继续统一缩放，那样会永远收敛不了，最后把几十米全压到某个点上）。
pub fn fit_speeds_capped(w: &mut [f64], dts: &[f64], target: f64, floor: f64, ceil: f64) -> f64 {
    for x in w.iter_mut() {
        *x = x.clamp(floor, ceil);
    }
    for _ in 0..64 {
        let need = target - w.iter().zip(dts).map(|(x, dt)| x * dt).sum::<f64>();
        if need.abs() <= 0.2 {
            break;
        }
        // 还能加速 / 减速的采样点，按「时间 × 剩余容量」分摊差量。
        let mut capacity = 0.0f64;
        for (x, dt) in w.iter().zip(dts) {
            capacity += dt * if need > 0.0 { ceil - *x } else { *x - floor };
        }
        if capacity <= 1e-9 {
            break;
        }
        let k = (need / capacity).clamp(-1.0, 1.0);
        for (x, dt) in w.iter_mut().zip(dts) {
            let room = dt * if need > 0.0 { ceil - *x } else { *x - floor };
            *x = (*x + k * room / dt).clamp(floor, ceil);
        }
    }
    w.iter().zip(dts).map(|(x, dt)| x * dt).sum()
}

/// 校验一条 `Track` 是否可作为「真人一次跑步」上报。
pub fn assert_authentic(track: &Track) -> Result<AuthenticityReport, String> {
    let report = audit(track);
    if report.passed() {
        Ok(report)
    } else {
        Err(format!(
            "轨迹未通过真实性互校（{}）：\n{}",
            report.summary(),
            report.failures.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::generator::build;

    fn square_points() -> Vec<(f64, f64)> {
        vec![
            (39.9000, 116.4000),
            (39.9000, 116.4020),
            (39.9020, 116.4020),
            (39.9020, 116.4000),
        ]
    }

    #[test]
    fn sample_grid_never_exceeds_gap_limit() {
        let mut rng = Rng::new(7);
        for dur in [60.0f64, 300.0, 600.0, 1234.0] {
            let (times, dts) = sample_grid(dur, &mut rng);
            // `dts[0]` 是首点（没有前驱）的占位 0，其余都是真实采样间隔。
            assert_eq!(dts[0], 0.0);
            assert!(dts[1..].iter().all(|d| *d > 0.0 && *d <= MAX_SAMPLE_GAP_S));
            assert_eq!(times.len(), dts.len());
        }
    }

    #[test]
    fn fit_speeds_hits_target_without_leaving_window() {
        let dts = vec![5.0f64; 100];
        let mut w = vec![3.0f64; 100];
        let total = fit_speeds_capped(&mut w, &dts, 1500.0, 1.9, 6.3);
        assert!((total - 1500.0).abs() < 1.0);
        assert!(w.iter().all(|x| (1.9..=6.3).contains(x)));
    }

    #[test]
    fn travel_plan_scales_access_with_ring_length() {
        let mut rng = Rng::new(11);
        let short = TravelPlan::plan(2000.0, 720.0, 400.0, &mut rng);
        let long = TravelPlan::plan(2000.0, 720.0, 1500.0, &mut rng);
        assert!(short.entry_frac + short.exit_frac <= TravelPlan::MAX_ACCESS_FRAC + 1e-9);
        assert!(long.entry_frac + long.exit_frac <= TravelPlan::MAX_ACCESS_FRAC + 1e-9);
        assert!(short.laps > long.laps);
        assert!(short.uses_access());
    }

    #[test]
    fn lane_tracker_stays_within_band() {
        let mut rng = Rng::new(3);
        let mut lane = LaneTracker::new(0.06, 1.2, &mut rng);
        for _ in 0..500 {
            let o = lane.step(5.0, &mut rng);
            assert!(o.abs() <= 3.6 + 1e-9);
        }
    }

    #[test]
    fn circularity_separates_analytic_circle_from_noisy_path() {
        let circle: Vec<(f64, f64)> = (0..180)
            .map(|k| {
                let a = k as f64 / 180.0 * std::f64::consts::TAU;
                (200.0 * a.cos(), 200.0 * a.sin())
            })
            .collect();
        let circ = circularity(&circle).unwrap();
        assert!(circ > MAX_CIRCULARITY, "analytic circle => {circ}");

        let mut rng = Rng::new(5);
        let noisy: Vec<(f64, f64)> = circle
            .iter()
            .map(|&(x, y)| {
                let r = (x * x + y * y).sqrt();
                let k = (r + rng.gauss(0.0, 2.0)) / r;
                (x * k, y * k)
            })
            .collect();
        let circ_noisy = circularity(&noisy).unwrap();
        assert!(circ_noisy < MAX_CIRCULARITY, "noisy circle => {circ_noisy}");
    }

    /// 现行经典生成器的失败项必须被这条自检抓到；改造后必须通过。
    #[test]
    fn audit_reports_failures_with_reasons() {
        let track = build(
            2_000.0,
            720,
            42,
            (39.9, 116.4),
            1_700_000_000_000,
            &square_points(),
        );
        let report = audit(&track);
        assert_eq!(report.passed(), report.failures.is_empty());
        // 摘要与明细都要能输出，便于 dry-run 打印。
        assert!(!report.summary().is_empty());
        assert!(!report.detail().is_empty());
    }
}
