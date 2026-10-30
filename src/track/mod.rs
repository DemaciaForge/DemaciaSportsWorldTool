//! 轨迹层：生成器 / OBS 组装 / 官方卡路里。

// 路线算法只剩一条：经典打卡点环（Catmull-Rom 连接打卡点）。
// 路网（Road）与高德自定义路线两条路径已删除——前者自带一套速度剖面与字段
// 逻辑、不走 `assemble`，后者依赖外部路径服务，都是维护负担的主要来源。

pub mod altitude;
pub mod biomech;
pub mod calorie;
pub mod generator;
pub mod geom;
pub mod model;
pub mod postfix;
pub mod realism;
pub mod shape;
pub mod stadium;
pub mod wire;

#[cfg(test)]
mod tests {
    use super::generator::build;
    use super::geom::{MET_PER_DEG_LAT, MET_PER_DEG_LNG};

    use super::wire::*;

    pub(super) fn sample_points() -> Vec<(f64, f64)> {
        // 某校园 5 个打卡点（BD 系）
        vec![
            (38.901678, 121.540241),
            (38.902564, 121.541233),
            (38.900921, 121.542310),
            (38.899823, 121.541010),
            (38.900455, 121.539512),
        ]
    }

    /// 轨迹生成抽样断言：距离精确、采样间隔分布、哨兵/断崖/位移语义。
    #[test]
    fn test_generator_distribution() {
        let pts = sample_points();
        let start = 1_788_958_186_123i64;
        let track = build(3300.0, 1220, 42, (38.9, 121.54), start, &pts);
        assert!(
            track.locations.iter().all(|point| point.ptype != -1),
            "campus track must not contain invalid drift points"
        );
        // 声明距离取自「按坐标实测的位移」，与规划值的偏差控制在 3% 以内
        // （偏差来自横向漂移与进场/离场段的几何，属于真实轨迹固有的那一部分）。
        assert!(
            (track.totalDistance / 3300.0 - 1.0).abs() < 0.03,
            "dist={}",
            track.totalDistance
        );
        assert_eq!(track.totalTime, 1220);
        // 点数合理（主 5s 采样）
        let n = track.locations.len();
        assert!((200..320).contains(&n), "n={n}");
        // 哨兵：索引0 type∈{0,7}/totalTime=0/state=1；索引1 type=5 全零；末点 type=6
        assert!([0, 7].contains(&track.locations[0].ptype));
        assert_eq!(track.locations[0].totalTime, 0);
        assert_eq!(track.locations[0].state, 1);
        assert_eq!(track.locations[1].ptype, 5);
        assert_eq!(track.locations[1].totalDis, 0.0);
        assert_eq!(track.locations[1].steps, 0);
        assert_eq!(track.locations.last().unwrap().ptype, 6);
        // 累计距离单调不减、末点 ≈ 总距离
        let mut prev = 0.0;
        for p in &track.locations {
            assert!(p.totalDis >= prev - 1e-6, "totalDis 回退");
            prev = p.totalDis;
        }
        // 距离只由正常点承担：终点哨兵(type=6)携带全程累计距离
        assert!(
            (track.locations.last().unwrap().totalDis - track.totalDistance).abs() < 1.0,
            "末点={}",
            track.locations.last().unwrap().totalDis
        );
        // 采样间隔：5s 占比 ≥ 60%
        let mut fives = 0;
        let mut total = 0;
        for w in track.locations.windows(2) {
            let dt = w[1].totalTime - w[0].totalTime;
            if dt > 0 {
                total += 1;
                if dt == 5 {
                    fives += 1;
                }
            }
        }
        assert!(fives as f64 / total as f64 > 0.6, "5s 占比不足");
        // 10s 窗非空、结构合法
        assert!(!track.speedPerTenSec.is_empty());
        assert_eq!(track.speedPerTenSec.len(), track.stepsPerTenSec.len());
        // 首点 lat/lng 占位 -1.0，coorType gcj02
        assert_eq!(track.locations[0].lat, -1.0);
        assert_eq!(track.locations[0].coorType, "gcj02");
        // 步数为正、步频在合理范围
        assert!(track.totalSteps > 500, "steps={}", track.totalSteps);
    }

