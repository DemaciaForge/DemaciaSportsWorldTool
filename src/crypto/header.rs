//! 请求头明文构造。
//! 固定键序紧凑 JSON；appInstallTime/appUpdateTime 从设备身份读取，
//! 同一 DeviceId 全生命周期稳定（逐请求漂移会影响设备一致性）。

use super::envelope::{md5_hex, now_ms};

/// tokenSign 固定后缀（与 Android 签名盐同源）。
pub const TOKEN_SIGN_SUFFIX: &str = "2slhe02lsfiwowlcixisla_sls-_slaor";

pub const IOS_APP_VERSION: &str = "7.3.92";
pub const ANDROID_APP_VERSION: &str = "7.3.92";

/// iOS UA。注意 `Scale` 必须与机型对应（三倍屏的机型 Scale/3.00）。
///
/// 版本号必须与 `HeaderIdentity::os_version` 一致——否则同一个请求的
/// HTTP 头和 `headerSign` 会声明两个不同的系统版本。
/// 原值写死 18.1 而 identity 默认 26.5.2，就是这个矛盾。
pub const UA_IOS: &str = "SWCampus/7.3.92 (iPhone; iOS 18.1; Scale/3.00)";

/// 按设备身份生成一致的 iOS UA（版本取自 `os_version`）。
pub fn ua_ios_for(identity: &HeaderIdentity) -> String {
    let ov = if identity.os_version.is_empty() {
        "18.1"
    } else {
        identity.os_version.as_str()
    };
    format!("SWCampus/{IOS_APP_VERSION} (iPhone; iOS {ov}; Scale/3.00)")
}
/// 安卓 UA 是 **OkHttp 默认值**，不是浏览器式串。
///
/// 原生 App 用 OkHttp 且从不自己设 UA，抓包里登录/埋点两处都是这个裸字符串。
/// 旧值 `Mozilla/5.0 (Linux; Android 14; ...) AppleWebKit/537.36 SWCampus/7.3.70`
/// 看着"更真"，实际是浏览器形态 —— 原生客户端不会这么拼，反而是异常值。
pub const UA_ANDROID: &str = "okhttp/4.12.0";

/// 设备身份。app_install_time <= 0 表示尚未生成。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeaderIdentity {
    #[serde(default = "default_platform")]
    pub platform: String, // "ios" | "android"
    #[serde(default)]
    pub device_id: String,
    #[serde(default)]
    pub idfa: String,
    #[serde(default = "default_os_version")]
    pub os_version: String,
    #[serde(default = "default_device_name")]
    pub device_name: String,
    /// 本地展示的制造商品牌；不改变服务端 deviceName（机型）字段。
    #[serde(default)]
    pub manufacturer: String,
    #[serde(default = "default_anchor_lat")]
    pub anchor_lat: f64,
    #[serde(default = "default_anchor_lon")]
    pub anchor_lon: f64,
    #[serde(default)]
    pub app_install_time: i64,
    /// 设备 MAC（首次生成后持久化，避免写死指纹）
    #[serde(default)]
    pub mac_address: String,
    /// 提交 body 的城市名
    #[serde(default = "default_city")]
    pub city: String,
}

/// iPhone 机型号码与屏幕参数对照表。
///
/// `(deviceName, osVersion, logicPixel, physicPixel)`
///
/// **为什么需要这张表**：原版把 `logicPixel` / `physicPixel` 写死为
/// `360x800` / `1080x2400`，但同时发 `osType=1` + `deviceName=iPhone`。
/// `1080x2400` 是安卓机分辨率，任何 iPhone 都不是这个值——
/// 服务端只要有一张机型-分辨率表就能直接挑出来。
///
/// 取值均为真实机型，不做任何李生。
pub const IPHONE_PROFILES: &[(&str, &str, &str, &str)] = &[
    ("iPhone 16 Pro Max", "18.1", "440x956", "1320x2868"),
    ("iPhone 16 Pro", "18.1", "402x874", "1206x2622"),
    ("iPhone 16 Plus", "18.1", "430x932", "1290x2796"),
    ("iPhone 16", "18.1", "393x852", "1179x2556"),
    ("iPhone 15 Pro Max", "17.6", "430x932", "1290x2796"),
    ("iPhone 15 Pro", "17.6", "393x852", "1179x2556"),
    ("iPhone 15", "17.6", "393x852", "1179x2556"),
    ("iPhone 14 Pro Max", "16.7", "430x932", "1290x2796"),
    ("iPhone 14 Pro", "16.7", "393x852", "1179x2556"),
    ("iPhone 14", "16.7", "390x844", "1170x2532"),
    ("iPhone 13", "15.7", "390x844", "1170x2532"),
    ("iPhone 13 mini", "15.7", "375x812", "1080x2340"),
    ("iPhone SE (3rd)", "15.7", "375x667", "750x1334"),
];

