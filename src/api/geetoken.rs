//! 极验 GeeGuard 本地 geeToken 生成（纯离线，不依赖 unidbg / Java / 真机）。
//!
//! # 算法来源
//!
//! 三条独立证据，全部在本地复算通过：
//!
//! 1. **AES-256 密钥**：`libgtcore.so` 的 `.rodata` 偏移 `0x5807` 起 32 字节，
//!    逐字节 XOR `0x6e` 即为密钥。原始字节
//!    `393d5f000c393f0823245f030b1c061f375b3c1d283427161c24415b5a242b53`
//!    → `WS1nbWQfMJ1merhqY5RsFZIxrJ/54JE=`。
//! 2. **黄金向量**：`plain = {"a":1}`、`random4 = 11 22 33 44` 加密后必须是
//!    `R0VFAAUxMTIyMzM0NDAxNWFiZDdmAAAAECc8nwc1mlvLNbq5fbAGXQs=`。
//! 3. **AppID**：`R.string.geet_geeguard_id`，通过手写 ARSC 解析器绑定证实 ——
//!    资源键索引 `14766`，`ResTable_entry @0x2d40e0` → `Res_value @0x2d40e8`
//!    （`dtype=0x03`，`data=5562`）→ 全局字符串池第 5562 项。
//!
//! # 容器格式
//!
//! ```text
//! key    = 上述 32 字节 ASCII
//! digest = SHA256(紧凑 JSON)
//! nonce  = random4 || digest[0..4]              // 8 字节
//! iv     = nonce 的 16 个 ASCII hex 字符        // 不是 nonce 本身
//! body   = AES-256-CBC(PKCS7(明文), key, iv)
//! token  = base64("GEE\0" + 0x05 + nonce_hex + u32BE(len(body)) + body)
//! ```
//!
//! # 两级 token
//!
//! | 形态 | base64 前缀 | 长度 | 来源 | 配对的 `errorCode` |
//! |---|---|---|---|---|
//! | 本地降级 `GEE\0` | `R0VF` | ~3384 | **本模块离线生成** | `-300` |
//! | 服务端 `G02\0` | `RzAy` | ~348–500 | POST 到 `client_report` 换回 | `200` |

use serde_json::Value;

/// 本地 token 前缀（`GEE\0` 的 base64 开头）。
pub const TOKEN_PREFIX_LOCAL: &str = "R0VF";
/// 服务端 token 前缀（`G02\0` 的 base64 开头）。
pub const TOKEN_PREFIX_SERVER: &str = "RzAy";

/// 极验回执接口。
pub const CLIENT_REPORT_URL: &str = "https://riskct.geetest.com/g2/api/v1/client_report";

/// `client_report` 的 `AppID` 头。
///
/// 取自 `R.string.geet_geeguard_id`，**不是** token 明文里的数字 `appId`
/// （那个是 12439574，另一层级）。拿错会得到 `{"code":-50101,"msg":"not app_id"}`。
pub const GT_APP_ID: &str = "mhs0n2abroy5ayzs5adtslzzsk1cya6h";

/// token 明文里的数字 appId（仅记录，不要当作 `AppID` 头）。
pub const GT_TOKEN_APP_ID: &str = "12439574";

/// 32 字节 AES-256 密钥（`.rodata 0x5807` 的 32 字节逐字节 XOR `0x6e`）。
pub const GT_KEY: &[u8; 32] = b"WS1nbWQfMJ1merhqY5RsFZIxrJ/54JE=";

/// 由 token 形态推导 `errorCode`（回执与提交必须一致）。
///
/// * `G02\0`（base64 以 `RzAy` 开头）= 服务端 token → `200`
/// * `GEE\0`（base64 以 `R0VF` 开头）= 本地降级 token → `-300`
/// * 空串 = 兜底失败 → `-999`
pub fn code_for_token(token: &str) -> i64 {
    let t = token.trim();
    if t.is_empty() {
        return -999;
    }
    if t.starts_with(TOKEN_PREFIX_SERVER) || t.starts_with("G02") {
        200
    } else if t.starts_with(TOKEN_PREFIX_LOCAL) || t.starts_with("GEE") {
        -300
    } else {
        -999
    }
}

