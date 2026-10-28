//! 轨迹海拔覆盖。
//!
//! 海拔是轨迹点的一部分，而不是提交时临时拼出来的字段。统一在生成器
//! 完成后覆盖 `bdA`，这样提交体的 totalAscent、OBS 的 run_data 和每圈
//! elevationGain 都会读取同一份数据。

#![allow(non_snake_case)]

use super::geom::round_to;
use super::model::Track;

pub const ASCENT_NOISE_THRESHOLD_M: f64 = 0.15;

/// 生成器的海拔曲线参数：基准海拔 + 上下震荡幅度。
///
/// 由本地用户填写，因为「本地地面海拔」没有任何可解析的来源（`ground_altitude`
/// 原来硬编码 82.0，与场地实际无关）。实测口径：基准取场地常年读数的中位数，
/// 震荡取上下摆动的半幅。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AltitudeProfile {
    /// 基准海拔（米）：海拔曲线的中线。
    pub base_m: f64,
    /// 震荡幅度（米）：曲线在基准上下摆动的半幅，例如 ±3 m 填 3.0。
    pub oscillation_m: f64,
}

impl Default for AltitudeProfile {
    /// 未指定时的中性默认：沿用历史基线 82.0 m，幅度 1.08 m。
    ///
    /// 两个值都不代表任何真实场地，只为让未填参数的调用方保持旧行为。
    fn default() -> Self {
        Self {
            base_m: 82.0,
            // 0 = 不用固定幅度，改走 `jittered_band` 按记录种子抖动出本次的海拔带。
            // 原来是 1.08（固定幅度），导致每条记录海拔带都只有 3 m 左右，
            // 而实测真跑是 5.00-11.78 m。
            oscillation_m: 0.0,
        }
    }
}

/// 每条记录的海拔带（最高−最低，米）随机范围。
///
/// 标定自 5 条确认的真实记录：5.00 / 6.97 / 9.78 / 11.27 / 11.78 m。
///
/// 曲线经 0.35 一阶低通后实际带宽约是 osc 的 3.2 倍，所以这里的数值
/// 是「osc 输入」而不是最终带宽——最终带宽由 `jittered_band` ÷ 2.2 后再衰减得到。
///
/// **绝不能让每条记录都用同一个跨度**——真人跑步不可能 10 次海拔跨度完全相同。
/// 早期版本用手动区间 `--altitude 6-22` 覆盖，结果 10 条记录的海拔带全部是
/// 16.00，这本身就是「APP参数数据异常」的直接证据。
pub const BAND_RANGE_M: (f64, f64) = (2.2, 10.2);

/// `BAND_RANGE_M` 是**全程海拔带**（最高−最低）。曲线按 `osc/2` 的半幅正弦
/// 生成，峰峰 = osc，所以半幅要取带宽的 1/2。噪声会再撑大约 15%，因此实际
/// 取带宽除以 2.2。
pub const BAND_TO_HALF_AMPLITUDE: f64 = 2.2;

impl AltitudeProfile {
    /// 按记录种子抖动出一个本次专属的海拔带，返回带边界 (base−/2, base+/2) 的
    /// 实际幅度。让两次跑步的海拔跨度天然不同。
    pub fn jittered_band(&self, rng: &mut super::geom::Rng) -> f64 {
        if self.oscillation_m > 0.0 {
            // 用户明确了幅度就尊重，但仍加 ±3% 抖动，避免每次都一模一样
            self.oscillation_m * rng.uniform(0.97, 1.03)
        } else {
            rng.uniform(BAND_RANGE_M.0, BAND_RANGE_M.1) / BAND_TO_HALF_AMPLITUDE
        }
    }

    pub fn validate(self) -> Result<Self, String> {
        if !self.base_m.is_finite() || !(-500.0..=9000.0).contains(&self.base_m) {
            return Err("基准海拔必须是 -500 到 9000 米之间的数字".into());
        }
        if !self.oscillation_m.is_finite() || !(0.0..=200.0).contains(&self.oscillation_m) {
            return Err("震荡幅度必须是 0 到 200 米之间的数字".into());
        }
        Ok(self)
    }
}

pub fn positive_ascent_delta(delta_m: f64) -> f64 {
    if delta_m - ASCENT_NOISE_THRESHOLD_M > 1e-9 {
        delta_m
    } else {
        0.0
    }
}

