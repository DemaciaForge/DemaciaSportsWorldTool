//! 跑步页：恒 sportType=1；距离/配速范围，起跑时刻恒为「现在 − 用时」。
//!
//! 不再提供开始时间控件：提交的永远是刚刚跑完的那一段。14:40 提交一次 23 分钟的
//! 跑步，记录的就是 14:17–14:40。把记录排到过去的任意时刻会让"刚跑完"这件事本身
//! 站不住。

use super::{mobile, theme, App};
use chrono::TimeZone;
use eframe::egui;

#[derive(Default)]
pub struct RunPage {
    pub dist_min: f32,
    pub dist_max: f32,
    pub pace_min: f32,
    pub pace_max: f32,
    /// 手动绝对海拔（米）；空白表示使用生成器默认海拔。
    pub manual_altitude: String,
    pub face_check: bool,
    /// 预计算方案：参数变更时重抽样，提交直接使用
    pub plan: Option<RunPlan>,

    /// Standard athletics-track preset; Auto preserves the selected route mode.
    pub track_spec: crate::track::stadium::TrackSpec,
    /// Custom oval measurement-line length, kept as text while editing.
    pub custom_track_length: String,
    /// 本地地面基准海拔（米），留空用手指默认值。
    pub altitude_base: String,
    /// 海拔上下震荡幅度（米，半幅），留空用手指默认值。
    pub altitude_osc: String,

    /// 预览是否过期（参数变更后置真）
    pub preview_stale: bool,
}

/// 一次提交的确定方案（进入页面/参数变更时抽样生成）。
#[derive(Debug, Clone)]
pub struct RunPlan {
    pub dist_min: f32,
    pub dist_max: f32,
    pub pace_min: f32,
    pub pace_max: f32,
    /// 公里
    pub dist: f64,
    /// 秒/km
    pub pace: f32,
    /// 秒
    pub dur: i64,
    pub start_ms: i64,
    /// 本次方案的随机种子（预览与提交共用，保证所见即所得）
    pub seed: u64,
}

impl RunPage {
    /// 距离/配速是否与方案一致（不一致需重掷运动参数）。
    fn shape_matches(&self, p: &RunPlan) -> bool {
        p.dist_min == self.dist_min
            && p.dist_max == self.dist_max
            && p.pace_min == self.pace_min
            && p.pace_max == self.pace_max
    }

    /// 按当前参数抽样运动量（距离 / 配速 / 用时）。
    fn sample_shape(&self) -> (f64, f32, i64) {
        let (lo, hi) = (
            self.dist_min.min(self.dist_max),
            self.dist_min.max(self.dist_max),
        );
        let (plo, phi) = (
            self.pace_min.min(self.pace_max),
            self.pace_min.max(self.pace_max),
        );
        let pace = plo + (phi - plo) * rand::random::<f32>();
        let dist = (lo + (hi - lo) * rand::random::<f32>()) as f64;
        let dur = (dist * pace as f64).round() as i64;
        (dist, pace, dur)
    }

    /// 起跑时刻：恒为「现在 − 用时」。
    ///
    /// 提交的永远是刚刚跑完的那一段：14:40 提交一次 23 分钟的跑步，记录的就是
    /// 14:17–14:40。不再支持把记录排到过去的任意时刻。
    fn start_ms_for(dur: i64) -> i64 {
        crate::crypto::envelope::now_ms() - dur.max(1) * 1000
    }

    /// 运动参数变更时重抽距离/配速/用时，起跑时刻随之重算。
    fn regen_shape(&mut self) {
        let (dist, pace, dur) = self.sample_shape();
        let start_ms = Self::start_ms_for(dur);
        self.write_plan(dist, pace, dur, start_ms);
    }

    /// 把当前参数与已算好的量写成方案。
    fn write_plan(&mut self, dist: f64, pace: f32, dur: i64, start_ms: i64) {
        self.plan = Some(RunPlan {
            dist_min: self.dist_min,
            dist_max: self.dist_max,
            pace_min: self.pace_min,
            pace_max: self.pace_max,
            dist,
            pace,
            dur,
            start_ms,
            seed: rand::random::<u64>(),
        });
        self.preview_stale = true;
    }

