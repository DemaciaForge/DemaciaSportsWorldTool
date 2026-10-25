//! 跑步页：恒 sportType=1；距离/配速范围，起跑时刻恒为「现在 − 用时」。
//!
//! 不再提供开始时间控件：提交的永远是刚刚跑完的那一段。14:40 提交一次 23 分钟的
//! 跑步，记录的就是 14:17–14:40。把记录排到过去的任意时刻会让"刚跑完"这件事本身
//! 站不住。

use super::map::MapState;
use super::{mobile, theme, App};
use crate::track::generate_road::RouteMode;
use chrono::{Local, TimeZone};
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
    /// 路线算法模式
    pub route_mode: RouteMode,
    /// 真实道路模式随机选中的普通打卡点数量（运行时可调）
    pub ordinary_pick: i64,
    /// Standard athletics-track preset; Auto preserves the selected route mode.
    pub track_spec: crate::track::stadium::TrackSpec,
    /// Custom oval measurement-line length, kept as text while editing.
    pub custom_track_length: String,
    /// 详情页路线颜色模式
    pub track_color_mode: crate::api::model::TrackColorMode,
    /// 地图视图状态
    pub map: MapState,
    /// 路网预览（真实道路路由）
    pub preview: Option<crate::track::generate_road::RoadPlan>,
    /// 预览是否过期（参数变更后置真）
    pub preview_stale: bool,
    /// 地图视野是否已适配
    pub map_fitted: bool,
    /// 自定义路线：起点（地址或坐标）
    pub custom_origin: String,
    /// 自定义路线：终点（地址或坐标）
    pub custom_destination: String,
    /// 自定义路线：绕圈（A→B→A 闭环）
    pub custom_loop: bool,
    /// 自定义路线：圈数（0 = 按目标距离自动）
    pub custom_laps: i64,
    /// 高德 Web 服务 Key（留空用 config.amap_key）
    pub amap_key: String,
    /// 自定义路线预览（高德解析结果）
    pub custom_preview: Option<crate::track::custom::CustomRing>,
    /// 自定义路线预览状态文本
    pub custom_status: String,
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
        let prev_route = self.run_page.route_mode;
        let mut custom_fetch_wanted = false;
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
            // 起跑时刻恒为「现在 − 用时」，提交的永远是刚刚跑完的那一段，
            // 因此页面上不再提供开始时间控件。

            mobile::row(ui, |ui| {
                ui.label("人脸校验标记：");
                ui.checkbox(&mut page.face_check, "faceCheck=1");
            });
            ui.horizontal(|ui| {
                ui.label("路线算法：");
                ui.selectable_value(&mut page.route_mode, RouteMode::Legacy, "经典打卡点环");
                ui.selectable_value(&mut page.route_mode, RouteMode::Road, "真实道路路由");
                ui.selectable_value(&mut page.route_mode, RouteMode::Custom, "自定义路线");
                if page.route_mode == RouteMode::Road && self.network.is_none() {
                    ui.colored_label(theme::warn(), "（需先在「路网」页导入 OSM）");
                }
            });
            if page.route_mode == RouteMode::Road {
                ui.horizontal(|ui| {
                    ui.label("随机普通点：");
                    let before = page.ordinary_pick;
                    mobile::drag_i64(
                        ui,
                        "run_ordinary_pick",
                        &mut page.ordinary_pick,
                        0..=10,
                        1.0,
                        "",
                        " 个",
                    );
                    if page.ordinary_pick != before {
                        page.preview_stale = true;
                    }
                });
            }
            if page.route_mode == RouteMode::Custom {
                mobile::row(ui, |ui| {
                    ui.label("高德 Key：");
                    mobile::text_edit(
                        ui,
                        "run_amap_key",
                        &mut page.amap_key,
                        crate::platform::InputKind::Text,
                        200.0,
                    );
                    ui.label("（高德开放平台 Web 服务）");
                });
                mobile::row(ui, |ui| {
                    ui.label("起点：");
                    mobile::text_edit(
                        ui,
                        "run_custom_origin",
                        &mut page.custom_origin,
                        crate::platform::InputKind::Text,
                        150.0,
                    );
                });
                mobile::row(ui, |ui| {
                    ui.label("终点：");
                    mobile::text_edit(
                        ui,
                        "run_custom_destination",
                        &mut page.custom_destination,
                        crate::platform::InputKind::Text,
                        150.0,
                    );
                    ui.label("地址或 经度,纬度");
                });
                mobile::row(ui, |ui| {
                    ui.checkbox(&mut page.custom_loop, "绕圈跑（去程+返程闭环）");
                    ui.label("圈数：");
                    mobile::drag_i64(
                        ui,
                        "run_custom_laps",
                        &mut page.custom_laps,
                        0..=99,
                        1.0,
                        "",
                        "（0=按距离自动）",
                    );
                });
                mobile::row(ui, |ui| {
                    let busy = page.custom_status.starts_with("获取中");
                    let btn = if busy {
                        theme::primary_btn("获取中…")
                    } else {
                        theme::primary_btn("获取路线")
                    };
                    if ui.add_enabled(!busy, btn).clicked() {
                        custom_fetch_wanted = true;
                    }
                    ui.label(&page.custom_status);
                });
            }
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
            ui.horizontal(|ui| {
                ui.label("路线颜色：");
                ui.radio_value(
                    &mut page.track_color_mode,
                    crate::api::model::TrackColorMode::FullGreen,
                    "全绿色",
                );
                ui.radio_value(
                    &mut page.track_color_mode,
                    crate::api::model::TrackColorMode::HalfGreenGray,
                    "灰绿混合",
                );
                ui.label("（全绿色可避免详情页出现灰色分段）");
            });
        }
        if custom_fetch_wanted {
            self.refresh_custom_route();
        }

        // 切换到真实道路路由时按需拉取电子围栏与实时点位（经典模式不触发这些端点）。
        if self.run_page.route_mode == RouteMode::Road && prev_route != RouteMode::Road {
            self.refresh_fence();
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

        self.draw_run_map(ui);
        self.draw_custom_map(ui);

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

    fn draw_run_map(&mut self, ui: &mut egui::Ui) {
        if self.run_page.route_mode != RouteMode::Road {
            return;
        }
        let Some(net) = self.network.clone() else {
            ui.colored_label(
                theme::warn(),
                "未加载 OSM 路网，无法预览（请先到「路网」页导入）",
            );
            return;
        };
        let Some(plan) = self.run_page.plan.clone() else {
            return;
        };

        if self.run_page.preview_stale {
            self.run_page.preview = None;
            self.run_page.map_fitted = false;
            self.run_page.preview_stale = false;
            // 点位缓存按锚点隔离（main 的串城市防护）；锚点无效时视为无缓存
            let cached = self
                .identity
                .anchor_coordinate()
                .ok()
                .and_then(crate::api::model::load_points_cache_for);
            if let Some((_ts, pts)) = cached {
                let fixed = crate::api::points::points_fixed(&pts);
                let ordinary = crate::api::points::points_free(&pts);
                if !fixed.is_empty() || !ordinary.is_empty() {
                    let fences = crate::api::model::load_fence_cache().unwrap_or_default();
                    match crate::track::generate_road::plan_road_view(
                        &net,
                        &fixed,
                        &ordinary,
                        plan.dist * 1000.0,
                        plan.seed,
                        &fences,
                        self.run_page.ordinary_pick.max(0) as usize,
                    ) {
                        Ok(p) => self.run_page.preview = Some(p),
                        Err(e) => self.status = format!("路线预览失败：{e}"),
                    }
                } else {
                    self.status = "无打卡点缓存，提交后可回显轨迹".into();
                }
            } else {
                self.status = "无打卡点缓存，提交后可回显轨迹".into();
            }
        }

        if self.run_page.preview.is_some() {
            super::map::legend(
                ui,
                &[
                    ("道路", egui::Color32::from_rgb(200, 208, 204)),
                    ("建筑", egui::Color32::from_rgb(224, 194, 170)),
                    ("路线", egui::Color32::from_rgb(30, 111, 216)),
                    ("打卡点", egui::Color32::from_rgb(240, 180, 0)),
                    ("电子围栏", egui::Color32::from_rgb(34, 197, 94)),
                    ("起点", egui::Color32::from_rgb(22, 160, 90)),
                    ("终点", egui::Color32::from_rgb(220, 38, 38)),
                ],
            );
        }

        ui.add_space(4.0);
        let rect = ui.available_rect_before_wrap();
        let (response, painter) = ui.allocate_painter(
            egui::Vec2::new(rect.width().max(200.0), 240.0),
            egui::Sense::drag(),
        );
        let canvas = response.rect;
        painter.rect_filled(
            canvas,
            egui::Rounding::ZERO,
            egui::Color32::from_rgb(250, 252, 251),
        );

        // 首次预览后适配视野：优先以电子围栏为中点/范围，无围栏时退化为路线+打卡点+道路
        if !self.run_page.map_fitted {
            let mut bounds: Vec<(f64, f64)> = Vec::new();
            if let Some(p) = &self.run_page.preview {
                for f in &p.fences {
                    bounds.extend(f.iter().copied());
                }
                if bounds.is_empty() {
                    bounds = super::map::collect_bounds(&p.edges);
                    bounds.extend(p.route.iter().copied());
                    bounds.extend(p.checkpoints.iter().copied());
                }
            }
            if !bounds.is_empty() {
                self.run_page.map.fit(canvas, &bounds);
                self.run_page.map_fitted = true;
            }
        }

        if let Some(p) = &self.run_page.preview {
            let road = egui::Color32::from_rgb(200, 208, 204);
            for e in &p.edges {
                self.run_page
                    .map
                    .draw_polyline(&painter, canvas, e, road, 1.0);
            }
            let bld = egui::Color32::from_rgb(224, 194, 170);
            for b in &p.buildings {
                self.run_page
                    .map
                    .draw_polygon(&painter, canvas, b, bld, 1.0);
            }
            let fence_c = egui::Color32::from_rgb(34, 197, 94);
            for f in &p.fences {
                self.run_page
                    .map
                    .draw_polygon(&painter, canvas, f, fence_c, 2.0);
            }
            let route_c = egui::Color32::from_rgb(30, 111, 216);
            self.run_page
                .map
                .draw_polyline(&painter, canvas, &p.route, route_c, 2.5);
            let cp = egui::Color32::from_rgb(240, 180, 0);
            for &(la, lo) in &p.checkpoints {
                self.run_page
                    .map
                    .draw_point(&painter, canvas, la, lo, cp, 3.5);
            }
            if let Some(first) = p.route.first() {
                self.run_page.map.draw_point(
                    &painter,
                    canvas,
                    first.0,
                    first.1,
                    egui::Color32::from_rgb(22, 160, 90),
                    4.5,
                );
            }
            if let Some(last) = p.route.last() {
                self.run_page.map.draw_point(
                    &painter,
                    canvas,
                    last.0,
                    last.1,
                    egui::Color32::from_rgb(220, 38, 38),
                    4.5,
                );
            }
            let label = if p.length_m > 0.0 {
                let loops = plan.dist * 1000.0 / p.length_m;
                if loops > 1.15 {
                    format!(
                        "本次方案 {:.2} km · 单圈 {:.0} m × {:.1} 圈 · {} 打卡点",
                        plan.dist,
                        p.length_m,
                        loops,
                        p.checkpoints.len()
                    )
                } else {
                    format!(
                        "本次方案 {:.2} km · 路线 {:.0} m · {} 打卡点",
                        plan.dist,
                        p.length_m,
                        p.checkpoints.len()
                    )
                }
            } else {
                format!(
                    "本次方案 {:.2} km · {} 打卡点",
                    plan.dist,
                    p.checkpoints.len()
                )
            };
            painter.text(
                egui::Pos2::new(canvas.left() + 8.0, canvas.top() + 8.0),
                egui::Align2::LEFT_TOP,
                label,
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(88, 104, 99),
            );
        } else {
            painter.text(
                canvas.center(),
                egui::Align2::CENTER_CENTER,
                "（无预览）",
                egui::FontId::proportional(14.0),
                egui::Color32::from_rgb(150, 160, 156),
            );
        }

        self.run_page.map.interact(ui, canvas);
    }

    /// 自定义路线预览：绘制高德闭环路线与打卡点（无 OSM 路网底图）。
    fn draw_custom_map(&mut self, ui: &mut egui::Ui) {
        if self.run_page.route_mode != RouteMode::Custom {
            return;
        }
        let Some(ring) = self.run_page.custom_preview.clone() else {
            return;
        };
        ui.add_space(4.0);
        super::map::legend(
            ui,
            &[
                ("路线", egui::Color32::from_rgb(30, 111, 216)),
                ("打卡点", egui::Color32::from_rgb(240, 180, 0)),
                ("起点", egui::Color32::from_rgb(22, 160, 90)),
                ("终点", egui::Color32::from_rgb(220, 38, 38)),
            ],
        );
        let rect = ui.available_rect_before_wrap();
        let (response, painter) = ui.allocate_painter(
            egui::Vec2::new(rect.width().max(200.0), 220.0),
            egui::Sense::drag(),
        );
        let canvas = response.rect;
        painter.rect_filled(
            canvas,
            egui::Rounding::ZERO,
            egui::Color32::from_rgb(250, 252, 251),
        );
        if !self.run_page.map_fitted {
            let mut bounds: Vec<(f64, f64)> = ring.points_bd.clone();
            if let Some((_ts, pts)) = self
                .identity
                .anchor_coordinate()
                .ok()
                .and_then(crate::api::model::load_points_cache_for)
            {
                bounds.extend(crate::api::points::points_bd(&pts));
            }
            if !bounds.is_empty() {
                self.run_page.map.fit(canvas, &bounds);
                self.run_page.map_fitted = true;
            }
        }
        let route_c = egui::Color32::from_rgb(30, 111, 216);
        self.run_page
            .map
            .draw_polyline(&painter, canvas, &ring.points_bd, route_c, 2.5);
        if let Some((_ts, pts)) = self
            .identity
            .anchor_coordinate()
            .ok()
            .and_then(crate::api::model::load_points_cache_for)
        {
            let cp = egui::Color32::from_rgb(240, 180, 0);
            for (la, lo) in crate::api::points::points_bd(&pts) {
                self.run_page
                    .map
                    .draw_point(&painter, canvas, la, lo, cp, 3.0);
            }
        }
        if let Some(first) = ring.points_bd.first() {
            self.run_page.map.draw_point(
                &painter,
                canvas,
                first.0,
                first.1,
                egui::Color32::from_rgb(22, 160, 90),
                4.5,
            );
        }
        if let Some(last) = ring.points_bd.last() {
            self.run_page.map.draw_point(
                &painter,
                canvas,
                last.0,
                last.1,
                egui::Color32::from_rgb(220, 38, 38),
                4.5,
            );
        }
        let label = format!(
            "高德路线：去程 {:.0} m · 闭环 {:.0} m · {} 点",
            ring.leg_length_m,
            ring.total_length_m,
            ring.points_bd.len()
        );
        painter.text(
            egui::Pos2::new(canvas.left() + 8.0, canvas.top() + 8.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(12.0),
            egui::Color32::from_rgb(88, 104, 99),
        );
        self.run_page.map.interact(ui, canvas);
    }

    /// 后台获取高德自定义路线（不登录、不访问学校服务器）。
    pub(crate) fn refresh_custom_route(&mut self) {
        let spec = crate::track::custom::CustomRouteSpec {
            origin: self.run_page.custom_origin.trim().to_string(),
            destination: self.run_page.custom_destination.trim().to_string(),
            loop_mode: self.run_page.custom_loop,
            laps: self.run_page.custom_laps,
        };
        if !spec.is_valid() {
            self.run_page.custom_status = "请填写起点与终点".into();
            return;
        }
        let key = if self.run_page.amap_key.trim().is_empty() {
            self.config.amap_key.clone()
        } else {
            self.run_page.amap_key.trim().to_string()
        };
        // 立即持久化，避免仅预览未开跑时重启丢失 key/起终点
        self.config.amap_key = key.clone();
        self.config.custom_origin = spec.origin.clone();
        self.config.custom_destination = spec.destination.clone();
        self.config.custom_loop = spec.loop_mode;
        self.config.custom_laps = spec.laps;
        let _ = crate::api::model::save_config(&self.config);
        self.run_page.custom_status = "获取中…".into();
        self.spawn_job(move |tx| {
            let payload = match crate::amap::fetch_custom_ring(&spec, &key) {
                Ok(ring) => {
                    let msg = format!(
                        "√ 去程 {:.0} m · 闭环 {:.0} m（{}）",
                        ring.leg_length_m,
                        ring.total_length_m,
                        if ring.loop_mode { "绕圈" } else { "单程" }
                    );
                    serde_json::json!({ "ok": true, "ring": ring, "message": msg })
                }
                Err(e) => serde_json::json!({ "ok": false, "message": e }),
            };
            tx.send(format!("{}{}", crate::ui::CUSTOM_ROUTE_DONE, payload))
                .ok();
        });
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
        let (manual_altitude, manual_altitude_range) = match altitude_spec {
            None => (None, None),
            Some(crate::track::altitude::AltitudeSpec::Single(value)) => (Some(value), None),
            Some(crate::track::altitude::AltitudeSpec::Range(range)) => (None, Some(range)),
        };
        let (dist, dur) = (plan.dist * 1000.0, plan.dur); // 米
        let start_ms = plan.start_ms;
        let face_check = if page.face_check { 1 } else { 0 };
        let route_mode = page.route_mode;
        let ordinary_pick = page.ordinary_pick.max(0) as usize;
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
        let track_color_mode = page.track_color_mode;
        let custom = if route_mode == RouteMode::Custom {
            Some(crate::track::custom::CustomRouteSpec {
                origin: page.custom_origin.trim().to_string(),
                destination: page.custom_destination.trim().to_string(),
                loop_mode: page.custom_loop,
                laps: page.custom_laps,
            })
        } else {
            None
        };
        let amap_key = if page.amap_key.trim().is_empty() {
            self.config.amap_key.clone()
        } else {
            page.amap_key.trim().to_string()
        };
        self.config.dist_min = page.dist_min;
        self.config.dist_max = page.dist_max;
        self.config.pace_min = page.pace_min;
        self.config.pace_max = page.pace_max;
        self.config.face_check = page.face_check;
        self.config.manual_altitude = manual_altitude;
        self.config.manual_altitude_range = manual_altitude_range;
        self.config.ordinary_pick = ordinary_pick;
        self.config.route_mode = match route_mode {
            RouteMode::Road => "road".into(),
            RouteMode::Custom => "custom".into(),
            RouteMode::Legacy => "legacy".into(),
        };
        self.config.track_spec = track_spec;
        self.config.track_color_mode = track_color_mode;
        self.config.amap_key = page.amap_key.trim().to_string();
        self.config.custom_origin = page.custom_origin.trim().to_string();
        self.config.custom_destination = page.custom_destination.trim().to_string();
        self.config.custom_loop = page.custom_loop;
        self.config.custom_laps = page.custom_laps;
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
                track_color_mode,
                seed,
                route_mode,
                ordinary_pick,
                track_spec,
                custom,
                amap_key,
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
