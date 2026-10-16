use eframe::egui::{self, Margin, RichText, Stroke};

use crate::bench_ui::BenchUi;
use crate::info_ui::InfoUi;
use crate::theme::{self, Icon};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Benchmark,
    Drives,
}

pub struct DiskToolsApp {
    tab: Tab,
    bench: BenchUi,
    info: InfoUi,
    elevated: bool,
}

impl DiskToolsApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let elevated = crate::sys::is_elevated();
        // `--tab drives` opens straight on the Drives tab.
        let args: Vec<String> = std::env::args().collect();
        let tab = match args.windows(2).find(|w| w[0] == "--tab").map(|w| w[1].as_str()) {
            Some("drives" | "info") => Tab::Drives,
            _ => Tab::Benchmark,
        };
        Self { tab, bench: BenchUi::new(), info: InfoUi::new(&cc.egui_ctx), elevated }
    }
}

impl eframe::App for DiskToolsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = theme::palette(ui);

        egui::Panel::top("topbar")
            .frame(
                egui::Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(16, 10))
                    .stroke(Stroke::new(1.0, p.border)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(theme::icon(Icon::Gauge)).size(17.0).color(p.accent));
                    ui.label(RichText::new("DiskTools").font(theme::semibold(15.0)).color(p.text));
                    ui.add_space(12.0);
                    theme::segmented(
                        ui,
                        &mut self.tab,
                        &[
                            (Tab::Benchmark, theme::with_icon(Icon::Gauge, "Benchmark")),
                            (Tab::Drives, theme::with_icon(Icon::HardDrive, "Drives")),
                        ],
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let dark = ui.visuals().dark_mode;
                        let (glyph, next, tip) = if dark {
                            (Icon::Sun, egui::ThemePreference::Light, "Light theme")
                        } else {
                            (Icon::Moon, egui::ThemePreference::Dark, "Dark theme")
                        };
                        let toggle = egui::Button::new(RichText::new(theme::icon(glyph)).size(16.0).color(p.muted))
                            .frame(false)
                            .min_size(egui::vec2(28.0, 28.0));
                        if ui.add(toggle).on_hover_text(tip).clicked() {
                            ui.ctx().set_theme(next);
                        }
                        if self.elevated {
                            ui.label(
                                RichText::new(theme::with_icon(Icon::ShieldCheck, "Administrator"))
                                    .size(12.0)
                                    .color(p.muted),
                            );
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.bg).inner_margin(Margin::same(16)))
            .show(ui, |ui| match self.tab {
                Tab::Benchmark => self.bench.ui(ui),
                Tab::Drives => self.info.ui(ui, self.elevated),
            });
    }
}
