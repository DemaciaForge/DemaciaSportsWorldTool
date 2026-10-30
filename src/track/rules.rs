//! 服务端跑步判定规则（只读推断，不参与提交）。
//!
//! 来源：对端「运动世界控制台」v2.7.0 安装包的只读逆向，
//! `isvalidpoint` 模块自述其逐指令还原自官方 `libswsport.so @ 0x10e84`
//! （AArch64 JNI 静态注册），并经 29,232 组 Unicorn 差分验证。本文件只搬
//! 该可验证的**评分表**与**判定式**，实测证据见各常量注释。
//!
//! 用途：在提交前按服务端同一套口径预演「这条轨迹会不会被判无效」，
//! 而不是等提交完再看 `reasonList`。
//!
//! 边界（务必读）：这是**静态复现**，不是抓包实证。服务端仍可能改表；
//! 本模块只回答「按这套已知规则是否通过」，不构成合规性保证。

/// 通过线：总分 **严格大于** 65（反汇编是 `cmp w8, #0x41` + `cset hi`）。
pub const PASS_THRESHOLD: i32 = 65;

/// 点位类型/序号档位分（`w3` → 分）。
///
/// 反汇编分支：`w3 < 3` → 0；`3..5` → 5；`5..8` → 15；`8..15` → 25；`>=15` → 30。
pub fn base_score(w3: i32) -> i32 {
    if w3 < 3 {
        0
    } else if w3 < 5 {
        5
    } else if w3 < 8 {
        15
    } else if w3 < 15 {
        25
    } else {
        30
    }
}

/// 点位种类分（`w2` → 分）：1 → 30；2 → 20；其余 → 15。
///
/// `w2` 就是 `type` 字段（真机 0 普通点 / 2 打卡点命中 / 5、6 起终点）。
pub fn point_type_score(w2: i32) -> i32 {
    match w2 {
        1 => 30,
        2 => 20,
        _ => 15,
    }
}

/// 与上一点间距分（`s1`，米）：`<10` → 30；`<50` → 20；`<100` → 10；其余 0。
///
/// ★ 边界实证：`10.0` 落在 20 分档、`50.0` 落在 10 分档、`100.0` 落在 0 分档
/// （反汇编 `b.pl` / `csel mi`，即 `>=` 才降档）。
pub fn distance_score(s1: f32) -> i32 {
    if s1 < 10.0 {
        30
    } else if s1 < 50.0 {
        20
    } else if s1 < 100.0 {
        10
    } else {
        0
    }
}

/// 瞬时速度分（`s0`，m/s）：`> 0.0` → 10，否则 0（反汇编 `csel gt`）。
pub fn speed_score(s0: f32) -> i32 {
    if s0 > 0.0 {
        10
    } else {
        0
    }
}

/// 四项求和（不含两个标志位）。
pub fn total_score(w2: i32, s0: f32, s1: f32, w3: i32) -> i32 {
    speed_score(s0) + base_score(w3) + point_type_score(w2) + distance_score(s1)
}

/// `isValidPoint` 的完整判定：
/// 总分 > 65 **且** `(w4 & 0xff) != 0` **且** `(w5 & 0xff) == 0`。
///
/// `w4` / `w5` 是原生侧的两个标志位参数；调用方按点位来源给出。默认
/// `(1, 0)` 即"正常点"（与本机复现的默认参数一致）。
pub fn is_valid_point(w2: i32, s0: f32, s1: f32, w3: i32, w4: i32, w5: i32) -> bool {
    let passed = total_score(w2, s0, s1, w3) > PASS_THRESHOLD;
    passed && (w4 & 0xff) != 0 && (w5 & 0xff) == 0
}

/// **保守判据**：在 `w3` 取最高档（30 分）时仍然不通过。
///
/// 为什么需要它：`w3`（"点位类型/序号分档"）的确切来源在原生调用点未恢复，
/// 只知道它按 3/5/8/15 分段给 0~30 分。拿它当"点序号"会让一条正常轨迹的
/// 前几个点凭空失分 —— 那是我们的映射错误，不是轨迹的问题。
///
/// 因此本地预演只用**与 w3 无关**的结论：若一个点在最高档位都过不了，
/// 那么无论服务端怎么填 `w3`，它都一定被判无效。这样既不会误报，
/// 又能抓住真正会踩异常标记的点（例如采样间距 > 100 m）。
pub fn is_valid_point_best_case(w2: i32, s0: f32, s1: f32) -> bool {
    is_valid_point(w2, s0, s1, 15, 1, 0)
}

/// 达标判定的原因码（详情接口 `reasonList[].type`）。
///
/// 1..13 的完整表；其中 1 / 7 / 9 / 11 / 12 五条在实测记录里直接出现过。
/// 未知码返回 `None`，由调用方原样展示，不做猜测。
pub fn reason_label(code: i64) -> Option<&'static str> {
    Some(match code {
        1 => "距离",
        2 => "开始时间",
        3 => "结束时间",
        4 => "配速",
        5 => "步频",
        6 => "时长",
        7 => "时长",
        8 => "打卡点",
        9 => "打卡点顺序",
        10 => "步数",
        11 => "步频",
        12 => "配速",
        13 => "海拔",
        _ => return None,
    })
}

