//! 提交台账：把每次成功提交的 `rrid` / `uuid` / 起跑时刻追加到本地 JSONL。
//!
//! 服务端的记录列表只保留最近 10 条，更早的记录会被新记录挤出列表。一旦被挤出，
//! `rrid` 和 `uuid` 就再也拿不到——而 OBS 的读取 key 正是由这两者（加起跑时刻的
//! 小时桶）推导的（见 `track::wire::obs_keys`）。也就是说：**记录被挤出列表的那一刻，
//! 它的原始轨迹就永久失联了**。
//!
//! 台账在提交成功的那一刻就把这些标识落到本地，因此任何一条历史记录都还能用
//! `obs-real --rrid <n>` 拉回来对照。
//!
//! 只记标识与汇总，不记轨迹点、不记凭据。

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

/// 台账文件名（与 `identity.json` 同目录，即可执行文件所在目录）。
pub const LEDGER_FILE: &str = "submitted_records.jsonl";

/// 一条已提交记录的本地档案。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmittedRecord {
    /// 落账时刻（毫秒）。
    pub submitted_at: i64,
    /// 服务端记录 id —— `obs-real --rrid` 用它。
    pub rrid: i64,
    /// 服务端记录 uuid —— OBS key 的一部分。
    pub uuid: String,
    /// 起跑时刻（毫秒）—— OBS key 的小时桶由它推导。
    pub start_ms: i64,
    /// 实际上报的累计距离（米）。
    pub total_dis: f64,
    /// 实际上报的总时长（秒）。
    pub total_time: i64,
    /// 实际上报的总步数。
    pub total_steps: i64,
    /// 本次请求的目标距离（米）。
    pub target_dist: f64,
    /// 本次请求的目标时长（秒）。
    pub target_dur: i64,
    /// 轨迹点数。
    pub points: usize,
    /// 几何模式：legacy / road / custom。
    pub route_mode: String,
    /// 随机种子（可复现同一条轨迹）。
    pub seed: u64,
    /// 真实性自检是否通过。
    pub realism_passed: bool,
    /// OBS 双 key 上传成功数（0–2）；提交当时还未知时为 `None`。
    pub obs_ok: Option<usize>,
}

impl SubmittedRecord {
    /// 可直接喂给 `obs-real --rrid` 的那一行。
    pub fn hint(&self) -> String {
        use chrono::TimeZone;
        format!(
            "obs-real --rrid {}   # 起跑 {}",
            self.rrid,
            chrono::Local
                .timestamp_millis_opt(self.start_ms)
                .single()
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".into())
        )
    }
}

fn ledger_path() -> PathBuf {
    crate::platform::data_dir().join(LEDGER_FILE)
}

/// 追加一条台账。返回写入了多少字节。
///
/// 写入失败不向上抛错：台账是辅助能力，不该因为磁盘问题让一次已经成功的提交
/// 变成失败。调用方只会拿到 `Err` 用于记日志。
pub fn append(entry: &SubmittedRecord) -> Result<PathBuf, String> {
    let path = ledger_path();
    let line = serde_json::to_string(entry).map_err(|e| format!("台账序列化失败: {e}"))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("台账打开失败 {}: {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("台账写入失败: {e}"))?;
    Ok(path)
}

/// 回填某条记录的 OBS 上传结果（读全量 → 改写该行 → 原子替换）。
///
/// 台账以「先落 rrid、后补 OBS 结果」两步写入：提交成功立刻落 rrid，OBS 上传结束
/// 后再补一次。这样即使进程在两步之间退出，标识也已保存。
pub fn set_obs_ok(rrid: i64, obs_ok: usize) -> Result<(), String> {
    let path = ledger_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(()),
    };
    let mut out = String::with_capacity(text.len());
    let mut touched = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<SubmittedRecord>(line) {
            Ok(mut rec) if rec.rrid == rrid => {
                rec.obs_ok = Some(obs_ok);
                touched = true;
                out.push_str(&serde_json::to_string(&rec).map_err(|e| e.to_string())?);
            }
            // 解析不了的旧行原样保留，不要因为一行坏数据丢掉整本台账。
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    if !touched {
        return Ok(());
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, out).map_err(|e| format!("台账临时文件写入失败: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("台账替换失败: {e}"))?;
    Ok(())
}

/// 读取全部台账（最新的在后）。坏行跳过。
///
/// 容忍 UTF-8 BOM：台账可能被人用记事本打开再保存，多出一个 BOM 不该让第一条
/// 记录凭空消失。
pub fn read_all() -> Vec<SubmittedRecord> {
    let Ok(text) = std::fs::read_to_string(ledger_path()) else {
        return Vec::new();
    };
    text.lines()
        .map(|l| l.trim_start_matches('\u{feff}'))
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<SubmittedRecord>(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(rrid: i64) -> SubmittedRecord {
        SubmittedRecord {
            submitted_at: 1_791_724_862_000,
            rrid,
            uuid: format!("UUID-{rrid}"),
            start_ms: 1_791_724_862_000,
            total_dis: 2_012.4,
            total_time: 918,
            total_steps: 2_019,
            target_dist: 2_000.0,
            target_dur: 920,
            points: 178,
            route_mode: "legacy".into(),
            seed: 7,
            realism_passed: true,
            obs_ok: None,
        }
    }

    /// 台账是追加写、坏行不连坐、且能回填 OBS 结果。
    #[test]
    fn ledger_appends_survives_bad_lines_and_backfills() {
        let dir = std::env::temp_dir().join(format!("nsw-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::platform::TEST_DATA_DIR.with(|p| *p.borrow_mut() = Some(dir.clone()));

        append(&sample(1001)).unwrap();
        append(&sample(1002)).unwrap();
        assert_eq!(read_all().len(), 2);

        // 掺一行坏数据：不能让它把整本台账读废。
        let path = dir.join(LEDGER_FILE);
        {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            writeln!(f, "{{not json").unwrap();
        }
        let rows = read_all();
        assert_eq!(rows.len(), 2, "坏行应被跳过而不是吞掉好行");
        assert_eq!(rows[0].rrid, 1001);

        // 回填 OBS 结果：只改目标那一行，其余不动。
        set_obs_ok(1002, 2).unwrap();
        let rows = read_all();
        assert_eq!(rows[0].obs_ok, None);
        assert_eq!(rows[1].obs_ok, Some(2));
        assert!(rows[1].hint().contains("--rrid 1002"));

        // 文件被记事本保存过（开头多一个 BOM）时，第一条记录不该消失。
        let with_bom = format!("\u{feff}{}", std::fs::read_to_string(&path).unwrap());
        std::fs::write(&path, with_bom).unwrap();
        assert_eq!(read_all().len(), 2, "BOM 不该让第一条记录消失");

        crate::platform::TEST_DATA_DIR.with(|p| *p.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