fn hex_lower(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 0x0f) as usize] as char);
    }
    s
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect()
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex_lower(&Sha256::digest(data))
}

/// 用已还原的算法加密：`plain` 是紧凑 JSON，`random4` 是 4 字节随机数。
pub fn encrypt_token(plain: &[u8], random4: [u8; 4]) -> String {
    use cbc::cipher::{block_padding::Pkcs7, BlockModeEncrypt, KeyIvInit};
    type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;

    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(plain);
    let mut nonce = [0u8; 8];
    nonce[..4].copy_from_slice(&random4);
    nonce[4..].copy_from_slice(&digest[..4]);
    let nonce_hex = hex_lower(&nonce).into_bytes(); // 16 个 ASCII hex 字符

    let iv: [u8; 16] = nonce_hex[..16].try_into().expect("nonce_hex 固定 16 字节");
    let key: [u8; 32] = *GT_KEY;
    let body = Aes256CbcEnc::new(&key.into(), &iv.into()).encrypt_padded_vec::<Pkcs7>(plain);

    let mut out = Vec::with_capacity(25 + body.len());
    out.extend_from_slice(b"GEE\0");
    out.push(0x05);
    out.extend_from_slice(&iv);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);

    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(&out)
}

/// 解密 token（诊断用）：还原出明文 JSON。
pub fn decrypt_token(token: &str) -> Result<String, String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(token.trim().as_bytes())
        .map_err(|e| format!("base64 解码失败: {e}"))?;
    if raw.len() < 25 || &raw[..4] != b"GEE\0" {
        return Err("不是 GEE 容器".into());
    }
    let len = u32::from_be_bytes([raw[21], raw[22], raw[23], raw[24]]) as usize;
    if 25 + len > raw.len() {
        return Err(format!("长度字段 {len} 超出容器（总长 {}）", raw.len()));
    }
    let body = &raw[25..25 + len];

    use cbc::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
    type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;
    let key: [u8; 32] = *GT_KEY;
    let iv: [u8; 16] = raw[5..21]
        .try_into()
        .map_err(|_| "IV 长度不是 16".to_string())?;
    let dec = Aes256CbcDec::new(&key.into(), &iv.into())
        .decrypt_padded_vec::<Pkcs7>(body)
        .map_err(|e| format!("AES 解密失败: {e}"))?;
    String::from_utf8(dec).map_err(|e| format!("明文不是 UTF-8: {e}"))
}

/// 首次生成后必须**缓存复用**的设备串 `d`（否则 `fp` 变化，等于换了台设备）。
pub fn fresh_device_string(seed: &str, now_ms: i64) -> String {
    let h = crate::crypto::envelope::md5_hex(seed.as_bytes());
    format!("{}{}{}", &h[..20], &h[20..32], now_ms)
}

/// 构造 scene 回执的 payload：`{"attr":"{...}","scene":<n>}`。
///
/// 与真机 `DeepKnowUtils.OooO0O0` 一致：`uuid` **非空才**放进 `attr` ——
/// `scene=4`（uuid 空）只有 uid；`scene=6/7`（uuid 非空）带 uid+uuid。
pub fn scene_payload(uid: i64, uuid: &str, scene: i64) -> String {
    if uuid.is_empty() {
        format!(
            "{{\"attr\":\"{{\\\"uid\\\":{}}}\",\"scene\":{}}}",
            uid, scene
        )
    } else {
        format!(
            "{{\"attr\":\"{{\\\"uid\\\":{},\\\"uuid\\\":\\\"{}\\\"}}\",\"scene\":{}}}",
            uid, uuid, scene
        )
    }
}

