//! 跑步策略：POST /api/v70103/runModePolicy。
//! 返回 data.runRuleModel.minDistance（提交时 selDistance 用它）与 data.policy。

use super::client::{get_field, ApiClient};
use super::fence::point_xy;
use serde_json::{json, Value};

pub const POLICY_PATH: &str = "/api/v70103/runModePolicy";

pub struct PolicyInfo {
    pub timestamp: i64,
    pub policy: i64,
    pub min_distance: i64,
    pub valid_time: i64,
    /// 必经点（BD 系，与打卡点同系），policy 响应里若有则返回。
    #[allow(dead_code)] // 自定义路线删除后暂无读者；保留服务端必经点解析结果
    pub must_points: Vec<(f64, f64)>,
    /// 服务端下发的跑步冻结剩余时长（**秒**，0 = 未冻结）。
    ///
    /// 对应 `RandomConfigBean.freezeRunTime`。真实客户端在
    /// `RunCreditFragment.onPolicySuccess` 里判定 `if (freezeRunTime > 0)`，
    /// 命中就切到 `RunStartButtonState.FROZEN`、菜单显示「🚫 跑步冻结」并挂
    /// `CountDownTimer(1000 * freezeRunTime)` 倒计时，**开始按钮被禁用**。
    ///
    /// 被封禁的账号在真实 App 里根本点不到「开始跑步」，也就不会有任何提交。
    /// 我们的程序直接发包，必须自己守住这条门，否则服务端看到「封禁期内仍有
    /// 跑步提交」就是明确的违规信号。
    pub freeze_run_time: i64,
    /// 服务端随策略下发的提示文案（`RandomConfigBean.message`）。
    pub message: String,
    /// 点位顺序规则：0 = 随机点位模式（不强制顺序），非 0 = 顺序跑。
    ///
    /// 对应 `RunRuleModel.sequenceRule`。官方完成条件：
    ///   - 未开启顺序跑：经过必经点位 + 至少 2 个普通点位 + 达到目标里程
    ///   - 已开启顺序跑：按点位顺序跑完显示的所有点位 + 达到目标里程
    ///
    /// 界面上「有没有点位序号」是这条规则的直接体现。
    pub sequence_rule: i64,
    /// 顺序强度（`RunRuleModel.sequenceStrength`）。
    pub sequence_strength: i64,
    /// 学校是否启用人脸抽检（`RunRuleModel.faceVerify`）。
    ///
    /// 这个值是**服务端自己下发的**，提交体的 `faceCheck` 必须与之一致：
    /// 学校没开人脸却上报 `faceCheck=1`，等于声称做过一次根本不存在的校验。
    pub face_verify: bool,
    /// 服务端可选的校园区域/绿色围栏元数据；没有配置时为自由跑默认值。
    pub area: crate::track::wire::RunAreaMeta,
}

impl PolicyInfo {
    /// 账号是否处于跑步冻结期；返回剩余秒数。
    ///
    /// 真实客户端拿到 `freezeRunTime > 0` 就把开始按钮切成 FROZEN、倒计时
    /// `freezeRunTime` 秒，用户在冻结期内**根本无法发起跑步**。我们直接发包，
    /// 必须自己拦：否则服务端会看到「封禁账号仍在提交跑步记录」。
    pub fn frozen_for_secs(&self) -> Option<i64> {
        (self.freeze_run_time > 0).then_some(self.freeze_run_time)
    }