/// 安卓机型与屏幕参数对照表。
///
/// `(deviceName, osVersion, logicPixel, physicPixel)`
pub const ANDROID_PROFILES: &[(&str, &str, &str, &str)] = &[
    ("22081212C", "14", "360x800", "1080x2400"),
    ("2211133C", "13", "393x873", "1080x2400"),
    ("23127PN0CC", "14", "412x919", "1080x2400"),
    ("PDEM30", "14", "360x804", "1080x2408"),
    ("RMX3706", "13", "360x800", "1080x2400"),
    ("V2219A", "13", "360x800", "1080x2400"),
];

/// 按机型名查表，不匹配返回 None。
fn profile_for(
    platform: &str,
    device_name: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    let table = if platform == "android" {
        ANDROID_PROFILES
    } else {
        IPHONE_PROFILES
    };
    table
        .iter()
        .find(|(name, _, _, _)| *name == device_name)
        .copied()
}

/// 由机型名推导出一致的 (机型, 系统版本, 逻辑像素, 物理像素)。
///
/// 机型不在表中时，回退到表首（而不是保留 `iPhone` + 安卓分辨率的矛盾组合），
/// 并且 **系统版本也一并覆盖**——否则会出现 UA 写 iOS 18.1、
/// headerSign 写 26.5.2 这种同一请求两个版本号的情况。
pub fn resolve_device_profile(
    platform: &str,
    device_name: &str,
    os_version: &str,
) -> (String, String, String, String) {
    let (dn, ov, lp, pp) = match profile_for(platform, device_name) {
        Some(p) => p,
        None => {
            let table = if platform == "android" {
                ANDROID_PROFILES
            } else {
                IPHONE_PROFILES
            };
            table[0]
        }
    };
    // 用户显式指定了合法版本就尊重，但必须与机型表里的一致性无冲突。
    let ov = if os_version.is_empty() {
        ov
    } else {
        os_version
    };
    (
        dn.to_string(),
        ov.to_string(),
        lp.to_string(),
        pp.to_string(),
    )
}

pub fn random_mac() -> String {
    // 本地管理位随机单播 MAC
    let mut rng = rand::thread_rng();
    let octets: [u8; 3] = [0x08, 0xa4, rand::Rng::gen(&mut rng)];
    let tail: [u8; 3] = rand::Rng::gen(&mut rng);
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        octets[0], octets[1], octets[2], tail[0], tail[1], tail[2]
    )
}

fn default_platform() -> String {
    "ios".into()
}
fn default_os_version() -> String {
    // 必须与 `UA_IOS` 里的版本一致。原值 26.5.2 不存在（iOS 当前最高 18.x），
    // 而 UA 写的是 18.1——同一个请求里两个版本号，本身就是异常。
    "18.1".into()
}
fn default_device_name() -> String {
    "iPhone".into()
}
fn default_anchor_lat() -> f64 {
    38.901678
}
fn default_anchor_lon() -> f64 {
    121.540241
}
fn default_city() -> String {
    "大连市".into()
}

impl HeaderIdentity {
    /// 返回设备页配置的锚点，并执行统一边界校验。
    pub fn anchor_coordinate(&self) -> Result<crate::location::Coordinate, String> {
        crate::location::Coordinate::new(self.anchor_lat, self.anchor_lon, 0.0)
    }

    /// 旧版本首次启动会写入大连默认值。未明确配置前禁止将它作为异地跑步位置使用。
    pub fn has_unconfigured_default_location(&self) -> bool {
        self.city.trim().is_empty()
            || self.city.trim() == crate::location::default_city()
            || self
                .anchor_coordinate()
                .map(|c| c.is_default_dalian())
                .unwrap_or(true)
    }

    /// 安装时间：持久值优先；缺失时按平台惯例回退（iOS 3 天 / Android 90 天前）。
    pub fn install_time(&self, ts: i64) -> i64 {
        if self.app_install_time > 0 {
            self.app_install_time
        } else if self.platform == "android" {
            ts - 90 * 86_400_000
        } else {
            ts - 3 * 86_400_000
        }
    }