/// 明文 JSON 模板（真机结构；动态字段用占位符）。
///
/// * `data.dev` —— 设备/包信息（40 字段）
/// * `data.test` —— 环境探测结果
/// * `data.gid` —— geeid 缓存：`d` / `fp=SHA256(d)` / `sign` / `"3.0"` / `ts`
/// * `appId` / `lib` / `libVer` / `ct` / `crt` / `level` —— 常量
/// * `sign` —— `SHA256(payload)`
/// * `ts` / `et` —— 毫秒时间戳 / 有效秒数
const GT_PLAIN_TEMPLATE: &str = r##"{"data":{"dev":{"app":"{{DEV_app}}","ven":"{{DEV_ven}}","ps":"{{DEV_ps}}","ps2":"{{DEV_ps2}}","prp":"{{DEV_prp}}","pla":"{{DEV_pla}}","mod":"{{DEV_mod}}","osv":"{{DEV_osv}}","rel":"{{DEV_rel}}","bui":"{{DEV_bui}}","bs":{{DEV_bs}},"br":{{DEV_br}},"plu":{{DEV_plu}},"hea":{{DEV_hea}},"btv":{{DEV_btv}},"cou":"{{DEV_cou}}","lang":"{{DEV_lang}}","w":{{DEV_w}},"h":{{DEV_h}},"bt":{{DEV_bt}},"bri":{{DEV_bri}},"s":{{DEV_s}},"op":"{{DEV_op}}","car":"{{DEV_car}}","btc":{{DEV_btc}},"rad":"{{DEV_rad}}","mem":"{{DEV_mem}}","tsz":{{DEV_tsz}},"asz":{{DEV_asz}},"os":"{{DEV_os}}","loc":{{DEV_loc}},"ism":{{DEV_ism}},"ip":{{DEV_ip}},"sim":{{DEV_sim}},"ssid":"{{DEV_ssid}}","bssid":"{{DEV_bssid}}","rssi":{{DEV_rssi}},"gpa":{{DEV_gpa}},"hdt":"{{DEV_hdt}}","aum":{{DEV_aum}}},"test":{"ir":1,"edp":["su"],"ka":{"st":"9f3a7c21b4e85d06"},"iam":0,"idse":0,"iod":0,"idv":1,"ads":"running","dtp":0,"idc":0,"xpd":1,"sub":0,"slp":0,"smp":[],"sh":"4c4cc77bbda12e6684d40f92fe8b5607","nhdp":[],"fi":{},"ghdp":[],"rtp":[],"rrt":[],"rrs":[],"rmp":[],"pm":[],"sp":["r"],"ro":[],"sfp":[],"iss":0,"py":{},"onv":1,"gas":[],"lauf":"2022-05-10 04:00:00","lauv":"7.3.93","laun":"阳光体育","lau":"","pat":"/data/data/com.zjwh.android_wh_physicalfitness/files","sen":[],"sou":"","suv":"","mua":"Dalvik/2.1.0 (Linux; U; Android 12; Redmi K30 5G Build/SKQ1.211019.001)","brl":{"man":"LGE","pla":"Redmi","mod":"Redmi K30 5G","osv":"12","api":"23","pro":"bullhead","abl":"arm64-v8a,armeabi-v7a,armeabi","cai":"arm64-v8a","soc":"msm8992","fp":"google/bullhead/bullhead:6.0/MDA89E/2296692:user/release-keys","cn":""},"sb":0,"build":{"gss":"ABSENT","gvb":"M8994F-2.6.22.0.56","gvri":"Qualcomm RIL 1.0","nh":"android-9c25757206da9a09","rbdu":"1443642794","rbd":"Wed Sep 30 19:53:14 UTC 2015","rbdi":"MDA89E","rbh":"vpef12.mtv.corp.google.com","rbi":"MDA89E","rpb":"bullhead","gnt":"Unknown","rcgc":"android-google","gon":"","rh":"bullhead","rmvr":"","rhbd":"","rchvh":""},"nnp":-1,"srs":0,"bid":"","sdi":"","sat":{"sam":"0|0|m","saa":"0|0|a"},"fit":"1652155200","lut":"1652155200"},"gid":{"m":"","m_m":"Nexus 5X","d":"{{D}}","d_u":"","3.0":"{{G30}}","sign":"{{GSIGN}}","fp":"{{FP}}","fp_k":"2","ver":"3.0.0","ts":"{{GTS}}","name":"geeid","oaid":"","uid":"","sot":"","mid":"","vmds":""}},"appId":"12439574","libVer":"2.7.4","sign":"{{SIGN}}","lib":"GGD","ct":"1","crt":"5","level":10,"ts":{{TS}},"et":{{ET}}}"##;

