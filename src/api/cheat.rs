//! 违规通报名单：POST {DISCOVERY}/api/v78/cheat/cheatlist。
//! self=null 自己干净；非 null 已被标记（含原因字段）。

use super::client::ApiClient;
use crate::api::model::DISCOVERY;
use serde_json::{json, Value};

pub const CHEATLIST_PATH: &str = "/api/v78/cheat/cheatlist";

pub struct CheatReport {
    /// null=干净。
    pub self_info: Value,
    pub list: Vec<Value>,
}

impl CheatReport {
    pub fn is_clean(&self) -> bool {
        self.self_info.is_null()
    }

    pub fn self_brief(&self) -> String {
        if self.self_info.is_null() {
            return "干净".into();
        }
        let v = &self.self_info;
        let reason = v
            .get("reason")
            .or_else(|| v.get("punishReason"))
            .or_else(|| v.get("cause"))
            .and_then(|r| r.as_str())
            .unwrap_or("");
        let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
        format!("{} {}", name, reason).trim().to_string()
    }
}

pub fn query(
    client: &mut ApiClient,
    page: i64,
    log: &mut dyn FnMut(&str),
) -> Result<CheatReport, String> {
    let unid: i64 = client
        .login
        .as_ref()
        .map(|s| s.unid.parse().unwrap_or(0))
        .unwrap_or(0);
    let body = json!({ "pageNum": page, "pageSize": 20, "unid": unid }).to_string();
    let biz = client.call_host(DISCOVERY, "POST", CHEATLIST_PATH, &body, &[])?;
    let data = super::client::parse_data_field(&biz);
    let list = data
        .get("list")
        .and_then(|l| l.as_array())
        .cloned()
        .unwrap_or_default();
    let self_info = data.get("self").cloned().unwrap_or(Value::Null);
    log(&format!(
        "[cheat] self={} 全校违规 {} 条",
        if self_info.is_null() {
            "null"
        } else {
            "非空"
        },
        list.len()
    ));
    Ok(CheatReport { self_info, list })
}

// ── 申诉状态 ────────────────────────────────────────────────────────────
//
// POST api/v8/runnings/appeal/info  -> AppealInfoBean{ appealStatus, verifier, verifyInfo, verifyTime }
//   appealStatus 语义（客户端 AppealInfoBean 只有这四个字段，没有枚举常量）：
//     未见提交状态值为 null —— 说明还没申诉过。
// POST api/v670/runnings/hardware/antCheating/details -> { config, list:[{ shootingTime, name, imgUrl }] }
//   设备取证：抓拍照片 + 说明。查询只读。

pub const APPEAL_INFO_PATH: &str = "/api/v8/runnings/appeal/info";
pub const CHEAT_DETAILS_PATH: &str = "/api/v670/runnings/hardware/antCheating/details";

/// 申诉状态。字段原样保留服务端值，不猜测未确定的枚举含义。
#[derive(Debug, Clone)]
pub struct AppealInfo {
    /// 服务端原值；`None` = 未申诉过。
    pub appeal_status: Option<i64>,
    /// 复核人（有名字说明走人工复核）。
    pub verifier: Option<String>,
    /// 复核意见。
    pub verify_info: Option<String>,
    /// 复核时间（毫秒时间戳）。
    pub verify_time: Option<i64>,
}

impl AppealInfo {
    /// 是否申诉过。
    pub fn submitted(&self) -> bool {
        self.appeal_status.is_some()
    }

    /// 一行摘要。
    pub fn brief(&self) -> String {
        let Some(status) = self.appeal_status else {
            return "未提交申诉".into();
        };
        let who = self.verifier.as_deref().filter(|s| !s.trim().is_empty());
        let info = self.verify_info.as_deref().filter(|s| !s.trim().is_empty());
        let mut out = format!("appealStatus={status}");
        if let Some(v) = who {
            out.push_str(&format!(" 复核人={v}"));
        }
        match info {
            Some(i) => out.push_str(&format!(" 意见={i}")),
            None => out.push_str(" 意见=（空，可能仍在审核）"),
        }
        out
    }
}

/// 查询申诉状态。只读，不发任何写请求。
pub fn query_appeal(
    client: &mut ApiClient,
    rrid: &str,
    log: &mut dyn FnMut(&str),
) -> Result<AppealInfo, String> {
    let body = json!({ "rrid": rrid }).to_string();
    let biz = client.call_host(DISCOVERY, "POST", APPEAL_INFO_PATH, &body, &[])?;
    let data = super::client::parse_data_field(&biz);
    let info = AppealInfo {
        appeal_status: data.get("appealStatus").and_then(|v| v.as_i64()),
        verifier: data
            .get("verifier")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        verify_info: data
            .get("verifyInfo")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        verify_time: data.get("verifyTime").and_then(|v| v.as_i64()),
    };
    log(&format!("[appeal] rrid={rrid} {}", info.brief()));
    Ok(info)
}

/// 查询一次跑步的设备取证详情（抓拍照片清单）。只读。
pub fn query_cheat_details(
    client: &mut ApiClient,
    rrid: &str,
    log: &mut dyn FnMut(&str),
) -> Result<Value, String> {
    let body = json!({ "rrid": rrid }).to_string();
    let biz = client.call_host(DISCOVERY, "POST", CHEAT_DETAILS_PATH, &body, &[])?;
    let data = super::client::parse_data_field(&biz);
    let n = data
        .get("list")
        .and_then(|l| l.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    log(&format!("[cheat-details] rrid={rrid} 取证照片 {n} 条"));
    Ok(data)
}
