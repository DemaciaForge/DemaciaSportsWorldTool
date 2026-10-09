//! 固定在主界面上的开源署名与防倒卖水印。

use eframe::egui;

use super::theme;

const REPOSITORY_URL: &str = "https://github.com/YanamiNeko/NekoSportsWorldTool";
const CONTRIBUTORS_URL: &str =
    "https://github.com/YanamiNeko/NekoSportsWorldTool/graphs/contributors";

/// 在状态栏上方绘制固定水印，避免影响页面内容和输入控件。
pub fn draw(ctx: &egui::Context) {
    egui::Area::new(egui::Id::new("anti_resale_watermark"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -42.0])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(235, 241, 238, 218))
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    egui::Color32::from_rgba_unmultiplied(13, 148, 136, 110),
                ))
                .rounding(egui::Rounding::same(6.0))
                .inner_margin(egui::Margin::same(7.0))
                .show(ui, |ui| {
                    ui.set_max_width(320.0);
                    ui.label(
                        egui::RichText::new("本项目完全开源 · 完全免费")
                            .small()
                            .strong()
                            .color(theme::text_dim()),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "主要贡献者（公开记录）：YanamiNeko、laotierocket、hb-degithub、Yunlk、DemaciaForge（德玛西亚）",
                            )
                            .small()
                            .color(theme::text_dim()),
                        )
                        .wrap(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("如果您是付费的，请向售卖者提出退款")
                                .small()
                                .color(theme::warn()),
                        )
                        .wrap(),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "GitHub 开源项目：YanamiNeko/NekoSportsWorldTool",
                            )
                            .small()
                            .color(theme::text_dim()),
                        )
                        .wrap(),
                    );
                    ui.hyperlink_to(
                        egui::RichText::new("打开项目主页")
                            .small()
                            .color(theme::text_dim()),
                        REPOSITORY_URL,
                    );
                    ui.hyperlink_to(
                        egui::RichText::new("查看公开贡献者记录")
                            .small()
                            .color(theme::text_dim()),
                        CONTRIBUTORS_URL,
                    );
                });
        });
}