/// 真机 `data.dev` 的 40 个设备字段。
///
/// 默认值取自一次真实生成。`ven/ps/ps2/prp/rel/bui` 必须与目标 App 一致；
/// `mod/pla/osv/w/h` 应与身份档案同源。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GtDevProfile {
    /// 应用名（URL 编码）。真机恒为「阳光体育」，不是 APK 启动器名「运动世界」。
    pub app: String,
    /// 包名。
    pub ven: String,
    /// APK 签名 MD5（32 位大写 hex）。
    pub ps: String,
    /// APK 签名 SHA-1（40 位小写 hex）。
    pub ps2: String,
    /// APK 安装路径。
    pub prp: String,
    /// 厂商 / Brand。
    pub pla: String,
    /// 机型（`Build.MODEL`）。
    #[serde(rename = "mod")]
    pub model: String,
    /// 系统版本（`Build.VERSION.RELEASE`）。
    pub osv: String,
    /// App 版本名。
    pub rel: String,
    /// App 构建号。
    pub bui: String,
    pub bs: i64,
    pub br: i64,
    pub plu: i64,
    pub hea: i64,
    pub btv: i64,
    /// 国家。
    pub cou: String,
    /// 语言。
    pub lang: String,
    /// 屏幕宽 px。
    pub w: i64,
    /// 屏幕高 px。
    pub h: i64,
    /// 开机时长 ms。
    pub bt: i64,
    /// 屏幕亮度 0-255。
    pub bri: i64,
    /// 屏幕密度。
    pub s: f64,
    /// 运营商 MCC+MNC。
    pub op: String,
    /// 运营商名称。
    pub car: String,
    /// 电池容量 mAh。
    pub btc: i64,
    /// 网络类型。
    pub rad: String,
    /// 内存信息串。
    pub mem: String,
    /// 存储总量。
    pub tsz: i64,
    /// 存储可用。
    pub asz: i64,
    /// 系统标识。
    pub os: String,
    /// 定位状态。
    pub loc: Value,
    /// 模拟器标记（真机必须 0）。
    pub ism: i64,
    /// 内网 IP。
    pub ip: Value,
    /// SIM 状态。
    pub sim: i64,
    pub ssid: String,
    pub bssid: String,
    pub rssi: i64,
    /// 传感器标记。
    pub gpa: i64,
    pub hdt: String,
    pub aum: i64,
}

impl Default for GtDevProfile {
    fn default() -> Self {
        Self {
            app: "%E9%98%B3%E5%85%89%E4%BD%93%E8%82%B2".into(),
            ven: "com.zjwh.android_wh_physicalfitness".into(),
            ps: "A1B2C3D4E5F60718293A4B5C6D7E8F90".into(),
            ps2: "767dd585192630228dc58c782a3832e99ca0ec5b".into(),
            prp: "/data/app/com.zjwh.android_wh_physicalfitness/base.apk".into(),
            pla: "google".into(),
            model: "Nexus 5X".into(),
            osv: "6.0".into(),
            rel: "7.3.93".into(),
            bui: "7393".into(),
            bs: -1,
            br: -1,
            plu: -1,
            hea: -1,
            btv: -1,
            cou: "CN".into(),
            lang: "zh".into(),
            w: 1080,
            h: 2340,
            bt: 1_990_654_450,
            bri: 0,
            s: 2.75,
            op: "46000".into(),
            car: "China Mobile".into(),
            btc: 4500,
            rad: "WIFI".into(),
            mem: String::new(),
            tsz: 0,
            asz: 0,
            os: "android".into(),
            loc: serde_json::json!({"st":0,"se":"1"}),
            ism: 0,
            ip: serde_json::json!({}),
            sim: 1,
            ssid: String::new(),
            bssid: String::new(),
            rssi: -1,
            gpa: 1,
            hdt: String::new(),
            aum: 0,
        }
    }
}

impl GtDevProfile {
    /// 从身份档案派生机型 / 系统 / 分辨率 / 版本，保证与 HTTP 头一致。
    pub fn from_identity(id: &crate::crypto::header::HeaderIdentity) -> Self {
        let mut p = Self::default();
        let (_, osv, _, physic) = crate::crypto::header::resolve_device_profile(
            &id.platform,
            &id.device_name,
            &id.os_version,
        );
        p.model = id.device_name.clone();
        p.osv = osv;
        p.pla = if id.manufacturer.is_empty() {
            p.pla
        } else {
            id.manufacturer.clone()
        };
        if let Some((w, h)) = parse_px(&physic) {
            p.w = w;
            p.h = h;
        }
        p
    }