    /// 路线颜色固定全绿色：所有点都必须是官方详情页识别为绿色的形态。
    #[test]
    fn all_track_points_are_green() {
        let track = build(
            3300.0,
            1220,
            42,
            (38.9, 121.54),
            1_788_958_186_123,
            &sample_points(),
        );
        let last = track.locations.len() - 1;
        for (i, point) in track.locations.iter().enumerate() {
            let expected_type = if i == 1 {
                5
            } else if i == last {
                6
            } else {
                0
            };
            assert_eq!(point.ptype, expected_type);
            assert_eq!(point.locType, 1);
            assert_eq!(point.state, 1);
            assert_eq!(point.bdG, 1);
        }
    }

    /// 打卡点吸附：轨迹必过点位。
    ///
    /// 吸附半径仍是 40 m，但 40 m 以内的**走不过去**的点不再硬吸（旧实现会造出
    /// 10–100 m/s 的瞬移）。这里断言的是「轨迹确实经过大部分点位附近」，未命中的
    /// 点由跑步页提示「本次可能不计入有效成绩」。
    #[test]
    fn test_point_snapping() {
        let pts = sample_points();
        let track = build(2200.0, 900, 7, (38.9, 121.54), 1_788_958_186_123, &pts);
        let mut near = 0;
        for pl in &pts {
            let min_m = track
                .locations
                .iter()
                .map(|p| {
                    (((p.gLat - pl.0) * MET_PER_DEG_LAT).powi(2)
                        + ((p.gLng - pl.1) * MET_PER_DEG_LNG).powi(2))
                    .sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            if min_m < 40.0 {
                near += 1;
            }
        }
        assert!(
            near >= pts.len() - 2,
            "轨迹应经过多数点位附近（{near}/{}）",
            pts.len()
        );
    }

    /// 10 秒窗均值配速全部落在有效窗口内（判定规则 2'21"-10'00"/km），且总距精确。
    ///
    /// 首窗允许放宽：t=0 的首点是「开始记录还没迈步」，真人记录的首个 10 s 窗
    /// 本来就比匀速慢（真机样本同样如此），因此首窗按 20'00"/km 判。
    ///
    /// 逐窗配速必须按**实际窗宽**换算：总时长不是 10 的整数倍时，末窗就是一个不足
    /// 10 秒的残窗（真机同样如此），把它当成整 10 秒读会显示成一次并不存在的掉速。
    #[test]
    fn test_speeds_within_valid_pace_window() {
        let pts = sample_points();
        let combos = [
            (1050.0, 480i64),
            (1440.0, 661),
            (1920.0, 719),
            (2100.0, 900),
            (3300.0, 1220),
        ];
        for seed in 0..16u64 {
            for &(dist, dur) in &combos {
                let t = build(dist, dur, seed, (38.9, 121.54), 1_788_958_186_123, &pts);
                let spans = crate::track::wire::ten_second_spans(&t);
                for (i, s) in spans.iter().enumerate() {
                    let secs = (s.end_s - s.begin_s).max(1) as f64;
                    let pace = 1000.0 / (s.distance_m / secs); // 秒/km
                                                               // 首尾窗允许放宽：t=0 的首点是「开始记录还没迈步」，末窗是
                                                               // 结尾断崖（收速）所在的窗口，真机记录里这两端都比匀速慢。
                    let ceil = if i == 0 {
                        1200.0
                    } else if i + 1 == spans.len() {
                        750.0
                    } else {
                        600.0
                    };
                    assert!(
                        (141.0..=ceil).contains(&pace),
                        "seed={seed} dist={dist} 窗{i} 配速 {}/km 越界（窗宽 {secs:.0}s）",
                        format_args!("{}:{:02}", pace as i64 / 60, (pace as i64) % 60)
                    );
                }
                assert!(
                    (t.totalDistance / dist - 1.0).abs() < 0.05,
                    "seed={seed} dist={}: {}",
                    dist,
                    t.totalDistance
                );
            }
        }
    }

    /// BD→GCJ 实测向量。
    #[test]
    fn test_bd09_to_gcj02_vector() {
        let (lat, lng) = bd09_to_gcj02(38.901678, 121.540241);
        assert!((lat - 38.8956025774013).abs() < 1e-9, "lat={lat}");
        assert!((lng - 121.5337497718317).abs() < 1e-9, "lng={lng}");
    }

    /// 坐标基准变换：gcj02_to_bd09 是 bd09_to_gcj02 的逆。
    #[test]
    fn test_coord_datum_roundtrip() {
        use super::geom::gcj02_to_bd09;
        let bd = (38.901678, 121.540241);
        let gcj = bd09_to_gcj02(bd.0, bd.1);
        let back = gcj02_to_bd09(gcj.0, gcj.1);
        assert!((back.0 - bd.0).abs() < 1e-6, "lat={} 期望 {}", back.0, bd.0);
        assert!((back.1 - bd.1).abs() < 1e-6, "lng={} 期望 {}", back.1, bd.1);
    }

    /// OBS 对象：10 键、gzip+base64 可解、run_data 27 键点集。
    #[test]
    fn test_obs_object_structure() {
        let pts: Vec<serde_json::Value> = sample_points()
            .iter()
            .enumerate()
            .map(|(i, (la, lo))| {
                serde_json::json!({
                    "lon": lo, "lat": la, "isFixed": 0,
                    "pointName": format!("P{i}"), "glon": lo - 0.006,
                    "glat": la - 0.006,
                })
            })
            .collect();
        let track = build(
            3300.0,
            1220,
            42,
            (38.9, 121.54),
            1_788_958_186_123,
            &sample_points(),
        );
        let obj = build_obs_object(&track, 1320403809, "UUID-TEST", 13056447, &pts);
        let keys: Vec<&str> = obj
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            keys,
            vec![
                "rrid",
                "uuid",
                "uid",
                "run_data",
                "fixed_point_json",
                "segment_json",
                "speed_json",
                "step_freq_json",
                "laps_json",
                "runFaceCheck"
            ]
        );
        // rrid gzip 可解
        let raw = crate::crypto::envelope::b64_decode(obj["rrid"].as_str().unwrap()).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        use std::io::Read;
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        assert_eq!(s, "1320403809");
        // run_data 解包 → 27 键点集
        let raw = crate::crypto::envelope::b64_decode(obj["run_data"].as_str().unwrap()).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        let wrap: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(wrap["useZip"], false);
        let pts: Vec<serde_json::Value> =
            serde_json::from_str(wrap["allLocJson"].as_str().unwrap()).unwrap();
        // 真机点结构只有 27 个字段（实测 3 条记录、1039 个点）。
        // 多发 `accuracy` / `hasAltitude` / `steps` 会被 schema 白名单挑出来。
        assert_eq!(pts[0].as_object().unwrap().len(), 27, "点键数必须 27");
        for k in ["stepDistance", "coorType", "validTime"] {
            assert!(pts[0].get(k).is_some(), "点必须带 {k}");
        }
        // 这三个字段真机没有，我们也不得发。
        for extra in ["accuracy", "hasAltitude", "steps"] {
            assert!(pts[0].get(extra).is_none(), "点结构不应包含 {extra}");
        }
        // `stepDistance` 真机恒为 0（实测 1039/1039 个点）。
        let last_sd = pts.last().unwrap()["stepDistance"].as_f64().unwrap();
        assert!(
            last_sd.abs() < 1e-9,
            "末点 stepDistance 应为 0，实际 {last_sd}"
        );
        // segment_json 是空串 gzip
        let raw =
            crate::crypto::envelope::b64_decode(obj["segment_json"].as_str().unwrap()).unwrap();
        let mut dec = flate2::read::GzDecoder::new(&raw[..]);
        let mut s = String::new();
        dec.read_to_string(&mut s).unwrap();
        assert_eq!(s, "");
        // obs keys 两个
        let ks = obs_keys(&track, 1320403809, "UUID-TEST");
        assert_eq!(ks.len(), 2);
        assert!(ks[0].contains("run_data/"));
        assert!(ks[1].starts_with("run_data/1320/1320403809.json"));
    }
}

#[cfg(test)]
mod altitude_calibration_tests {
    use super::altitude;
    use super::generator::build;