/// 一条记录的达标结论（`complete` / `isCheat` / `statusInfo` 三者联合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 达标且无异常。
    Pass,
    /// 数据达标，但过程被判异常。
    PassWithAnomaly,
    /// 不达标。
    Fail,
    /// 不达标且存在异常。
    FailWithAnomaly,
}

impl Verdict {
    /// 由详情接口三个字段得出。
    ///
    /// ★ `status` 字段**不是**达标标记：实测 8 条正常记录 `status` 全为 0，
    /// 而 `complete` 全为 true。判定只认 `complete` 与 `isCheat`。
    pub fn from_fields(complete: bool, is_cheat: i64) -> Self {
        match (complete, is_cheat != 0) {
            (true, false) => Verdict::Pass,
            (true, true) => Verdict::PassWithAnomaly,
            (false, false) => Verdict::Fail,
            (false, true) => Verdict::FailWithAnomaly,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "达标",
            Verdict::PassWithAnomaly => "数据达标，锻炼过程存在异常",
            Verdict::Fail => "不达标",
            Verdict::FailWithAnomaly => "不达标，存在异常",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_score_buckets_match_the_score_table() {
        assert_eq!(base_score(0), 0);
        assert_eq!(base_score(2), 0);
        assert_eq!(base_score(3), 5);
        assert_eq!(base_score(4), 5);
        assert_eq!(base_score(5), 15);
        assert_eq!(base_score(7), 15);
        assert_eq!(base_score(8), 25);
        assert_eq!(base_score(14), 25);
        assert_eq!(base_score(15), 30);
        assert_eq!(base_score(20), 30);
    }

    #[test]
    fn distance_score_boundaries_are_inclusive_lower_edges() {
        // 10.0 已经不算"<10"，落到 20 分档（反汇编 b.pl：>= 才继续往下判）
        assert_eq!(distance_score(9.99), 30);
        assert_eq!(distance_score(10.0), 20);
        assert_eq!(distance_score(49.99), 20);
        assert_eq!(distance_score(50.0), 10);
        assert_eq!(distance_score(99.9), 10);
        assert_eq!(distance_score(100.0), 0);
        assert_eq!(distance_score(5000.0), 0);
    }

    #[test]
    fn point_type_and_speed_scores() {
        assert_eq!(point_type_score(1), 30);
        assert_eq!(point_type_score(2), 20);
        assert_eq!(point_type_score(0), 15);
        assert_eq!(point_type_score(6), 15);
        assert_eq!(speed_score(0.0), 0);
        assert_eq!(speed_score(0.001), 10);
    }

    #[test]
    fn threshold_is_strictly_greater_than_65() {
        // type=2(20) + 速度(10) + 序号档(30, w3=20) + 间距<10(30) = 90 通过
        assert_eq!(total_score(2, 3.0, 9.9, 20), 90);
        assert!(is_valid_point(2, 3.0, 9.9, 20, 1, 0));
        // 100 m 以上的间距拿 0 分：20 + 10 + 30 + 0 = 60 不通过
        assert_eq!(total_score(2, 3.0, 120.0, 20), 60);
        assert!(!is_valid_point(2, 3.0, 120.0, 20, 1, 0));
        // 恰好 65：20 + 10 + 5 + 30 = 65 -> 不通过（严格大于）
        assert_eq!(total_score(2, 1.0, 9.0, 4), 65);
        assert!(!is_valid_point(2, 1.0, 9.0, 4, 1, 0));
        // 66：间距压进 20 分档 -> 20 + 10 + 5 + 20 = 55，仍不通过
        assert_eq!(total_score(2, 1.0, 10.0, 4), 55);
        // 换成必经档位 w3=15 -> 20 + 10 + 30 + 20 = 80 通过
        assert!(is_valid_point(2, 1.0, 10.0, 15, 1, 0));
    }

    #[test]
    fn flag_words_gate_the_result() {
        let base = (2, 3.0, 9.9, 20);
        assert!(is_valid_point(base.0, base.1, base.2, base.3, 1, 0));
        // w4 & 0xff == 0 -> 失败
        assert!(!is_valid_point(base.0, base.1, base.2, base.3, 0, 0));
        assert!(!is_valid_point(base.0, base.1, base.2, base.3, 0x100, 0));
        // w5 & 0xff != 0 -> 失败
        assert!(!is_valid_point(base.0, base.1, base.2, base.3, 1, 1));
        assert!(!is_valid_point(base.0, base.1, base.2, base.3, 1, 0xff));
        // 高位不影响
        assert!(is_valid_point(base.0, base.1, base.2, base.3, 1, 0x100));
    }

    #[test]
    fn reason_codes_map_and_unknown_stay_unknown() {
        assert_eq!(reason_label(1), Some("距离"));
        assert_eq!(reason_label(7), Some("时长"));
        assert_eq!(reason_label(9), Some("打卡点顺序"));
        assert_eq!(reason_label(11), Some("步频"));
        assert_eq!(reason_label(12), Some("配速"));
        assert_eq!(reason_label(0), None);
        assert_eq!(reason_label(14), None);
    }

    #[test]
    fn verdict_uses_complete_and_is_cheat_only() {
        assert_eq!(Verdict::from_fields(true, 0), Verdict::Pass);
        assert_eq!(Verdict::from_fields(true, 3), Verdict::PassWithAnomaly);
        assert_eq!(Verdict::from_fields(false, 0), Verdict::Fail);
        assert_eq!(Verdict::from_fields(false, -1), Verdict::FailWithAnomaly);
    }
}