/// 爬升量：优先用手动海拔覆盖时记下的值（见 `Track::altitude_gain_override`）。
pub fn total_ascent_of(track: &Track) -> f64 {
    match track.altitude_gain_override {
        Some(v) => round_to(v, 2),
        None => total_ascent(&track.locations),
    }
}

pub fn total_ascent(locs: &[super::model::GenPoint]) -> f64 {
    round_to(
        locs.windows(2)
            .map(|pair| positive_ascent_delta(pair[1].bdA - pair[0].bdA))
            .sum(),
        2,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AltitudeRange {
    pub min_m: f64,
    pub max_m: f64,
}

fn validate_altitude(altitude_m: f64) -> Result<(), String> {
    if !altitude_m.is_finite() || !(-500.0..=9000.0).contains(&altitude_m) {
        return Err("海拔必须是 -500 到 9000 米之间的数字".into());
    }
    Ok(())
}

fn validate_range(min_m: f64, max_m: f64) -> Result<AltitudeRange, String> {
    validate_altitude(min_m)?;
    validate_altitude(max_m)?;
    if !(max_m > min_m) {
        return Err("海拔区间的上限必须大于下限".into());
    }
    if max_m - min_m > 200.0 {
        return Err("海拔区间跨度不能超过 200 米".into());
    }
    Ok(AltitudeRange { min_m, max_m })
}

pub fn override_bd_a(track: &mut Track, altitude_m: f64) -> Result<(), String> {
    validate_altitude(altitude_m)?;
    for point in &mut track.locations {
        point.bdA = round_to(altitude_m, 2);
        point.hasAltitude = true;
    }
    // 固定海拔：曲线被压平，爬升必须是 0，不能拿噪声正差分累加成几百米。
    track.altitude_gain_override = Some(0.0);
    Ok(())
}

/// 将生成器的海拔曲线平滑映射到用户指定的绝对海拔区间。
pub fn override_bd_a_range(track: &mut Track, range: AltitudeRange) -> Result<(), String> {
    let range = validate_range(range.min_m, range.max_m)?;
    let (mut current_min, mut current_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for point in &track.locations {
        current_min = current_min.min(point.bdA);
        current_max = current_max.max(point.bdA);
    }
    let span = current_max - current_min;
    let target_span = range.max_m - range.min_m;
    for point in &mut track.locations {
        let mapped = if span.abs() < f64::EPSILON {
            (range.min_m + range.max_m) / 2.0
        } else {
            range.min_m + ((point.bdA - current_min) / span) * target_span
        };
        point.bdA = round_to(mapped.clamp(range.min_m, range.max_m), 2);
        point.hasAltitude = true;
    }
    // 区间映射后曲线被线性缩放，爬升应等于「最高 − 最低」这个总起伏，
    // 而不是重算那条已经被改写过的曲线（重算会把缩放后的噪声也算进去）。
    track.altitude_gain_override = Some(target_span);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::generator::build;

    fn points() -> Vec<(f64, f64)> {
        vec![(38.901678, 121.540241), (38.902564, 121.541233)]
    }

    #[test]
    fn override_replaces_every_bd_a_and_clears_ascent() {
        let mut track = build(1200.0, 600, 7, (38.9, 121.54), 1_700_000_000_000, &points());
        override_bd_a(&mut track, 36.75).unwrap();
        assert!(track.locations.iter().all(|p| p.bdA == 36.75));
        assert_eq!(super::total_ascent_of(&track), 0.0);
    }

    #[test]
    fn rejects_non_finite_or_out_of_range_values() {
        let mut track = build(1000.0, 500, 1, (38.9, 121.54), 1_700_000_000_000, &points());
        assert!(override_bd_a(&mut track, f64::NAN).is_err());
        assert!(override_bd_a(&mut track, 9001.0).is_err());
    }

    #[test]
    fn range_mapping_stays_inside_requested_bounds() {
        let mut track = build(1200.0, 600, 7, (38.9, 121.54), 1_700_000_000_000, &points());
        override_bd_a_range(
            &mut track,
            AltitudeRange {
                min_m: 11.6,
                max_m: 22.8,
            },
        )
        .unwrap();
        assert!(track
            .locations
            .iter()
            .all(|p| (11.6..=22.8).contains(&p.bdA)));
    }

    #[test]
    fn ascent_ignores_noise_at_and_below_point_fifteen_metres() {
        assert_eq!(positive_ascent_delta(0.15), 0.0);
        assert_eq!(positive_ascent_delta(0.1500000000000004), 0.0);
        assert_eq!(positive_ascent_delta(0.150001), 0.150001);
    }
}
