//! 官方卡路里/功率公式。
//!
//! 两路都来自真机语料标定，且**必须互相自洽**：同一条记录的 `calorie` 与
//! `avgPower` 反推出的隐含体重应当一致。10 条 policy=1 真机记录里，取现行
//! 口径的 6 条，两路独立算出 61.67 / 61.68 kg，吻合。
//!
//!   calorie  = 体重(kg) × 距离(km) × 1.036
//!   avgPower = 体重(kg) × 均速(m/s) × 0.98
//!
//! 旧的 MET 分段式（33/p+4.1 等）与 1.1 功率系数在真机语料上系统性偏高：
//! 同一体重下 kcal/km 会算到 70+，而真机实测区间是 59.1~67.5（均值 63.9）；
//! 功率系数 1.1 对应隐含体重 68 kg，与热量路算出的 61.7 kg 互相矛盾 ——
//! 两个字段同时上报时，服务端把两式各解一次就能看出体重对不上。
//!
//! 真机在同一距离下 kcal/km 仍有 ±4% 波动（不是纯距离函数，推测按窗 MET
//! 计算），本式是距离型近似；抹掉这点残差需要逐窗模型，收益不大。

/// 官方展示 kcal：体重 × 距离(km) × 1.036。
pub fn official_kcal(weight: f64, total_time: i64, total_dis: f64) -> i64 {
    let _ = total_time;
    if weight <= 0.0 || total_dis <= 0.0 {
        return 0;
    }
    (weight * (total_dis / 1000.0) * 1.036).round() as i64
}

/// 官方平均功率（瓦）：体重 × 均速(m/s) × 0.98。
pub fn avg_power(weight: f64, total_dis: f64, total_time: i64) -> i64 {
    if total_time <= 0 || weight <= 0.0 || total_dis <= 0.0 {
        return 0;
    }
    (weight * (total_dis / total_time as f64) * 0.98).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// kcal/km 落在真机标定区间：62 kg → 64.2 kcal/km。
    #[test]
    fn kcal_per_km_matches_the_calibrated_range() {
        let per_km = official_kcal(62.0, 600, 1000.0) as f64;
        assert!(
            (59.1..=67.5).contains(&per_km),
            "kcal/km = {per_km} 超出真机实测区间 59.1~67.5"
        );
        assert_eq!(per_km, 64.0);
    }

    /// 两路口径反推出的隐含体重必须一致（本模块的核心不变量）。
    #[test]
    fn calorie_and_power_imply_the_same_body_weight() {
        let kcal = official_kcal(62.0, 1220, 3300.0) as f64;
        let power = avg_power(62.0, 3300.0, 1220) as f64;
        let w_from_kcal = kcal / (3.3 * 1.036);
        let w_from_power = power / ((3300.0 / 1220.0) * 0.98);
        assert!(
            (w_from_kcal - w_from_power).abs() < 1.0,
            "两路隐含体重不一致: {w_from_kcal:.2} vs {w_from_power:.2}"
        );
        assert!((w_from_kcal - 62.0).abs() < 1.0);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert_eq!(official_kcal(0.0, 600, 1000.0), 0);
        assert_eq!(official_kcal(62.0, 600, 0.0), 0);
        assert_eq!(avg_power(62.0, 3300.0, 0), 0);
        assert_eq!(avg_power(62.0, 0.0, 600), 0);
    }
}