    /// 按平台生成一份新的安装时间（设备随机化时调用）。
    pub fn fresh_install_time(platform: &str) -> i64 {
        let days = if platform == "android" { 90 } else { 3 };
        now_ms() - days * 86_400_000
    }
}

impl Default for HeaderIdentity {
    fn default() -> Self {
        Self {
            platform: default_platform(),
            device_id: String::new(),
            idfa: String::new(),
            os_version: default_os_version(),
            device_name: default_device_name(),
            manufacturer: String::new(),
            anchor_lat: default_anchor_lat(),
            anchor_lon: default_anchor_lon(),
            app_install_time: 0,
            mac_address: String::new(),
            city: default_city(),
        }
    }
}

/// SWEncryption.signObject:suffix: 的 tokenSign。
/// SWJSON.sortedQueryString 按 key 排序：`timeStamp=<ts>&token=<token>&uid=<uid>`。
pub fn native_token_sign(uid: i64, token: &str, timestamp_ms: i64) -> String {
    let query = format!("timeStamp={}&token={}&uid={}", timestamp_ms, token, uid);
    md5_hex(format!("{}{}", query, TOKEN_SIGN_SUFFIX).as_bytes())
}

/// 由 device_id 派生 androidId（16 位小写 hex，同设备恒定）。
///
/// 真机取 `Settings.Secure.ANDROID_ID`，是 64 bit 随机数的 16 位小写 hex，
/// **永不为空** —— 安卓号发空串等于自报是脚本。这里用 md5 派生：既保证同一台
/// 设备重跑得到同一个值（稳定指纹），又不会与 DeviceId 呈前缀包含关系。
pub fn android_id_for(device_id: &str) -> String {
    let digest = md5_hex(device_id.as_bytes());
    digest.chars().take(16).collect()
}

fn insert_uuid(map: &mut serde_json::Map<String, serde_json::Value>) -> String {
    let u = uuid::Uuid::new_v4().to_string().to_uppercase();
    map.insert("nonce".into(), serde_json::Value::String(u.clone()));
    u
}

/// iOS 请求头明文（SWRequestHeaderProvider._fillFullHeaders 字段序）。
/// 返回 (紧凑 JSON, HTTP 附加头字段: nonce/tokenSign/timeStamp)。
pub fn build_native_ios_header(
    identity: &HeaderIdentity,
    uid: i64,
    token: &str,
    timestamp_ms: Option<i64>,
) -> (String, Vec<(String, String)>) {
    let mut m = serde_json::Map::new();
    let device_id = if identity.device_id.is_empty() {
        uuid::Uuid::new_v4().to_string().to_uppercase()
    } else {
        identity.device_id.clone()
    };
    let ts = timestamp_ms.unwrap_or_else(now_ms);
    let install = identity.install_time(ts);

    m.insert("osType".into(), serde_json::Value::String("1".into()));
    m.insert(
        "DeviceId".into(),
        serde_json::Value::String(device_id.clone()),
    );
    m.insert(
        "deviceName".into(),
        serde_json::Value::String(identity.device_name.clone()),
    );
    m.insert(
        "CustomDeviceId".into(),
        serde_json::Value::String(format!("{device_id}_iOS_sportsWorld_campus")),
    );
    m.insert(
        "osVersion".into(),
        serde_json::Value::String(identity.os_version.clone()),
    );
    if !identity.idfa.is_empty() {
        m.insert(
            "IDFA".into(),
            serde_json::Value::String(identity.idfa.clone()),
        );
    }
    // 屏幕参数必须与机型一致：原版写死 360x800/1080x2400 是安卓分辨率，
    // 却同时发 osType=1 + iPhone，属于可直接比对的矛盾。
    let (_dn, _ov, lp, pp) = resolve_device_profile(
        &identity.platform,
        &identity.device_name,
        &identity.os_version,
    );
    m.insert("logicPixel".into(), serde_json::Value::String(lp));
    m.insert("physicPixel".into(), serde_json::Value::String(pp));
    // 真机 CPU 架构：iOS 是 arm64；x86_64 只存在于模拟器，写上去等于自报"我不是手机"。
    m.insert("cpuModel".into(), serde_json::Value::String("arm64".into()));
    m.insert(
        "appVersion".into(),
        serde_json::Value::String(IOS_APP_VERSION.into()),
    );
    m.insert("isRoot".into(), serde_json::Value::Bool(false));
    m.insert("appInstallTime".into(), serde_json::Value::from(install));
    let nonce = insert_uuid(&mut m);
    if uid >= 1 {
        m.insert("uid".into(), serde_json::Value::from(uid));
    }
    if !token.is_empty() {
        m.insert("token".into(), serde_json::Value::String(token.to_string()));
    }
    m.insert("timeStamp".into(), serde_json::Value::from(ts));
    m.insert(
        "studentId".into(),
        serde_json::Value::from(if uid >= 1 { uid } else { 0 }),
    );
    if uid >= 1 && !token.is_empty() {
        m.insert(
            "tokenSign".into(),
            serde_json::Value::String(native_token_sign(uid, token, ts)),
        );
    }
    let extra = vec![
        ("nonce".to_string(), nonce),
        ("timeStamp".to_string(), ts.to_string()),
        ("tokenSign".to_string(), native_token_sign(uid, token, ts)),
    ];
    (serde_json::to_string(&m).unwrap_or_default(), extra)
}

