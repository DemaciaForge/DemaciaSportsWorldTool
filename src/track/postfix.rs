//! 轨迹后处理：结尾断崖、起终点哨兵、后半段类型散布、首点与索引 2 的边界修正。
//!
//! 这里的每一次改写都**同时改写时间与距离**，不再出现「坐标改了、累计量不动」：
//!
//! - 结尾断崖：把最后一段的时间与位移按收尾速度重算，末点落在一个真实位置上，
//!   其后不再有长间隔采样点；
//! - 起点哨兵（索引 1）：`setStartPoint` 的零值标记，时间与累计量都为 0；
//! - 索引 2：`avgSpeed` 由它自己的位移/时间差给出，不再被夹到窗口边界（夹值会
//!   制造「速度字段」与「位移」系统性不等）。

use super::geom::{fmt_gain_time, round_to, Rng, MET_PER_DEG_LAT, MET_PER_DEG_LNG};
use super::model::GenPoint;

/// 收尾速度（m/s）：真人到终点前的最后几十米会明显收速。
pub const STOP_SPEED_RANGE: (f64, f64) = (2.0, 2.6);

pub fn apply_post_fixes(locs: &mut [GenPoint], rng: &mut Rng, start_ms: i64) {
    // ① 末点的坐标与位移由生成器的收尾步给出（沿路线继续走「收尾配速 × 末段时长」），
    //    这里只把它的速度字段与时间标记对齐到收尾语义。
    //
    //    收尾速度取「最后一段的实际平均速度」，且不高于 STOP_SPEED_RANGE 的上沿：
    //    生成器已经把末点放在收尾配速上，这里再重算一次只会丢失两位小数。
    if locs.len() >= 4 {
        let last = locs.len() - 1;
        let dt = (locs[last].totalTime - locs[last - 1].totalTime).max(1) as f64;
        let seg = ((locs[last].gLat - locs[last - 1].gLat) * MET_PER_DEG_LAT)
            .hypot((locs[last].gLng - locs[last - 1].gLng) * MET_PER_DEG_LNG);
        let stop_speed = (seg / dt).clamp(0.5, STOP_SPEED_RANGE.1);
        locs[last].speed = round_to(stop_speed * 3.6, 4);
        locs[last].avgSpeed = round_to(stop_speed, 4);
        locs[last].bdS = round_to(rng.uniform(0.05, 0.15), 3);
        locs[last].validTime = locs[last].totalTime;
        // 不要把末点 gainTime 重写成严格等于 totalTime。
        //
        // 真机的「钟跨 − totalTime」实测为 +3 ～ +996 s（9 条真跑全为正）：
        // 按下结束后 App 仍在接收几拍定位，这几拍不计入 totalTime。
        // 这里保留生成器已经算好的偏移；只在它为 0（生成器没加）时才补一个正拖尾。
        if locs[last].gainTimeMs <= start_ms + locs[last].totalTime * 1000 {
            let tail = start_ms + locs[last].totalTime * 1000 + rng.uniform(3000.0, 90000.0) as i64;
            locs[last].gainTime = fmt_gain_time(tail);
            locs[last].gainTimeMs = tail;
        }
    }

    if locs.len() > 2 {
        // ② 起点哨兵（type=5 → setStartPoint）：全零累计 + state=1
        let sp = &mut locs[1];
        sp.ptype = 5;
        sp.state = 1;
        sp.locType = 1;
        sp.radius = round_to(rng.uniform(2.0, 3.5), 2);
        sp.speed = 0.0;
        sp.avgSpeed = 0.0;
        sp.bdS = 0.0;
        sp.totalDis = 0.0;
        sp.validDis = 0.0;
        sp.totalTime = 0;
        sp.validTime = 0;
        sp.steps = 0;

        // ③ 终点哨兵
        let last = locs.len() - 1;
        let e = &mut locs[last];
        e.ptype = 6;
        e.locType = 1;
        e.radius = round_to(rng.uniform(1.5, 3.0), 2);

        // ④ 后半段散布 type=1/2/8（67-99% 行程区间；type=8 真人低频出现）
        let from = (locs.len() as f64 * 0.67) as usize;
        for i in from..locs.len().saturating_sub(2) {
            if rng.random() < 0.06 {
                locs[i].ptype = rng.choice(&[1, 1, 2, 8]);
                locs[i].locType = 1;
            }
        }
    }

    // ⑤ 索引 2 是首个真实点：avgSpeed 由它自己的位移/时间差给出
    if locs.len() > 3 {
        let d = ((locs[2].gLat - locs[1].gLat) * MET_PER_DEG_LAT)
            .hypot((locs[2].gLng - locs[1].gLng) * MET_PER_DEG_LNG);
        let dt = (locs[2].totalTime - locs[1].totalTime).max(1) as f64;
        let v = d / dt;
        if v.is_finite() && v > 0.1 {
            locs[2].avgSpeed = round_to(v, 4);
            locs[2].speed = round_to(v * 3.6, 4);
        }
    }

    // ⑥ 首点 = 起点：totalTime=0、type∈{0,7}、速度取首个非零值
    if !locs.is_empty() {
        locs[0].totalTime = 0;
        locs[0].totalDis = 0.0;
        locs[0].validDis = 0.0;
        locs[0].validTime = 0;
        locs[0].steps = 0;
        locs[0].state = 1;
        locs[0].ptype = rng.choice(&[0, 0, 0, 7]);
        locs[0].locType = 1;
        locs[0].radius = round_to(rng.uniform(1.5, 3.0), 2);
        // 首点与起点哨兵是同一点（哨兵只是零值标记），坐标必须一致。
        if locs.len() > 1 {
            locs[0].gLat = locs[1].gLat;
            locs[0].gLng = locs[1].gLng;
        }
        for j in 1..locs.len() {
            if locs[j].avgSpeed > 0.0 {
                locs[0].avgSpeed = locs[j].avgSpeed;
                locs[0].bdS = locs[j].bdS;
                break;
            }
        }
        if locs[0].speed <= 0.0 {
            let v = locs
                .iter()
                .find(|p| p.speed > 0.0)
                .map(|p| p.speed)
                .unwrap_or(rng.uniform(7.0, 13.0));
            locs[0].speed = v;
        }
    }
}