    fn placeholders(&self) -> Vec<(String, String)> {
        vec![
            ("{{DEV_app}}".into(), self.app.clone()),
            ("{{DEV_ven}}".into(), self.ven.clone()),
            ("{{DEV_ps}}".into(), self.ps.clone()),
            ("{{DEV_ps2}}".into(), self.ps2.clone()),
            ("{{DEV_prp}}".into(), self.prp.clone()),
            ("{{DEV_pla}}".into(), self.pla.clone()),
            ("{{DEV_mod}}".into(), self.model.clone()),
            ("{{DEV_osv}}".into(), self.osv.clone()),
            ("{{DEV_rel}}".into(), self.rel.clone()),
            ("{{DEV_bui}}".into(), self.bui.clone()),
            ("{{DEV_bs}}".into(), self.bs.to_string()),
            ("{{DEV_br}}".into(), self.br.to_string()),
            ("{{DEV_plu}}".into(), self.plu.to_string()),
            ("{{DEV_hea}}".into(), self.hea.to_string()),
            ("{{DEV_btv}}".into(), self.btv.to_string()),
            ("{{DEV_cou}}".into(), self.cou.clone()),
            ("{{DEV_lang}}".into(), self.lang.clone()),
            ("{{DEV_w}}".into(), self.w.to_string()),
            ("{{DEV_h}}".into(), self.h.to_string()),
            ("{{DEV_bt}}".into(), self.bt.to_string()),
            ("{{DEV_bri}}".into(), self.bri.to_string()),
            ("{{DEV_s}}".into(), format!("{}", self.s)),
            ("{{DEV_op}}".into(), self.op.clone()),
            ("{{DEV_car}}".into(), self.car.clone()),
            ("{{DEV_btc}}".into(), self.btc.to_string()),
            ("{{DEV_rad}}".into(), self.rad.clone()),
            ("{{DEV_mem}}".into(), self.mem.clone()),
            ("{{DEV_tsz}}".into(), self.tsz.to_string()),
            ("{{DEV_asz}}".into(), self.asz.to_string()),
            ("{{DEV_os}}".into(), self.os.clone()),
            (
                "{{DEV_loc}}".into(),
                serde_json::to_string(&self.loc).unwrap_or_else(|_| "null".into()),
            ),
            ("{{DEV_ism}}".into(), self.ism.to_string()),
            (
                "{{DEV_ip}}".into(),
                serde_json::to_string(&self.ip).unwrap_or_else(|_| "null".into()),
            ),
            ("{{DEV_sim}}".into(), self.sim.to_string()),
            ("{{DEV_ssid}}".into(), self.ssid.clone()),
            ("{{DEV_bssid}}".into(), self.bssid.clone()),
            ("{{DEV_rssi}}".into(), self.rssi.to_string()),
            ("{{DEV_gpa}}".into(), self.gpa.to_string()),
            ("{{DEV_hdt}}".into(), self.hdt.clone()),
            ("{{DEV_aum}}".into(), self.aum.to_string()),
        ]
    }
}

