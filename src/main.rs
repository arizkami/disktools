#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("disktools talks to Windows storage APIs and currently builds on Windows only");

use eframe::egui;

mod app;
mod bench;
mod bench_ui;
mod diskinfo;
mod info_ui;
mod surface;
mod surface_ui;
mod sys;
mod theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("DiskTools")
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([680.0, 520.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png"))
                    .expect("bundled icon is a valid PNG"),
            ),
        ..Default::default()
    };
    eframe::run_native(
        "DiskTools",
        options,
        Box::new(|cc| Ok(Box::new(app::DiskToolsApp::new(cc)))),
    )
}
