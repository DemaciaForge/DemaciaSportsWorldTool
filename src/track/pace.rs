//! 配速窗口：由**学校规则**决定，而不是程序写死。
//!
//! 每个学校的有效配速区间不同（策略响应里 `runRuleModel.speedTop` /
//! `speedBottom`），程序不该替学生假设一个。这里的职责只有两件：
//!
//! 1. 把学校规则换算成生成器能用的 m/s 区间；
//! 2. 判定"用户填的配速是否在学校允许范围内"，越界时给出可读的报错。
//!
//! ## 单位约定
//!
//! 学校规则用的是**分/公里**（`speedTop = 3.0` 表示 3′00″/km，即最快；
//! `speedBottom = 8.0` 表示 8′00″/km，即最慢）。分/公里越小越快，因此：
//!
//! - `speedTop`（最快）→ 速度上界 `ceil_mps`
//! - `speedBottom`（最慢）→ 速度下界 `floor_mps`
//!
//! 换算：`m/s = 1000 / (分钟/公里 × 60)`。
//!
//! 生成器内部一律用 m/s，避免"越快数字越小"这种反直觉比较散落在各处。

use super::generator::{SPEED_CEIL, SPEED_FLOOR};

/// 配速窗口（m/s）：`floor_mps ≤ v ≤ ceil_mps`。
///
/// 字段里存的是**学校规则的字面值**，用于校验用户输入（学生填边界值必须能通过）。
/// 生成轨迹时用 [`PaceWindow::gen_floor_mps`] / [`PaceWindow::gen_ceil_mps`]——
/// 那两个向内收了一点，避免贴着判定阈值生成。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaceWindow {
    /// 最慢允许速度（m/s），学校规则 `speedBottom` 的字面换算。
    pub floor_mps: f64,
    /// 最快允许速度（m/s），学校规则 `speedTop` 的字面换算。
    pub ceil_mps: f64,
    /// 该窗口是否来自学校规则（`false` = 学校没声明，用的兜底值）。
    pub from_school: bool,
}

/// 生成时把窗口向内侧收一点，避免正好压在边界上。
///
/// 学校规则是**判定阈值**，不是目标值：贴着 8′00″/km 生成，任何噪声都会让
/// 某个 10 秒窗越界。按区间宽度收 2.5%，边界附近的记录就稳了。
///
/// 这只影响生成，不影响校验——见 [`PaceWindow::check_sec_km`]。
const MARGIN_FRAC: f64 = 0.025;

impl PaceWindow {
    /// 学校未声明配速规则时的兜底窗口。
    ///
    /// 只保证"是人类跑步"，**不保证符合任何具体学校**——所以 `from_school`
    /// 为 `false`，调用方应当提示用户这是兜底值。
    pub const FALLBACK: PaceWindow = PaceWindow {
        floor_mps: SPEED_FLOOR,
        ceil_mps: SPEED_CEIL,
        from_school: false,
    };

    /// 由学校规则构造。两个参数单位都是**分/公里**。
    ///
    /// 任一值非正或两个值大小关系颠倒（最快比最慢还慢）时返回 `Err`。
    /// 存的是字面值，不收边；收边只发生在生成侧。
    pub fn from_school(speed_top_min_km: f64, speed_bottom_min_km: f64) -> Result<Self, String> {
        if !speed_top_min_km.is_finite() || !speed_bottom_min_km.is_finite() {
            return Err("学校配速规则包含非有限数值".into());
        }
        if speed_top_min_km <= 0.0 || speed_bottom_min_km <= 0.0 {
            return Err("学校配速规则包含非正数值".into());
        }
        let ceil_mps = 1000.0 / (speed_top_min_km * 60.0);
        let floor_mps = 1000.0 / (speed_bottom_min_km * 60.0);
        if floor_mps >= ceil_mps {
            return Err(format!(
                "学校配速规则自相矛盾：最快 {}′/km 不快于最慢 {}′/km",
                speed_bottom_min_km, speed_top_min_km
            ));
        }
        Ok(PaceWindow {
            floor_mps,
            ceil_mps,
            from_school: true,
        })
    }