    /// 点位顺序模式的文字描述，供日志直接判断。
    pub fn sequence_mode(&self) -> &'static str {
        if self.sequence_rule != 0 {
            "顺序跑（须按点位顺序跑完全部点位）"
        } else {
            "随机点位（不强制顺序）"
        }
    }

    /// 冻结期内拒绝提交时给出的完整提示。
    pub fn frozen_message(&self) -> String {
        let secs = self.freeze_run_time.max(0);
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        let mins = (secs % 3600) / 60;
        let left = if days > 0 {
            format!("{days} 天 {hours:02}:{mins:02}")
        } else {
            format!("{hours:02}:{mins:02}")
        };
        let tail = if self.message.trim().is_empty() {
            String::new()
        } else {
            format!("（服务端提示：{}）", self.message.trim())
        };
        format!(
            "账号处于跑步冻结期，剩余 {left}，冻结期内不能提交跑步{tail}。\
             真实客户端此时「开始跑步」按钮是禁用的，强行提交会被判违规。"
        )
    }
}
/// 将已解密的业务响应解析为稳定的策略模型，供网络路径和 fixture 测试共用。
pub(crate) fn parse_policy_response(biz: &Value) -> Result<PolicyInfo, String> {
    let timestamp = get_field(biz, "timestamp")
        .and_then(|t| t.as_i64())
        .ok_or("policy 响应缺 timestamp")?;
    let policy = get_field(biz, "policy")
        .and_then(|t| t.as_i64())
        .unwrap_or(0);
    let rule = get_field(biz, "runRuleModel")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(PolicyInfo {
        timestamp,
        policy,
        min_distance: rule
            .get("minDistance")
            .and_then(|t| t.as_i64())
            .unwrap_or(1000),
        valid_time: rule.get("validTime").and_then(|t| t.as_i64()).unwrap_or(0),
        freeze_run_time: get_field(biz, "freezeRunTime")
            .and_then(|t| t.as_i64())
            // 兜底：个别版本把它放在 runRuleModel 里
            .or_else(|| rule.get("freezeRunTime").and_then(|t| t.as_i64()))
            .unwrap_or(0)
            .max(0),
        message: get_field(biz, "message")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
            .to_string(),
        sequence_rule: rule
            .get("sequenceRule")
            .and_then(|t| t.as_i64())
            .unwrap_or(0),
        sequence_strength: rule
            .get("sequenceStrength")
            .and_then(|t| t.as_i64())
            .unwrap_or(0),
        face_verify: rule
            .get("faceVerify")
            .and_then(|t| t.as_bool())
            .unwrap_or(false),
        must_points: extract_must_points(biz),
        area: super::points::area_from_payload(biz, &[]),
    })
}

/// 从 policy 响应 `data` 中防御式提取必经点列表（字段名不确定，逐个尝试）。
fn extract_must_points(v: &serde_json::Value) -> Vec<(f64, f64)> {
    let data = v.get("data").unwrap_or(v);
    for name in [
        "pointList",
        "runPointList",
        "mustPointList",
        "passPointList",
        "mustPoints",
        "passPoints",
        "runPoints",
        "points",
        "nodeList",
        "checkPointList",
    ] {
        let Some(arr) = data.get(name).and_then(|x| x.as_array()) else {
            continue;
        };
        // 复用 fence 的坐标系解析：BD 优先，缺失/全 0 回退 glat/glon（GCJ→BD）并过滤 (0,0)。
        let pts: Vec<(f64, f64)> = arr.iter().filter_map(point_xy).collect();
        if !pts.is_empty() {
            return pts;
        }
    }
    Vec::new()
}

/// body：{"runMode":1,"ruleUpdateTime":0,"geoFenceUpdateTime":0,"selectUnid":<unid>,"operateType":0}
pub fn fetch_policy(client: &mut ApiClient) -> Result<PolicyInfo, String> {
    let unid = client
        .login
        .as_ref()
        .map(|s| s.unid.clone())
        .unwrap_or_default();
    let select_unid = unid.parse::<i64>().unwrap_or(0);
    let body = json!({
        "runMode": 1,
        "ruleUpdateTime": 0,
        "geoFenceUpdateTime": 0,
        "selectUnid": select_unid,
        "operateType": 0,
    })
    .to_string();
    let biz = client.call("POST", POLICY_PATH, &body, &[])?;
    parse_policy_response(&biz)
}

#[cfg(test)]
mod tests {
    use super::parse_policy_response;

    #[test]
    fn parses_nested_policy_fixture_and_gcj_fallback_point() {
        let raw = include_str!("fixtures/policy_nested.json");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let policy = parse_policy_response(&value).unwrap();
        assert_eq!(policy.timestamp, 1_700_000_000_123);
        assert_eq!(policy.policy, 1);
        assert_eq!(policy.min_distance, 2000);
        assert_eq!(policy.valid_time, 86400);
        assert_eq!(policy.must_points.len(), 2);
        assert!((policy.must_points[0].0 - 39.9001).abs() < 1e-9);
        assert!((policy.must_points[1].0 - 39.9062).abs() < 0.01);
    }
}