    fn points() -> Vec<(f64, f64)> {
        vec![
            (39.493046, 116.255475),
            (39.493730, 116.255487),
            (39.492786, 116.254498),
            (39.492470, 116.255111),
            (39.493375, 116.254579),
        ]
    }

    /// 场地地形是平的，爬升量应是小量级。
    ///
    /// 用 open-meteo 的 DEM 在场地外接椭圆（160×120 m）上取 100 个点，海拔恒为
    /// **27.00 m、跨度 0.00 m**；周边 1 km 内为 27–34 m。所以这座操场本身没有
    /// 可计爬升的地形起伏，`totalAscent` 完全由传感器噪声贡献。
    ///
    /// 这条断言防的是「为了让爬升量看起来像真实记录而把噪声调大」：曾被调到
    /// σ=1.0、2 km 给出 138 m 爬升，那是照着 bug 数据拟合的结果。
    #[test]
    fn flat_field_keeps_ascent_in_sensor_noise_range() {
        for seed in [1915881480u64, 42, 7777] {
            for dist in [2000.0, 3000.0, 4000.0] {
                let dur = (dist / 1000.0 * 420.0) as i64;
                let track = build(
                    dist,
                    dur,
                    seed,
                    (39.493, 116.255),
                    1_792_916_403_089,
                    &points(),
                );
                let ascent = altitude::total_ascent_of(&track);
                let per_km = ascent / (track.totalDistance / 1000.0);
                assert!(
                    (0.0..=30.0).contains(&per_km),
                    "seed={seed} dist={dist} 爬升 {ascent:.1} m / {per_km:.1} m/km 超出平坦场地合理范围"
                );
            }
        }
    }