    /// 由显式 m/s 区间构造（人工指定 / 测试用）。
    pub fn from_mps(floor_mps: f64, ceil_mps: f64) -> Result<Self, String> {
        if !(floor_mps.is_finite() && ceil_mps.is_finite()) {
            return Err("配速区间包含非有限数值".into());
        }
        if floor_mps <= 0.0 || floor_mps >= ceil_mps {
            return Err(format!(
                "配速区间无效：下界 {floor_mps:.3} m/s 必须为正且小于上界 {ceil_mps:.3} m/s"
            ));
        }
        Ok(PaceWindow {
            floor_mps,
            ceil_mps,
            from_school: false,
        })
    }

    /// 生成用的下界（向内收）。
    pub fn gen_floor_mps(&self) -> f64 {
        let span = self.ceil_mps - self.floor_mps;
        self.floor_mps + (span * MARGIN_FRAC).min(span * 0.25)
    }

    /// 生成用的上界（向内收）。
    pub fn gen_ceil_mps(&self) -> f64 {
        let span = self.ceil_mps - self.floor_mps;
        self.ceil_mps - (span * MARGIN_FRAC).min(span * 0.25)
    }

    /// 学校规则的原始区间（分/公里）：`(最快, 最慢)`。
    pub fn min_km(&self) -> (f64, f64) {
        (
            1000.0 / (self.ceil_mps * 60.0),
            1000.0 / (self.floor_mps * 60.0),
        )
    }

    /// 学校规则的原始区间（秒/公里）：`(最快, 最慢)`。
    pub fn sec_km(&self) -> (f64, f64) {
        let (fast, slow) = self.min_km();
        (fast * 60.0, slow * 60.0)
    }

    pub fn contains_mps(&self, v: f64) -> bool {
        v >= self.floor_mps && v <= self.ceil_mps
    }

    /// 把「距离 + 时长」换算成平均速度并判定是否在窗口内。
    pub fn contains(&self, distance_m: f64, dur_s: i64) -> bool {
        if distance_m <= 0.0 || dur_s <= 0 {
            return false;
        }
        self.contains_mps(distance_m / dur_s as f64)
    }

    /// 校验用户填写的配速（**秒/公里**），越界时返回可直接展示给学生的报错。
    ///
    /// 允许 `s_km` 落在 `[最快, 最慢]` 之间（含端点）：端点本身是合法配速，
    /// 生成器内部还会按 `MARGIN_FRAC` 收紧，所以无需在此提前拒绝。
    pub fn check_sec_km(&self, s_km: f64) -> Result<(), String> {
        if !s_km.is_finite() || s_km <= 0.0 {
            return Err(format!("配速必须为正数（收到 {s_km} 秒/公里）"));
        }
        let (fast, slow) = self.sec_km();
        if s_km < fast - 0.5 || s_km > slow + 0.5 {
            return Err(format!(
                "配速 {} 超出学校规定范围 {}–{}，请改为范围内的值",
                fmt_sec_km(s_km),
                fmt_sec_km(fast),
                fmt_sec_km(slow)
            ));
        }
        Ok(())
    }

    /// 校验用户填写的配速区间（**秒/公里**），任一端越界即报错。
    pub fn check_sec_km_range(&self, min_s_km: f64, max_s_km: f64) -> Result<(), String> {
        let (lo, hi) = if min_s_km <= max_s_km {
            (min_s_km, max_s_km)
        } else {
            (max_s_km, min_s_km)
        };
        self.check_sec_km(lo)?;
        self.check_sec_km(hi)?;
        Ok(())
    }

    /// 把任意目标平均速度夹进**生成**窗口（自动模式用：用户没填配速时按学校范围收敛）。
    ///
    /// 用收过边的 `gen_*` 而不是字面边界：夹到边界上会立刻贴住判定阈值。
    pub fn clamp_mps(&self, v: f64) -> f64 {
        v.clamp(self.gen_floor_mps(), self.gen_ceil_mps())
    }

    /// 人可读描述，用于日志与界面提示。
    pub fn describe(&self) -> String {
        let (fast, slow) = self.sec_km();
        format!("{}–{}", fmt_sec_km(fast), fmt_sec_km(slow))
    }
}