#[cfg(test)]
mod freeze_tests {
    use super::parse_policy_response;
    use serde_json::json;

    fn base() -> serde_json::Value {
        json!({
            "timestamp": 1792916403089i64,
            "policy": 1,
            "runRuleModel": { "minDistance": 2000, "validTime": 0 }
        })
    }

    /// 未冻结：`freezeRunTime` 缺失或为 0，不应拦。
    #[test]
    fn missing_or_zero_freeze_does_not_block() {
        let p = parse_policy_response(&base()).unwrap();
        assert_eq!(p.freeze_run_time, 0);
        assert_eq!(p.frozen_for_secs(), None);

        let mut v = base();
        v["freezeRunTime"] = json!(0);
        assert_eq!(parse_policy_response(&v).unwrap().frozen_for_secs(), None);

        // 负数按 0 处理，不能因为脏数据把所有人锁死
        let mut neg = base();
        neg["freezeRunTime"] = json!(-5);
        assert_eq!(parse_policy_response(&neg).unwrap().freeze_run_time, 0);
    }

    /// 冻结：真实客户端 `if (freezeRunTime > 0)` 才切 FROZEN，这里必须一致。
    #[test]
    fn positive_freeze_blocks_with_remaining_seconds() {
        let mut v = base();
        v["freezeRunTime"] = json!(86400 + 3600 * 2 + 60 * 5);
        let p = parse_policy_response(&v).unwrap();
        assert_eq!(p.frozen_for_secs(), Some(93900));
        let msg = p.frozen_message();
        assert!(msg.contains("1 天 02:05"), "提示应含剩余时长，实际：{msg}");
        assert!(msg.contains("不能提交跑步"));
    }

    #[test]
    fn freeze_message_includes_server_hint_when_present() {
        let mut v = base();
        v["freezeRunTime"] = json!(600);
        v["message"] = json!("数据异常，冻结 10 分钟");
        let p = parse_policy_response(&v).unwrap();
        let msg = p.frozen_message();
        assert!(msg.contains("00:10"), "应显示 10 分钟，实际：{msg}");
        assert!(msg.contains("数据异常，冻结 10 分钟"));
    }

    /// `freezeRunTime` 也兼容放在 runRuleModel 里的版本。
    /// faceVerify 决定提交体的 faceCheck，必须按服务端下发的值走。
    #[test]
    fn face_verify_is_read_from_run_rule_model() {
        let mut v = base();
        // 缺失按 false：不声称做过人脸校验
        assert!(!parse_policy_response(&v).unwrap().face_verify);

        v["runRuleModel"]["faceVerify"] = json!(true);
        assert!(parse_policy_response(&v).unwrap().face_verify);

        v["runRuleModel"]["faceVerify"] = json!(false);
        assert!(!parse_policy_response(&v).unwrap().face_verify);
    }

    #[test]
    fn sequence_rule_is_read_from_run_rule_model() {
        let mut v = base();
        // 默认缺失 -> 0（随机点位模式）
        assert_eq!(parse_policy_response(&v).unwrap().sequence_rule, 0);
        assert!(parse_policy_response(&v)
            .unwrap()
            .sequence_mode()
            .contains("随机点位"));

        v["runRuleModel"]["sequenceRule"] = json!(1);
        v["runRuleModel"]["sequenceStrength"] = json!(1);
        let p = parse_policy_response(&v).unwrap();
        assert_eq!(p.sequence_rule, 1);
        assert_eq!(p.sequence_strength, 1);
        assert!(p.sequence_mode().contains("顺序跑"));
    }

    #[test]
    fn freeze_read_from_run_rule_model_fallback() {
        let mut v = base();
        v["runRuleModel"]["freezeRunTime"] = json!(1800);
        assert_eq!(
            parse_policy_response(&v).unwrap().frozen_for_secs(),
            Some(1800)
        );
    }
}