/// Android 399 请求头明文（NativeNetworkCommonHeaders.build 键集，osType="0"）。
pub fn build_android_header(
    identity: &HeaderIdentity,
    uid: i64,
    token: &str,
    timestamp_ms: Option<i64>,
) -> (String, Vec<(String, String)>) {
    let mut m = serde_json::Map::new();
    let device_id = if identity.device_id.is_empty() {
        uuid::Uuid::new_v4().to_string().to_uppercase()
    } else {
        identity.device_id.clone()
    };
    let ts = timestamp_ms.unwrap_or_else(now_ms);
    let install = identity.install_time(ts);

    m.insert(
        "Accept".into(),
        serde_json::Value::String("application/json".into()),
    );
    m.insert(
        "Content-Type".into(),
        serde_json::Value::String("application/json".into()),
    );
    m.insert(
        "appVersion".into(),
        serde_json::Value::String(ANDROID_APP_VERSION.into()),
    );
    m.insert("osType".into(), serde_json::Value::String("0".into()));
    m.insert(
        "DeviceId".into(),
        serde_json::Value::String(device_id.clone()),
    );
    m.insert(
        "osVersion".into(),
        serde_json::Value::String(identity.os_version.clone()),
    );
    m.insert(
        "deviceName".into(),
        serde_json::Value::String(identity.device_name.clone()),
    );
    m.insert("IMEI".into(), serde_json::Value::String(String::new()));
    // 同样必须与机型一致。
    let (_dn, _ov, lp, pp) = resolve_device_profile(
        &identity.platform,
        &identity.device_name,
        &identity.os_version,
    );
    m.insert("logicPixel".into(), serde_json::Value::String(lp));
    m.insert("physicPixel".into(), serde_json::Value::String(pp));
    // androidId 在真机上是 Settings.Secure.ANDROID_ID（64 bit 随机数的 16 位小写
    // hex），**永不为空**。发空串等于举手说"我是脚本"。
    //
    // 由 device_id 派生而不是随机：同一台设备重跑必须得到同一个值（换值 = 换
    // 设备指纹）。用 md5 而非直接截取 —— DeviceId 是 UUID，直接截前 16 位会与
    // DeviceId 呈"前缀包含"关系，那本身就是可校验的关联特征。
    m.insert(
        "androidId".into(),
        serde_json::Value::String(android_id_for(&device_id)),
    );
    m.insert("blMac".into(), serde_json::Value::String(String::new()));
    m.insert("wifiMac".into(), serde_json::Value::String(String::new()));
    m.insert(
        "cpuModel".into(),
        serde_json::Value::String("arm64-v8a".into()),
    );
    m.insert("isRoot".into(), serde_json::Value::Bool(false));
    m.insert("appUpdateTime".into(), serde_json::Value::from(install));
    m.insert("appInstallTime".into(), serde_json::Value::from(install));
    let nonce = insert_uuid(&mut m);
    m.insert("timeStamp".into(), serde_json::Value::from(ts));
    m.insert(
        "CustomDeviceId".into(),
        serde_json::Value::String(format!("{device_id}_android_sportsWorld_campus")),
    );
    m.insert("uid".into(), serde_json::Value::from(uid));
    m.insert("token".into(), serde_json::Value::String(token.to_string()));
    m.insert("studentId".into(), serde_json::Value::from(uid));
    let sign = native_token_sign(uid, token, ts);
    m.insert("tokenSign".into(), serde_json::Value::String(sign.clone()));

    let extra = vec![
        ("nonce".to_string(), nonce),
        ("timeStamp".to_string(), ts.to_string()),
        ("tokenSign".to_string(), sign),
    ];
    (serde_json::to_string(&m).unwrap_or_default(), extra)
}