/// 秒/公里 → `m′ss″/km`。
pub fn fmt_sec_km(s_km: f64) -> String {
    if !s_km.is_finite() || s_km <= 0.0 {
        return "-".into();
    }
    let total = s_km.round() as i64;
    format!("{}′{:02}″/km", total / 60, total % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 学校规则的换算方向：`speedTop` 是最快（分/公里最小），存字面值、不收边。
    #[test]
    fn school_rule_maps_top_to_ceil_and_bottom_to_floor() {
        let w = PaceWindow::from_school(3.0, 8.0).unwrap();
        assert!(w.from_school);
        // 字面值：3′00″/km = 5.5556 m/s，8′00″/km = 2.0833 m/s。
        assert!((w.ceil_mps - 1000.0 / 180.0).abs() < 1e-9, "{}", w.ceil_mps);
        assert!(
            (w.floor_mps - 1000.0 / 480.0).abs() < 1e-9,
            "{}",
            w.floor_mps
        );
        assert!(w.floor_mps < w.ceil_mps);
        // 生成窗口收在里面，但仍是有效区间。
        assert!(w.gen_floor_mps() > w.floor_mps);
        assert!(w.gen_ceil_mps() < w.ceil_mps);
        assert!(w.gen_floor_mps() < w.gen_ceil_mps());
    }

    /// 学生填的正好是学校规则的端点时必须能通过——校验用字面值，不是收边值。
    #[test]
    fn school_boundary_values_are_accepted_by_validation() {
        let w = PaceWindow::from_school(3.0, 8.0).unwrap();
        assert!(w.check_sec_km(180.0).is_ok(), "3′00″/km 应合法");
        assert!(w.check_sec_km(480.0).is_ok(), "8′00″/km 应合法");
        assert!(w.check_sec_km(450.0).is_ok(), "7′30″/km 在范围内");
    }

    /// 越界要报错，且报错里带学校范围，学生照着改。
    #[test]
    fn user_pace_outside_school_window_is_rejected_with_range() {
        let w = PaceWindow::from_school(3.0, 8.0).unwrap();
        let too_fast = w.check_sec_km(150.0).unwrap_err();
        assert!(too_fast.contains("超出学校规定范围"), "{too_fast}");
        let too_slow = w.check_sec_km(600.0).unwrap_err();
        assert!(too_slow.contains("超出学校规定范围"), "{too_slow}");
        // 报错里要能看出学校范围。
        assert!(too_slow.contains("3′00″/km"), "{too_slow}");
        assert!(too_slow.contains("8′00″/km"), "{too_slow}");
    }

    /// 区间里任一端越界都要拦下。
    #[test]
    fn range_check_rejects_either_end_outside() {
        let w = PaceWindow::from_school(3.0, 8.0).unwrap();
        assert!(w.check_sec_km_range(250.0, 400.0).is_ok());
        assert!(w.check_sec_km_range(150.0, 400.0).is_err(), "下限过快");
        assert!(w.check_sec_km_range(400.0, 600.0).is_err(), "上限过慢");
        // 顺序颠倒也要能正确判定
        assert!(w.check_sec_km_range(400.0, 250.0).is_ok());
    }

    /// 自相矛盾的学校规则要拒绝，而不是静默产出一个空窗口。
    #[test]
    fn contradictory_school_rule_is_rejected() {
        assert!(PaceWindow::from_school(8.0, 3.0).is_err(), "最快比最慢还慢");
        assert!(PaceWindow::from_school(0.0, 8.0).is_err());
        assert!(PaceWindow::from_school(f64::NAN, 8.0).is_err());
    }

    /// 收边只发生在生成侧，且永远收在学校范围之内。
    #[test]
    fn margin_stays_inside_the_school_range() {
        for (top, bottom) in [(3.0, 8.0), (4.0, 7.0), (2.5, 10.0), (5.0, 9.0)] {
            let w = PaceWindow::from_school(top, bottom).unwrap();
            assert!(w.gen_floor_mps() >= w.floor_mps, "生成下界不该越出学校范围");
            assert!(w.gen_ceil_mps() <= w.ceil_mps, "生成上界不该越出学校范围");
            assert!(w.gen_floor_mps() < w.gen_ceil_mps(), "生成区间不能为空");
        }
    }
}
