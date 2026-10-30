//! 跑步记录。
//! 列表：POST /api/v70230/runnings/records {}；单条详情：POST /api/v70320/runnings/get_one_record。
//!
//! ★ 列表接口只返回 18 个摘要字段，**没有判定信息**，不能用来做达标自检。
//! 判定字段（`complete` / `isCheat` / `reasonList`）全在详情接口里，
//! 且详情接口在现有实测中一直是 `v70320`（不是旧代码里的 `v70260`）。

use super::client::{get_field, parse_data_field, ApiClient};
use serde_json::{json, Value};

pub const RECORDS_PATH: &str = "/api/v70230/runnings/records";
pub const GET_ONE_PATH: &str = "/api/v70320/runnings/get_one_record";
/// 旧版本号；保留给「学校服务端还没升级」的降级重试。
pub const GET_ONE_PATH_LEGACY: &str = "/api/v70260/runnings/get_one_record";

/// 记录列表行（表格展示用）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecordRow {
    pub rrid: i64,
    pub total_dis: f64,
    pub total_time: i64,
    pub start_time: i64,
    pub complete: bool,
    pub avg_step_freq: i64,
    #[serde(default)]
    pub calorie: i64,
    #[serde(default)]
    pub avg_power: i64,
    #[serde(default)]
    pub total_steps: i64,
    #[serde(default)]
    pub uuid: String,
}

/// 拉取记录列表并归一化为行。
pub fn fetch_records(client: &mut ApiClient) -> Result<Vec<RecordRow>, String> {
    let biz = client.call("POST", RECORDS_PATH, "{}", &[])?;
    Ok(rows_from(records_array(&biz)))
}