    /// 「换一版」：重掷运动量，起跑时刻随新用时重算。
    pub fn regen_plan(&mut self) {
        let (dist, pace, dur) = self.sample_shape();
        let start_ms = Self::start_ms_for(dur);
        self.write_plan(dist, pace, dur, start_ms);
    }

    /// 确保方案与当前参数一致（参数变更补齐 / 提交前兜底）。
    pub fn ensure_plan(&mut self) {
        let shape_ok = self.plan.as_ref().map(|p| self.shape_matches(p)) == Some(true);
        if !shape_ok {
            self.regen_shape();
        }
    }
}

impl App {
    pub fn draw_run(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.draw_run_content(ui);
            });
    }

    fn draw_run_content(&mut self, ui: &mut egui::Ui) {
        {
            let page = &mut self.run_page;
            let compact = mobile::compact_ui(ui);
            let mut draw_distance_inputs = |ui: &mut egui::Ui| {
                mobile::drag_f32(
                    ui,
                    "run_dist_min",
                    &mut page.dist_min,
                    0.5..=20.0,
                    0.05,
                    2,
                    " km",
                );
                ui.label("至");
                mobile::drag_f32(
                    ui,
                    "run_dist_max",
                    &mut page.dist_max,
                    0.5..=20.0,
                    0.05,
                    2,
                    " km",
                );
            };
            if compact {
                ui.label("距离范围（km）：");
                ui.horizontal(draw_distance_inputs);
            } else {
                ui.horizontal(|ui| {
                    ui.label("距离范围（km）：");
                    draw_distance_inputs(ui);
                });
            }
            let mut draw_pace_inputs = |ui: &mut egui::Ui| {
                mobile::drag_f32(
                    ui,
                    "run_pace_min",
                    &mut page.pace_min,
                    180.0..=520.0,
                    5.0,
                    0,
                    "",
                );
                ui.label("至");
                mobile::drag_f32(
                    ui,
                    "run_pace_max",
                    &mut page.pace_max,
                    180.0..=520.0,
                    5.0,
                    0,
                    "",
                );
            };
            if compact {
                ui.label("配速范围（秒/km）：");
                ui.horizontal(draw_pace_inputs);
            } else {
                ui.horizontal(|ui| {
                    ui.label("配速范围（秒/km）：");
                    draw_pace_inputs(ui);
                });
            }
            mobile::row(ui, |ui| {
                ui.label("手动海拔（米/区间）：");
                mobile::text_edit(
                    ui,
                    "run_manual_altitude",
                    &mut page.manual_altitude,
                    crate::platform::InputKind::Text,
                    120.0,
                );
                ui.label("留空自动；可填 17.2 或 11.6-22.8");
            });
            mobile::row(ui, |ui| {
                ui.label("基准海拔（米）：");
                mobile::text_edit(
                    ui,
                    "run_altitude_base",
                    &mut page.altitude_base,
                    crate::platform::InputKind::Text,
                    80.0,
                );
                ui.label("本地地面常年读数，留空用默认");
            });
            mobile::row(ui, |ui| {
                ui.label("海拔震荡（米）：");
                mobile::text_edit(
                    ui,
                    "run_altitude_osc",
                    &mut page.altitude_osc,
                    crate::platform::InputKind::Text,
                    80.0,
                );
                ui.label("上下摆动的半幅，例如 ±3 填 3");
            });
            // 起跑时刻恒为「现在 − 用时」，提交的永远是刚刚跑完的那一段，
            // 因此页面上不再提供开始时间控件。

            mobile::row(ui, |ui| {
                ui.label("人脸校验标记：");
                ui.checkbox(
                    &mut page.face_check,
                    "强制报 faceCheck=1（一般不用勾，留空即跟随学校配置）",
                );
            });

            mobile::row(ui, |ui| {
                ui.label("操场规格：");
                egui::ComboBox::from_id_salt("run_track_spec")
                    .width(180.0)
                    .selected_text(match page.track_spec {
                        crate::track::stadium::TrackSpec::Custom { total_m } => {
                            format!("自定义 {total_m} 米")
                        }
                        spec => spec.label().to_string(),
                    })
                    .show_ui(ui, |ui| {
                        for spec in [
                            crate::track::stadium::TrackSpec::Auto,
                            crate::track::stadium::TrackSpec::M200,
                            crate::track::stadium::TrackSpec::M300,
                            crate::track::stadium::TrackSpec::M400,
                        ] {
                            ui.selectable_value(&mut page.track_spec, spec, spec.label());
                        }
                        let custom_length = page
                            .custom_track_length
                            .parse::<u32>()
                            .ok()
                            .filter(|length| (100..=1000).contains(length))
                            .unwrap_or(400);
                        ui.selectable_value(
                            &mut page.track_spec,
                            crate::track::stadium::TrackSpec::Custom {
                                total_m: custom_length,
                            },
                            format!("自定义 {custom_length} 米"),
                        );
                    });
                if matches!(
                    page.track_spec,
                    crate::track::stadium::TrackSpec::Custom { .. }
                ) {
                    ui.label("长度");
                    mobile::text_edit(
                        ui,
                        "run_custom_track_length",
                        &mut page.custom_track_length,
                        crate::platform::InputKind::Text,
                        70.0,
                    );
                    ui.label("100-1000 米");
                }
            });
        }

        // 进入跑步页拉一次围栏与点位：围栏用于把跑道直道摆正到场地长边方向，
        // 点位用于拟合打卡点环与预览。
        //
        // 注意 `draw_run_content` 每帧都会执行，这里必须用标记挡住，
        // 否则会以帧率拉点位（日志刷屏 + 打满点位接口的限流额度）。
        if !self.run_points_requested {
            self.run_points_requested = true;
            self.refresh_points();
        }

        ui.add_space(8.0);
        let fmt_pace = |s: f32| format!("{}:{:02}", (s / 60.0) as i64, (s as i64) % 60);
        let fmt_dur = |s: i64| {
            if s >= 3600 {
                format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
            } else {
                format!("{}:{:02}", s / 60, s % 60)
            }
        };
        // 参数变更时补方案；显示本次提交的确定方案
        self.run_page.ensure_plan();
        self.draw_run_warnings(ui);
        let plan_label = match &self.run_page.plan {
            Some(p) => {
                let start = chrono::Local
                    .timestamp_millis_opt(p.start_ms)
                    .single()
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_default();
                let (dist, pace, dur) = (p.dist, p.pace, p.dur);
                format!(
                    "本次方案：距离 {dist:.2} km · 配速 {}/km · 用时 {} · 开始 {start}",
                    fmt_pace(pace),
                    fmt_dur(dur)
                )
            }
            None => String::new(),
        };
        mobile::row(ui, |ui| {
            ui.colored_label(theme::plain(), plan_label);
            if ui.small_button("换一版").clicked() {
                self.run_page.regen_plan();
            }
        });

        ui.add_space(8.0);
        let enabled = !self.run_busy && self.session.is_some();
        let btn = if self.run_busy {
            theme::primary_btn("提交中…")
        } else {
            theme::primary_btn("开始跑步")
        };
        mobile::row(ui, |ui| {
            if ui.add_enabled(enabled, btn).clicked() {
                self.start_run();
            }
            if self.session.is_none() {
                ui.label("（请先登录）");
            }
        });
    }

    /// 方案提示。须在 `ensure_plan()` 之后调用，才对得上本次提交时刻。
    ///
    /// 起跑时刻恒为「现在 − 用时」，没有可选的日期/时刻，因此不再需要时间相关提示。
    fn draw_run_warnings(&self, ui: &mut egui::Ui) {
        let _ = ui;
    }

    fn start_run(&mut self) {
        // 参数变更时补齐方案；提交直接使用预计算值
        self.run_page.ensure_plan();
        let page = &mut self.run_page;
        let plan = match page.plan.clone() {
            Some(p) => p,
            None => return,
        };
        let altitude_spec = match crate::track::altitude::parse_spec(&page.manual_altitude) {
            Ok(spec) => spec,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let defaults = crate::track::altitude::AltitudeProfile::default();
        let parse_alt_field = |text: &str, fallback: f64| -> Option<f64> {
            let t = text.trim();
            if t.is_empty() {
                Some(fallback)
            } else {
                t.parse::<f64>().ok()
            }
        };
        let (Some(base_m), Some(oscillation_m)) = (
            parse_alt_field(&page.altitude_base, defaults.base_m),
            parse_alt_field(&page.altitude_osc, defaults.oscillation_m),
        ) else {
            self.status = "基准海拔与海拔震荡必须是数字（或留空用默认）".into();
            return;
        };
        let candidate = crate::track::altitude::AltitudeProfile {
            base_m,
            oscillation_m,
        };
        let altitude_profile = match candidate.validate() {
            Ok(p) => p,
            Err(e) => {
                self.status = e;
                return;
            }
        };

        let (manual_altitude, manual_altitude_range) = match altitude_spec {
            None => (None, None),
            Some(crate::track::altitude::AltitudeSpec::Single(value)) => (Some(value), None),
            Some(crate::track::altitude::AltitudeSpec::Range(range)) => (None, Some(range)),
        };
        let (dist, dur) = (plan.dist * 1000.0, plan.dur); // 米
        let start_ms = plan.start_ms;
        // 不勾选 -> None，跟随学校策略里的 faceVerify；勾选才显式报 1。
        // 这样换学校不用改代码：开了人脸的学校会自动报 1，没开的报 0。
        let face_check = page.face_check.then_some(1);

        let track_spec = match page.track_spec {
            crate::track::stadium::TrackSpec::Custom { .. } => {
                let Some(total_m) = page
                    .custom_track_length
                    .parse::<u32>()
                    .ok()
                    .filter(|length| (100..=1000).contains(length))
                else {
                    self.status = "自定义操场长度必须是 100-1000 米的整数".into();
                    return;
                };
                crate::track::stadium::TrackSpec::Custom { total_m }
            }
            spec => spec,
        };

        self.config.dist_min = page.dist_min;
        self.config.dist_max = page.dist_max;
        self.config.pace_min = page.pace_min;
        self.config.pace_max = page.pace_max;
        self.config.face_check = page.face_check;
        self.config.manual_altitude = manual_altitude;
        self.config.manual_altitude_range = manual_altitude_range;
        self.config.track_spec = track_spec;
        self.config.altitude_base_m = Some(altitude_profile.base_m);
        self.config.altitude_oscillation_m = Some(altitude_profile.oscillation_m);
        let _ = crate::api::model::save_config(&self.config);

        let identity = self.identity.clone();
        let session = match self.session.clone() {
            Some(s) => s,
            None => {
                self.status = "请先登录".into();
                return;
            }
        };
        self.run_busy = true;
        self.status = "跑步提交中…".into();
        let seed = plan.seed;
        self.spawn_job(move |tx| {
            let mut log = App::logger(tx.clone());
            let mut client = crate::api::client::ApiClient::new(identity, Some(session));
            let params = crate::api::flow::RunParams {
                dist,
                dur,
                start_ms,
                face_check,
                manual_altitude,
                manual_altitude_range,
                seed,
                track_spec,
                altitude_profile,
            };
            let payload = match crate::api::flow::run_full_flow(&mut client, &params, &mut log) {
                Ok(out) => {
                    log(&format!(
                        "全链完成 rrid={} obs={}/2 verify={} uuid={}",
                        out.result.rrid, out.obs_ok, out.detail_ok, out.result.uuid
                    ));
                    serde_json::json!({
                        "ok": true, "rrid": out.result.rrid,
                        "obs_ok": out.obs_ok, "verify": out.detail_ok,
                        "uuid": out.result.uuid,
                        "dist": out.result.total_dis, "dur": out.result.total_time,
                        "steps": out.result.total_steps, "avg_step_freq": out.result.avg_step_freq,
                        "calorie": out.result.calorie, "avg_power": out.result.avg_power,
                        "sel_distance": out.result.sel_distance, "start": out.result.start_ms,
                    })
                }
                Err(e) => {
                    log(&format!("跑步提交失败: {e}"));
                    serde_json::json!({ "ok": false, "message": e })
                }
            };
            tx.send(format!("__RUN_DONE__{payload}")).ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份参数合理的页面状态（配速约 6:00/km，距离 2 km 附近）。
    fn page() -> RunPage {
        RunPage {
            dist_min: 2.0,
            dist_max: 2.2,
            pace_min: 350.0,
            pace_max: 370.0,
            ..Default::default()
        }
    }

    /// 核心不变量：起跑时刻恒为「现在 − 用时」，即终点落在当前时刻。
    ///
    /// 14:40 提交一次 23 分钟的跑步 → 记录 14:17–14:40。这条替换掉原来
    /// 「可以把记录排到过去任意时刻」的整套行为。
    #[test]
    fn start_is_always_now_minus_duration() {
        for (dist_lo, dist_hi, pace_lo, pace_hi) in [
            (2.0f32, 2.2f32, 350.0f32, 370.0f32),
            (4.0, 4.2, 400.0, 430.0),
        ] {
            let mut p = page();
            p.dist_min = dist_lo;
            p.dist_max = dist_hi;
            p.pace_min = pace_lo;
            p.pace_max = pace_hi;
            p.ensure_plan();
            let plan = p.plan.clone().unwrap();

            let before = crate::crypto::envelope::now_ms();
            let end = plan.start_ms + plan.dur * 1000;
            // start_ms 在 ensure_plan 那一刻算出，这里读的 now 必然略晚，
            // 所以允许很小的余量，但绝不能落到明显过去或未来。
            assert!(
                end <= before,
                "终点 {end} 落到了未来（现在 {before}），用时 {}s",
                plan.dur
            );
            assert!(
                before - end < 2_000,
                "终点比现在早了 {}ms，说明起跑时刻没有锚定当前时刻",
                before - end
            );
            assert!(plan.dur > 0, "用时必须为正");
        }
    }

    /// 距离/配速变化时起跑时刻跟着重算，终点仍在当前时刻附近。
    #[test]
    fn shape_change_moves_start_with_duration() {
        let mut p = page();
        p.ensure_plan();
        let first = p.plan.clone().unwrap();

        p.dist_min = 5.0;
        p.dist_max = 5.2;
        p.regen_shape();
        let second = p.plan.clone().unwrap();

        assert!(second.dur > first.dur, "距离变大后用时应当变长");
        let end = second.start_ms + second.dur * 1000;
        let now = crate::crypto::envelope::now_ms();
        assert!(
            end <= now && now - end < 2_000,
            "换参数后终点应仍贴近当前时刻"
        );
    }

    /// 「换一版」重掷运动量，但起跑时刻仍锚定当前时刻。
    #[test]
    fn regen_plan_keeps_now_anchor() {
        let mut p = page();
        p.regen_plan();
        for _ in 0..5 {
            p.regen_plan();
            let plan = p.plan.clone().unwrap();
            let now = crate::crypto::envelope::now_ms();
            let end = plan.start_ms + plan.dur * 1000;
            assert!(
                end <= now && now - end < 2_000,
                "换一版后终点应仍贴近当前时刻"
            );
        }
    }

    /// 反复调用 ensure_plan 应当是幂等的（不改动任何已定值）。
    #[test]
    fn ensure_plan_is_idempotent() {
        let mut p = page();
        p.regen_plan();
        let first = p.plan.clone().unwrap();
        for _ in 0..5 {
            p.ensure_plan();
        }
        let again = p.plan.clone().unwrap();
        assert_eq!(
            (again.dist, again.pace, again.dur, again.start_ms),
            (first.dist, first.pace, first.dur, first.start_ms),
            "ensure_plan 不稳定"
        );
    }
}