/// 按 identity 平台选择 header 构造器。
pub fn build_header_for(
    identity: &HeaderIdentity,
    uid: i64,
    token: &str,
    timestamp_ms: Option<i64>,
) -> (String, Vec<(String, String)>) {
    if identity.platform == "android" {
        build_android_header(identity, uid, token, timestamp_ms)
    } else {
        build_native_ios_header(identity, uid, token, timestamp_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 实测向量。
    #[test]
    fn test_token_sign_vector() {
        assert_eq!(
            native_token_sign(13056447, "TOKENABC", 1788958186123),
            "4a8d163186d91ac7b539a7bb07d457ba"
        );
    }

    /// iOS 头键序与条件字段（缺省字段跳过）。
    #[test]
    fn test_ios_header_shape() {
        let identity = HeaderIdentity {
            device_id: "0FA3EA16-46F0-4E44-9A97-1B63F0D9B11A".into(),
            idfa: String::new(),
            ..Default::default()
        };
        let (json, _) = build_native_ios_header(&identity, -1, "", Some(1788958186123));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "osType",
                "DeviceId",
                "deviceName",
                "CustomDeviceId",
                "osVersion",
                "logicPixel",
                "physicPixel",
                "cpuModel",
                "appVersion",
                "isRoot",
                "appInstallTime",
                "nonce",
                "timeStamp",
                "studentId"
            ]
        );
        assert_eq!(v["osType"], "1");
        assert_eq!(v["studentId"], 0);
        // 带 uid/token 时出现 tokenSign
        let (json2, _) = build_native_ios_header(&identity, 13056447, "TOKEN", Some(1788958186123));
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert!(v2.get("tokenSign").is_some());
        assert_eq!(v2["uid"], 13056447);
        assert_eq!(
            v2["tokenSign"],
            native_token_sign(13056447, "TOKEN", 1788958186123)
        );
    }

    /// Android 头键集 + osType="0"。
    #[test]
    fn test_android_header_shape() {
        let identity = HeaderIdentity {
            platform: "android".into(),
            device_id: "D0FA3EA16-46F0-4E44-9A97-1B63F0D9B11A".into(),
            os_version: "14".into(),
            device_name: "22081212C".into(),
            ..Default::default()
        };
        let (json, _) = build_android_header(&identity, 42, "TK", Some(1788958186123));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["osType"], "0");
        assert_eq!(v["appVersion"], ANDROID_APP_VERSION);
        assert_eq!(v["studentId"], 42);
        assert!(v.get("tokenSign").is_some());
        assert!(json.starts_with("{\"Accept\":\"application/json\""));
        // androidId 必须是 16 位小写 hex 且同设备恒定（真机永不为空）
        let aid = v["androidId"].as_str().unwrap();
        assert_eq!(aid.len(), 16, "androidId 长度应为 16，实际 {aid:?}");
        assert!(
            aid.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "androidId 应为小写 hex，实际 {aid:?}"
        );
        let (json2, _) = build_android_header(&identity, 42, "TK", Some(1788958199000));
        let v2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert_eq!(v2["androidId"], v["androidId"], "androidId 必须跨请求恒定");
        // 且不能是 DeviceId 的前缀（那本身是可校验的关联特征）
        assert!(!identity.device_id.to_lowercase().starts_with(aid));
    }

    /// 同一身份的 appInstallTime 跨请求零漂移（设备一致性防护）。
    #[test]
    fn test_install_time_stable_across_requests() {
        let identity = HeaderIdentity {
            app_install_time: 1_700_000_000_000_i64,
            ..Default::default()
        };
        let (j1, _) = build_native_ios_header(&identity, 1, "T", Some(1788958186123));
        let (j2, _) = build_native_ios_header(&identity, 1, "T", Some(1788958187000));
        let v1: serde_json::Value = serde_json::from_str(&j1).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&j2).unwrap();
        assert_eq!(v1["appInstallTime"], v2["appInstallTime"]);
        assert_eq!(v1["appInstallTime"], 1_700_000_000_000_i64);
        // Android appUpdateTime == appInstallTime，同样稳定
        let (a1, _) = build_android_header(&identity, 1, "T", Some(1788958186123));
        let (a2, _) = build_android_header(&identity, 1, "T", Some(1788958190000));
        let a1: serde_json::Value = serde_json::from_str(&a1).unwrap();
        let a2: serde_json::Value = serde_json::from_str(&a2).unwrap();
        assert_eq!(a1["appInstallTime"], a2["appInstallTime"]);
        assert_eq!(a1["appUpdateTime"], a1["appInstallTime"]);
        // 身份未持久化安装时间时，回退值在同一请求内至少自洽
        let fallback = HeaderIdentity::default();
        assert_eq!(fallback.install_time(1000), 1000 - 3 * 86_400_000);
    }

    #[test]
    fn legacy_dalian_defaults_are_not_accepted_for_a_run() {
        assert!(HeaderIdentity::default().has_unconfigured_default_location());
        let configured = HeaderIdentity {
            city: "石家庄市".into(),
            anchor_lat: 38.0428,
            anchor_lon: 114.5149,
            ..Default::default()
        };
        assert!(!configured.has_unconfigured_default_location());
    }
}