fn records_array(biz: &Value) -> Vec<Value> {
    match parse_data_field(biz) {
        Value::Array(a) => a,
        Value::Object(ref o) => o
            .get("list")
            .or_else(|| o.get("records"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
        _ => get_field(biz, "list")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
    }
}

fn rows_from(arr: Vec<Value>) -> Vec<RecordRow> {
    arr.iter()
        .map(|r| RecordRow {
            rrid: r.get("rrid").and_then(|v| v.as_i64()).unwrap_or(0),
            total_dis: r.get("totalDis").and_then(|v| v.as_f64()).unwrap_or(0.0),
            total_time: r.get("totalTime").and_then(|v| v.as_i64()).unwrap_or(0),
            start_time: r.get("startTime").and_then(|v| v.as_i64()).unwrap_or(0),
            complete: r.get("complete").and_then(|v| v.as_bool()).unwrap_or(false),
            avg_step_freq: r.get("avgStepFreq").and_then(|v| v.as_i64()).unwrap_or(0),
            calorie: r.get("calorie").and_then(|v| v.as_i64()).unwrap_or(0),
            avg_power: r.get("avgPower").and_then(|v| v.as_i64()).unwrap_or(0),
            total_steps: r.get("totalSteps").and_then(|v| v.as_i64()).unwrap_or(0),
            uuid: r.get("uuid").and_then(|v| v.as_str()).unwrap_or("").into(),
        })
        .collect()
}

/// 单条详情（验证提交结果用）：返回内层 data 对象。
///
/// 首选 `v70320`（实测该路径才带判定字段）。只有**明确表示路径/版本不可用**
/// 的错误才回退到旧的 `v70260`；「记录不存在」「token 过期」这类业务错误一律
/// 直接返回 —— 盲目重试只会把一次明确失败变成两次请求加一个含混错误。
pub fn fetch_one_record(client: &mut ApiClient, rrid: i64) -> Result<Value, String> {
    let body = json!({ "rrid": rrid, "uuid": Value::Null, "calculateBadge": false }).to_string();
    let biz = match client.call("POST", GET_ONE_PATH, &body, &[]) {
        Ok(b) => b,
        Err(first) if path_unavailable(&first) => {
            match client.call("POST", GET_ONE_PATH_LEGACY, &body, &[]) {
                Ok(b) => b,
                Err(_) => return Err(first),
            }
        }
        Err(e) => return Err(e),
    };
    // business.data 可能是 JSON 字符串或对象，其内层再取 data
    let inner = parse_data_field(&biz);
    let record = if inner.get("data").is_some() {
        parse_data_field(&inner)
    } else {
        inner
    };
    if record.is_null() {
        return Err("记录详情为空".into());
    }
    Ok(record)
}

/// 错误是否表示「这个接口路径/版本在本服务端不存在」。
///
/// 只认 404 与明确的"接口不存在"型提示；业务错误（记录不存在、token 过期、
/// 参数非法）都不算 —— 那些重试旧路径也不会变好。
fn path_unavailable(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("http 404")
        || e.contains("http 405")
        || e.contains("not found")
        || e.contains("no mapping")
        || e.contains("该接口不存在")
        || e.contains("接口已下线")
}

/// 单条跑步详情（get_one_record 返回的完整 data）。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub rrid: i64,
    pub uuid: String,
    pub sport_type: i64,
    pub total_time: i64,
    pub total_dis: f64,
    pub speed: f64,
    pub calorie: i64,
    pub avg_power: i64,
    pub total_steps: i64,
    pub total_ascent: i64,
    pub total_descent: i64,
    pub avg_step_freq: i64,
    pub valid_dis: f64,
    pub valid_time: i64,
    pub address: String,
    pub status_info: String,
    pub complete: bool,
    /// 异常标记：0 = 正常，非 0 = 「锻炼过程存在异常」。
    ///
    /// 与 `complete` 是**两个独立维度**：`complete=true, isCheat!=0` 的文案是
    /// 「数据达标，锻炼过程存在异常」。旧实现只读 `complete`，于是异常记录被
    /// 显示成完全正常。
    pub is_cheat: i64,
    pub reason_list: Vec<ReasonItem>,
    pub start_time: i64,
}

impl RunDetail {
    /// 达标结论（`complete` + `isCheat`，不看 `status`）。
    pub fn verdict(&self) -> crate::track::rules::Verdict {
        crate::track::rules::Verdict::from_fields(self.complete, self.is_cheat)
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonItem {
    pub reason: String,
    pub complete: bool,
    /// 服务端的规则类型码（`type`）。1 距离 / 7 时长 / 9 打卡点顺序 /
    /// 11 步频 / 12 配速 …；未知码原样保留，别猜。
    #[serde(default)]
    pub rule_type: i64,
}

impl ReasonItem {
    /// 规则类别的可读名（未知码返回 `None`）。
    pub fn category(&self) -> Option<&'static str> {
        crate::track::rules::reason_label(self.rule_type)
    }
}

/// 拉取单条详情并解析为结构体。
pub fn fetch_detail(client: &mut ApiClient, rrid: i64) -> Result<RunDetail, String> {
    let raw = fetch_one_record(client, rrid)?;
    let reasons: Vec<ReasonItem> = raw
        .get("reasonList")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|r| ReasonItem {
                    reason: r
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .into(),
                    complete: r.get("complete").and_then(|v| v.as_bool()).unwrap_or(false),
                    rule_type: r.get("type").and_then(|v| v.as_i64()).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(RunDetail {
        rrid: raw.get("rrid").and_then(|v| v.as_i64()).unwrap_or(rrid),
        uuid: raw
            .get("uuid")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        sport_type: raw.get("sportType").and_then(|v| v.as_i64()).unwrap_or(0),
        total_time: raw.get("totalTime").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64,
        total_dis: raw.get("totalDis").and_then(|v| v.as_f64()).unwrap_or(0.0),
        speed: raw.get("speed").and_then(|v| v.as_f64()).unwrap_or(0.0),
        calorie: raw.get("calorie").and_then(|v| v.as_i64()).unwrap_or(0),
        avg_power: raw.get("avgPower").and_then(|v| v.as_i64()).unwrap_or(0),
        total_steps: raw.get("totalSteps").and_then(|v| v.as_i64()).unwrap_or(0),
        total_ascent: raw.get("totalAscent").and_then(|v| v.as_i64()).unwrap_or(0),
        total_descent: raw
            .get("totalDescent")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        avg_step_freq: raw.get("avgStepFreq").and_then(|v| v.as_i64()).unwrap_or(0),
        valid_dis: raw.get("validDis").and_then(|v| v.as_f64()).unwrap_or(0.0),
        valid_time: raw.get("validTime").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64,
        address: raw
            .get("address")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        status_info: raw
            .get("statusInfo")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .into(),
        complete: raw
            .get("complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        // 大小写不敏感：不同接口字段大小写并不完全统一。
        is_cheat: raw
            .get("isCheat")
            .or_else(|| raw.get("ischeat"))
            .or_else(|| raw.get("IsCheat"))
            .and_then(|v| v.as_i64().or_else(|| v.as_bool().map(i64::from)))
            .unwrap_or(0),
        reason_list: reasons,
        start_time: raw.get("startTime").and_then(|v| v.as_i64()).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detail_serializes_camel_case() {
        let d = RunDetail {
            rrid: 42,
            total_dis: 1234.0,
            valid_dis: 1200.0,
            status_info: "有效".into(),
            reason_list: vec![ReasonItem {
                reason: "里程达标".into(),
                complete: true,
                rule_type: 1,
            }],
            start_time: 1_700_000_000_000,
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::to_value(&d).unwrap();
        for key in [
            "totalDis",
            "validDis",
            "statusInfo",
            "reasonList",
            "startTime",
            "sportType",
            "avgStepFreq",
        ] {
            assert!(v.get(key).is_some(), "缺少键 {key}");
        }
        assert!(v.get("total_dis").is_none());
    }

    #[test]
    fn parses_wrapped_string_records_fixture() {
        let raw = include_str!("fixtures/records_wrapped_string.json");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let rows = rows_from(records_array(&value));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rrid, 42);
        assert_eq!(rows[0].total_dis, 2500.0);
        assert_eq!(rows[0].total_time, 1200);
        assert!(rows[0].complete);
        assert_eq!(rows[0].avg_step_freq, 165);
        assert_eq!(rows[0].uuid, "fixture-uuid");
    }

    /// 降级判据只认"路径不存在"，业务错误必须原样抛出。
    #[test]
    fn only_path_errors_trigger_the_legacy_fallback() {
        assert!(path_unavailable("HTTP 404: {\"error\":404}"));
        assert!(path_unavailable("业务错误 1404: No mapping for POST"));
        assert!(path_unavailable("接口已下线"));
        // 业务错误不该重试旧路径
        assert!(!path_unavailable("业务错误 10603: 操作过于频繁"));
        assert!(!path_unavailable("业务错误 401: token 已过期"));
        assert!(!path_unavailable("记录不存在"));
        assert!(!path_unavailable("网络错误: connection reset"));
    }

    /// 判定结论必须由 `complete` + `isCheat` 联合得出，且原因码要能映射。
    #[test]
    fn detail_verdict_combines_complete_and_is_cheat() {
        let mut raw = serde_json::json!({
            "rrid": 7,
            "complete": true,
            "isCheat": 0,
            "reasonList": [
                {"type": 1, "reason": "最少要跑2.00公里", "complete": true},
                {"type": 9, "reason": "按顺序通过所有点位", "complete": true}
            ]
        });
        // 直接走解析路径，确保字段名（isCheat）与序号都对上
        let reasons: Vec<ReasonItem> = raw["reasonList"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| ReasonItem {
                reason: r["reason"].as_str().unwrap_or("").into(),
                complete: r["complete"].as_bool().unwrap_or(false),
                rule_type: r["type"].as_i64().unwrap_or(0),
            })
            .collect();
        assert_eq!(reasons[0].category(), Some("距离"));
        assert_eq!(reasons[1].category(), Some("打卡点顺序"));

        let d = RunDetail {
            complete: true,
            is_cheat: 0,
            reason_list: reasons,
            ..Default::default()
        };
        assert_eq!(d.verdict(), crate::track::rules::Verdict::Pass);

        // 数据达标但过程异常，不能被显示成完全正常
        raw["isCheat"] = serde_json::json!(2);
        let d2 = RunDetail {
            complete: true,
            is_cheat: 2,
            ..Default::default()
        };
        assert_eq!(d2.verdict(), crate::track::rules::Verdict::PassWithAnomaly);
    }
}