    /// 手动海拔覆盖后，爬升量必须由覆盖决定，而不是重算曲线。
    #[test]
    fn manual_altitude_determines_reported_ascent() {
        let mut track = build(
            2000.0,
            840,
            42,
            (39.493, 116.255),
            1_792_916_403_089,
            &points(),
        );

        altitude::override_bd_a(&mut track, 27.0).unwrap();
        assert_eq!(altitude::total_ascent_of(&track), 0.0, "固定海拔必须零爬升");

        altitude::override_bd_a_range(
            &mut track,
            altitude::AltitudeRange {
                min_m: 27.0,
                max_m: 34.0,
            },
        )
        .unwrap();
        let ascent = altitude::total_ascent_of(&track);
        assert!(
            (ascent - 7.0).abs() < 1e-9,
            "区间爬升应等于最高减最低，实际 {ascent}"
        );
    }
}
#[cfg(test)]
mod alt_calibration {
    use super::generator::build;
    use super::tests::sample_points;

    /// 海拔曲线必须落在实测真跑区间内：
    /// 带宽 5.00-11.78 m、中位 |Δ海拔| 0.31-0.48 m。
    /// 同时边界不能同时是整数（那是参数覆盖特征）。
    #[test]
    fn altitude_matches_real_runs() {
        let pts = sample_points();
        for seed in 0..8u64 {
            let tr = build(
                2000.0,
                900,
                seed,
                (39.4930, 116.2550),
                1_788_958_186_123,
                &pts,
            );
            let alt: Vec<f64> = tr.locations.iter().map(|p| p.bdA).collect();
            let mn = alt.iter().cloned().fold(f64::INFINITY, f64::min);
            let mx = alt.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let band = mx - mn;
            let mut d: Vec<f64> = alt.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            d.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let med = d[d.len() / 2];

            assert!(
                (4.0..=14.0).contains(&band),
                "seed={seed} 海拔带 {band:.2} m 偏离真跑区间 5.00-11.78"
            );
            assert!(
                (0.18..=0.60).contains(&med),
                "seed={seed} 中位|Δ海拔| {med:.3} m 偏离真跑 0.31-0.48"
            );
            let near_int = |x: f64| (x - x.round()).abs() <= 0.005;
            assert!(
                !(near_int(mn) && near_int(mx)),
                "seed={seed} 海拔边界 {mn:.2}/{mx:.2} 同时为整数"
            );
        }
    }
}