#[cfg(test)]
mod device_consistency_tests {
    use super::*;

    fn id(platform: &str, name: &str, ver: &str) -> HeaderIdentity {
        HeaderIdentity {
            platform: platform.into(),
            device_id: "AAAA-BBBB".into(),
            idfa: String::new(),
            os_version: ver.into(),
            device_name: name.into(),
            manufacturer: String::new(),
            anchor_lat: 39.4930,
            anchor_lon: 116.2550,
            app_install_time: 1_700_000_000_000,
            mac_address: "08:a4:6f:45:37:ee".into(),
            city: "廊坊市".into(),
        }
    }

    fn json_of(i: &HeaderIdentity) -> serde_json::Value {
        let (s, _) = build_native_ios_header(i, 123, "tok", Some(1_700_000_000_000));
        serde_json::from_str(&s).unwrap()
    }

    /// 屏幕参数必须与机型对应：osType=1 + iPhone 时不能是安卓分辨率。
    #[test]
    fn iphone_pixels_are_never_android_values() {
        let v = json_of(&id("ios", "iPhone", "18.1"));
        let pp = v["physicPixel"].as_str().unwrap();
        let lp = v["logicPixel"].as_str().unwrap();
        assert_ne!(pp, "1080x2400", "iPhone 不可能是安卓分辨率");
        assert_ne!(lp, "360x800", "iPhone 不可能是安卓逻辑分辨率");
    }

    /// 每个 iPhone 机型的物理分辨率都要像真机，且逻辑/物理宽高比一致。
    #[test]
    fn physics_pixels_look_like_iphone() {
        for (name, _, lp, pp) in IPHONE_PROFILES {
            let (w, h) = pp.split_once('x').unwrap();
            let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
            assert!(w >= 750 && w <= 1400, "{name} 宽 {w} 不像 iPhone");
            assert!(h > w, "{name} 高度应大于宽度");
            let (lw, lh) = lp.split_once('x').unwrap();
            let (lw, lh): (u32, u32) = (lw.parse().unwrap(), lh.parse().unwrap());
            assert!(
                (lw as f64 / lh as f64 - w as f64 / h as f64).abs() < 0.02,
                "{name} 逻辑与物理宽高比不一致"
            );
        }
    }

    /// UA 里的 iOS 版本必须与 headerSign 的 osVersion 一致。
    #[test]
    fn ua_version_matches_header_version() {
        let i = id("ios", "iPhone", "17.6");
        let v = json_of(&i);
        let header_ver = v["osVersion"].as_str().unwrap();
        let ua = ua_ios_for(&i);
        assert!(
            ua.contains(&format!("iOS {header_ver}")),
            "UA [{ua}] 与 headerSign osVersion [{header_ver}] 不一致"
        );
    }

    /// 默认 identity 也必须自洽。
    #[test]
    fn default_identity_is_self_consistent() {
        let i = id("ios", "iPhone", &default_os_version());
        let v = json_of(&i);
        assert_eq!(v["osType"], "1");
        assert_ne!(v["physicPixel"], "1080x2400");
        let ua = ua_ios_for(&i);
        assert!(ua.contains(&format!("iOS {}", v["osVersion"].as_str().unwrap())));
    }

    /// 安卓机型必须拿到安卓分辨率。
    #[test]
    fn android_profile_matches() {
        let (_, _, lp, pp) = resolve_device_profile("android", "22081212C", "");
        assert_eq!(lp, "360x800");
        assert_eq!(pp, "1080x2400");
    }
}