fn parse_px(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once('x')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// 生成本地 token 所需的业务参数。
#[derive(Clone, Debug)]
pub struct GtRun {
    pub uid: i64,
    pub uuid: String,
    pub scene: i64,
    /// 设备串 `d`：首次生成后必须缓存复用。
    pub d: String,
    /// 毫秒时间戳（外层 `ts`）。
    pub ts_ms: i64,
    /// 有效秒数（外层 `et`，真机样本 2846~2980）。
    pub et: i64,
    /// `gid.ts`（秒）。真机是 geeid 创建时刻，可能比 `ts` 早 1~3 秒。
    pub gid_ts: Option<i64>,
}

/// 组装明文 JSON（与真机逐字节同构：字段顺序、紧凑格式都一致）。
pub fn build_plain_with(r: &GtRun, dev: &GtDevProfile) -> String {
    let fp = sha256_hex(r.d.as_bytes());
    let gts = r.gid_ts.unwrap_or(r.ts_ms / 1000).to_string();
    let payload = scene_payload(r.uid, &r.uuid, r.scene);
    let sign = sha256_hex(payload.as_bytes());
    let gid_digest = sha256_hex(format!("G02{fp}{gts}").as_bytes());
    let gid_sign = gid_digest[gid_digest.len() - 16..].to_string();
    let g30 = {
        use base64::Engine;
        let tail = &hex_to_bytes(&gid_digest)[26..32];
        format!(
            "G02-{gts}-{fp}-{}",
            base64::engine::general_purpose::STANDARD.encode(tail)
        )
    };

    let mut s = GT_PLAIN_TEMPLATE
        .replace("{{SIGN}}", &sign)
        .replace("{{G30}}", &g30)
        .replace("{{FP}}", &fp)
        .replace("{{D}}", &r.d)
        .replace("{{GSIGN}}", &gid_sign)
        .replace("{{GTS}}", &gts)
        .replace("{{TS}}", &r.ts_ms.to_string())
        .replace("{{ET}}", &r.et.to_string());
    for (k, v) in dev.placeholders() {
        s = s.replace(&k, &v);
    }
    s
}

/// 生成结果。
#[derive(Clone, Debug)]
pub struct LocalToken {
    pub token: String,
    pub gee_id: String,
    pub app_id: String,
    /// 明文 JSON，用于和提交体做一致性自检。
    pub plain: Value,
    /// 与 token 配对的 `errorCode`。
    pub error_code: i64,
}

/// 纯 Rust 离线生成本地 geeToken（`GEE\0` 形态，等价真机 `-300` 降级）。
///
/// `install_ms` 为 App 安装时刻；`<= 0` 时用当前时间（会改变 `d`，仅调试用）。
pub fn generate_local(
    uid: i64,
    uuid: &str,
    scene: i64,
    install_ms: i64,
    dev: &GtDevProfile,
) -> LocalToken {
    let now = crate::crypto::envelope::now_ms();
    let born = if install_ms > 0 { install_ms } else { now };
    let d = fresh_device_string(uuid, born);
    let r = GtRun {
        uid,
        uuid: uuid.to_string(),
        scene,
        d,
        ts_ms: now,
        et: {
            use rand::Rng;
            rand::thread_rng().gen_range(2846..=2980)
        },
        gid_ts: None,
    };
    let plain = build_plain_with(&r, dev);
    let mut r4 = [0u8; 4];
    {
        use rand::Rng;
        rand::thread_rng().fill(&mut r4);
    }
    let token = encrypt_token(plain.as_bytes(), r4);
    let pv: Value = serde_json::from_str(&plain).unwrap_or(Value::Null);
    let gee_id = pv
        .get("data")
        .and_then(|d| d.get("gid"))
        .and_then(|g| g.get("3.0"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let error_code = code_for_token(&token);
    LocalToken {
        token,
        gee_id,
        app_id: GT_APP_ID.into(),
        plain: pv,
        error_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 密钥必须能从 `.so` 的 `.rodata 0x5807` 逐字节 XOR 0x6e 还原（本测试固化该常量）。
    #[test]
    fn key_matches_so_rodata() {
        assert_eq!(GT_KEY.len(), 32);
        assert!(GT_KEY.iter().all(|b| b.is_ascii()));
        assert_eq!(GT_KEY, b"WS1nbWQfMJ1merhqY5RsFZIxrJ/54JE=");
    }

    /// 黄金向量：与独立参考实现逐字节一致。
    #[test]
    fn encrypt_matches_golden_vector() {
        let tok = encrypt_token(br#"{"a":1}"#, [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(
            tok, "R0VFAAUxMTIyMzM0NDAxNWFiZDdmAAAAECc8nwc1mlvLNbq5fbAGXQs=",
            "AES-256-CBC 本地 token 与参考实现不一致"
        );
    }

    /// 容器结构：`GEE\0` + 0x05 + nonce_hex(16) + u32BE(len) + body。
    #[test]
    fn token_container_shape() {
        use base64::Engine;
        let plain = br#"{"b":2}"#;
        let tok = encrypt_token(plain, [1, 2, 3, 4]);
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&tok)
            .unwrap();
        assert_eq!(&raw[..4], b"GEE\0");
        assert_eq!(raw[4], 0x05);
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(plain);
        assert_eq!(&raw[13..21], hex_lower(&d[..4]).as_bytes());
        let len = u32::from_be_bytes([raw[21], raw[22], raw[23], raw[24]]) as usize;
        assert_eq!(25 + len, raw.len(), "头里的长度必须等于 body 长度");
    }

    /// 加密再解密必须回到原文。
    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plain = br#"{"x":"y","n":1}"#;
        let tok = encrypt_token(plain, [9, 8, 7, 6]);
        assert_eq!(decrypt_token(&tok).unwrap().as_bytes(), plain);
    }

    /// `errorCode` 必须与 token 形态配对。
    #[test]
    fn error_code_follows_token_shape() {
        assert_eq!(code_for_token(""), -999);
        assert_eq!(code_for_token("   "), -999);
        assert_eq!(code_for_token("RzAyABQ..."), 200);
        assert_eq!(code_for_token("R0VFAAU..."), -300);
        assert_eq!(code_for_token("zzz"), -999);
    }

    /// `scene=4` 不带 uuid；`scene=6/7` 带 uuid。
    #[test]
    fn scene_payload_matches_device_semantics() {
        assert_eq!(
            scene_payload(100, "", 4),
            r#"{"attr":"{\"uid\":100}","scene":4}"#
        );
        assert_eq!(
            scene_payload(100, "u-1", 4),
            r#"{"attr":"{\"uid\":100,\"uuid\":\"u-1\"}","scene":4}"#
        );
    }

    /// 完整生成：前缀、长度、geeID 形态、以及密文可自解回合法 JSON。
    #[test]
    fn generate_local_is_self_consistent() {
        let dev = GtDevProfile::default();
        let t = generate_local(
            16835824,
            "7AEF1253-D675-48B3-9D64-9A1E01DB38C7",
            4,
            1_780_000_000_000,
            &dev,
        );
        assert!(t.token.starts_with("R0VF"), "前缀应为 R0VF");
        assert!(t.gee_id.starts_with("G02-"), "geeID 形态: {}", t.gee_id);
        assert_eq!(t.app_id, GT_APP_ID);
        assert_eq!(t.error_code, -300);
        assert!(t.token.len() > 3000, "token 长度 {}", t.token.len());

        let plain = decrypt_token(&t.token).expect("密文必须能自解");
        let v: Value = serde_json::from_str(&plain).expect("明文必须是合法 JSON");
        // 关键字段齐备
        for k in [
            "data", "appId", "lib", "libVer", "sign", "ts", "et", "ct", "crt", "level",
        ] {
            assert!(v.get(k).is_some(), "缺字段 {k}");
        }
        let dev_obj = v["data"]["dev"].as_object().expect("data.dev 必须是对象");
        assert_eq!(dev_obj.len(), 40, "data.dev 应有 40 个字段");
        // fp 必须是 SHA256(d)
        let d = v["data"]["gid"]["d"].as_str().unwrap();
        assert_eq!(
            v["data"]["gid"]["fp"].as_str().unwrap(),
            sha256_hex(d.as_bytes())
        );
        // 顶层 sign 必须是 SHA256(payload)
        let payload = scene_payload(16835824, "7AEF1253-D675-48B3-9D64-9A1E01DB38C7", 4);
        assert_eq!(v["sign"].as_str().unwrap(), sha256_hex(payload.as_bytes()));
        // 占位符必须全部替换完
        assert!(!plain.contains("{{"), "还有未替换的占位符");
    }

    /// 两次生成的 token 必须不同（nonce 随机），但 geeID 的 `fp` 部分应稳定（同 d）。
    #[test]
    fn tokens_are_randomized_but_device_stable() {
        let dev = GtDevProfile::default();
        let a = generate_local(1, "dev-seed", 4, 1_780_000_000_000, &dev);
        let b = generate_local(1, "dev-seed", 4, 1_780_000_000_000, &dev);
        assert_ne!(a.token, b.token, "nonce 必须随机");
        assert_eq!(
            a.plain["data"]["gid"]["fp"], b.plain["data"]["gid"]["fp"],
            "同一设备串的 fp 必须一致"
        );
    }
}
