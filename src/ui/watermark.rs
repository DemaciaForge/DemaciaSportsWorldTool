//! 主界面状态栏上方的开源信息水印。

use eframe::egui;

use super::theme;

const REPOSITORY_URL: &str = "https://github.com/YanamiNeko/NekoSportsWorldTool";
const CONTRIBUTORS_URL: &str =
    "https://github.com/YanamiNeko/NekoSportsWorldTool/graphs/contributors";

/// 在状态栏上方绘制水印，底边跟随状态栏实际位置，避免遮挡页面控件。
pub fn draw(ctx: &egui::Context, status_bar_rect: egui::Rect) {
    let screen_rect = ctx.screen_rect();
    let status_bar_offset = -(screen_rect.bottom() - status_bar_rect.top());
    egui::Area::new(egui::Id::new("anti_resale_watermark"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, status_bar_offset])
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
                        egui::RichText::new("本项目免费提供，禁止违反许可证规定进行商业使用。")
                            .small()
                            .strong()
                            .color(theme::text_dim()),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("如果您通过付费渠道获得本软件，请联系售卖者。")
                                .small()
                                .color(theme::warn()),
                        )
                        .wrap(),
                    );
                    ui.horizontal_wrapped(|ui| {
                        ui.hyperlink_to(
                            egui::RichText::new("GitHub 项目主页")
                                .small()
                                .color(theme::text_dim()),
                            REPOSITORY_URL,
                        );
                        ui.label(egui::RichText::new("|").small().color(theme::text_dim()));
                        ui.hyperlink_to(
                            egui::RichText::new("查看贡献者")
                                .small()
                                .color(theme::text_dim()),
                            CONTRIBUTORS_URL,
                        );
                    });
                });
        });
}
