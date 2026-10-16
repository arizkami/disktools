use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

use crate::bench::{self, BenchConfig, BenchHandle, MIB, Mode, Profile, Score, TestSpec};
use crate::diskinfo::fmt_bytes;
use crate::sys::{self, Volume};
use crate::theme::{self, Icon, Palette};

const SIZES: [u64; 13] = [
    16 * MIB,
    32 * MIB,
    64 * MIB,
    128 * MIB,
    256 * MIB,
    512 * MIB,
    1024 * MIB,
    2048 * MIB,
    4096 * MIB,
    8192 * MIB,
    16384 * MIB,
    32768 * MIB,
    65536 * MIB,
];

fn size_label(bytes: u64) -> String {
    if bytes >= 1024 * MIB {
        format!("{} GiB", bytes / (1024 * MIB))
    } else {
        format!("{} MiB", bytes / MIB)
    }
}

fn mode_label(m: Mode) -> &'static str {
    match m {
        Mode::ReadWrite => "Read + Write",
        Mode::ReadOnly => "Read only",
        Mode::WriteOnly => "Write only",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    MBps,
    GBps,
    Iops,
    Latency,
}

impl Unit {
    const ALL: [Unit; 4] = [Unit::MBps, Unit::GBps, Unit::Iops, Unit::Latency];

    fn label(self) -> &'static str {
        match self {
            Unit::MBps => "MB/s",
            Unit::GBps => "GB/s",
            Unit::Iops => "IOPS",
            Unit::Latency => "μs",
        }
    }

    fn format(self, s: &Score) -> String {
        match self {
            Unit::MBps => format!("{:.2}", s.mbps),
            Unit::GBps => format!("{:.3}", s.mbps / 1000.0),
            Unit::Iops => format!("{:.0}", s.iops),
            Unit::Latency => format!("{:.2}", s.latency_us),
        }
    }
}

pub struct BenchUi {
    volumes: Vec<Volume>,
    volume: usize,
    runs: u32,
    size: usize,
    duration_secs: u32,
    interval_secs: u32,
    profile: Profile,
    mode: Mode,
    unit: Unit,
    comment: String,

    /// Tests shown in the grid; frozen while a run is in progress.
    tests: [TestSpec; 4],
    results: [[Option<Score>; 2]; 4],
    status: String,
    progress: f32,
    active: Option<(usize, usize)>,
    error: Option<String>,
    handle: Option<BenchHandle>,
}

impl BenchUi {
    pub fn new() -> Self {
        let volumes = sys::volumes();
        let volume = volumes.iter().position(|v| v.letter == 'C').unwrap_or(0);
        Self {
            volumes,
            volume,
            runs: 5,
            size: 6, // 1 GiB
            duration_secs: 5,
            interval_secs: 1,
            profile: Profile::Default,
            mode: Mode::ReadWrite,
            unit: Unit::MBps,
            comment: String::new(),
            tests: Profile::Default.tests(),
            results: Default::default(),
            status: "Ready".into(),
            progress: 0.0,
            active: None,
            error: None,
            handle: None,
        }
    }

    fn running(&self) -> bool {
        self.handle.is_some()
    }

    fn poll(&mut self) {
        let Some(handle) = &self.handle else { return };
        let s = handle.shared.lock().unwrap();
        self.results = s.results;
        self.status = s.status.clone();
        self.progress = s.progress;
        self.active = s.active;
        if s.error.is_some() {
            self.error = s.error.clone();
        }
        let finished = !s.running;
        drop(s);
        if finished {
            self.handle = None;
        }
    }

    fn start(&mut self, ctx: &egui::Context, rows: [bool; 4]) {
        let Some(vol) = self.volumes.get(self.volume) else {
            self.error = Some("No drive selected".into());
            return;
        };
        let size = SIZES[self.size];
        if size + 64 * MIB > vol.free {
            self.error = Some(format!(
                "Not enough free space on {}: — needs {}, {} available",
                vol.letter,
                size_label(size),
                fmt_bytes(vol.free)
            ));
            return;
        }
        self.error = None;
        self.tests = self.profile.tests();
        let cfg = BenchConfig {
            drive_root: PathBuf::from(vol.root()),
            file_size: size,
            runs: self.runs,
            duration: Duration::from_secs(self.duration_secs as u64),
            interval: Duration::from_secs(self.interval_secs as u64),
            tests: self.tests,
            rows,
            mode: self.mode,
        };
        let ctx = ctx.clone();
        self.handle = Some(bench::start(cfg, move || ctx.request_repaint()));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let running = self.running();
        if !running {
            self.tests = self.profile.tests();
        }
        let p = theme::palette(ui);

        self.toolbar_ui(ui, &p, running);
        ui.add_space(14.0);
        self.grid_ui(ui, &p, running);
        ui.add_space(12.0);
        self.footer_ui(ui, &p, running);
    }

    fn toolbar_ui(&mut self, ui: &mut egui::Ui, p: &Palette, running: bool) {
        ui.horizontal_top(|ui| {
            if primary_button(ui, p, running).clicked() {
                if let Some(h) = &self.handle {
                    h.cancel();
                } else {
                    self.start(ui.ctx(), [true; 4]);
                }
            }
            ui.add_space(8.0);

            ui.add_enabled_ui(!running, |ui| {
                egui::Grid::new("bench_settings").num_columns(6).spacing([10.0, 8.0]).show(ui, |ui| {
                    let caption = |ui: &mut egui::Ui, t: &str| {
                        ui.label(RichText::new(t).color(p.muted));
                    };

                    caption(ui, "Drive");
                    let vol_text = |v: &Volume| {
                        let used = 1.0 - v.free as f64 / v.total.max(1) as f64;
                        let label = if v.label.is_empty() { String::new() } else { format!("{}  ", v.label) };
                        format!("{}:  {}{} · {:.0}% used", v.letter, label, fmt_bytes(v.total), used * 100.0)
                    };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        egui::ComboBox::from_id_salt("drive")
                            .selected_text(self.volumes.get(self.volume).map(vol_text).unwrap_or_default())
                            .width(228.0)
                            .show_ui(ui, |ui| {
                                for (i, v) in self.volumes.iter().enumerate() {
                                    ui.selectable_value(&mut self.volume, i, vol_text(v));
                                }
                            });
                        let refresh = egui::Button::new(RichText::new(theme::icon(Icon::RefreshCw)).color(p.muted)).frame(false);
                        if ui.add(refresh).on_hover_text("Rescan drives").clicked() {
                            self.volumes = sys::volumes();
                            self.volume = self.volume.min(self.volumes.len().saturating_sub(1));
                        }
                    });

                    caption(ui, "Size");
                    egui::ComboBox::from_id_salt("size")
                        .selected_text(size_label(SIZES[self.size]))
                        .width(104.0)
                        .show_ui(ui, |ui| {
                            for (i, s) in SIZES.iter().enumerate() {
                                ui.selectable_value(&mut self.size, i, size_label(*s));
                            }
                        });

                    caption(ui, "Runs");
                    egui::ComboBox::from_id_salt("runs")
                        .selected_text(self.runs.to_string())
                        .width(56.0)
                        .show_ui(ui, |ui| {
                            for n in 1..=9 {
                                ui.selectable_value(&mut self.runs, n, n.to_string());
                            }
                        });
                    ui.end_row();

                    caption(ui, "Profile");
                    egui::ComboBox::from_id_salt("profile")
                        .selected_text(self.profile.label())
                        .width(228.0)
                        .show_ui(ui, |ui| {
                            for pr in Profile::ALL {
                                ui.selectable_value(&mut self.profile, pr, pr.label());
                            }
                        });

                    caption(ui, "Mode");
                    egui::ComboBox::from_id_salt("mode")
                        .selected_text(mode_label(self.mode))
                        .width(104.0)
                        .show_ui(ui, |ui| {
                            for m in [Mode::ReadWrite, Mode::ReadOnly, Mode::WriteOnly] {
                                ui.selectable_value(&mut self.mode, m, mode_label(m));
                            }
                        });

                    caption(ui, "Timing");
                    let timing = format!("{} s / {} s", self.duration_secs, self.interval_secs);
                    ui.menu_button(timing, |ui| {
                        egui::Grid::new("timing").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                            ui.label(RichText::new("Measure").color(p.muted));
                            ui.add(egui::DragValue::new(&mut self.duration_secs).range(1..=60).suffix(" s"));
                            ui.end_row();
                            ui.label(RichText::new("Interval").color(p.muted));
                            ui.add(egui::DragValue::new(&mut self.interval_secs).range(0..=60).suffix(" s"));
                            ui.end_row();
                        });
                        ui.label(RichText::new("Interval pauses between tests so the drive can settle.").size(11.5).color(p.muted));
                    });
                    ui.end_row();
                });
            });
        });
    }

    fn grid_ui(&mut self, ui: &mut egui::Ui, p: &Palette, running: bool) {
        let gap = 8.0;
        let label_w = 128.0;
        let avail = ui.available_width();
        let cell_w = ((avail - label_w - gap * 2.0) / 2.0).max(120.0);
        let footer = 96.0;
        let header = 22.0;
        let row_h = ((ui.available_height() - footer - header - gap) / 4.0 - gap).clamp(56.0, 104.0);

        // Column headers.
        let (hdr, _) = ui.allocate_exact_size(Vec2::new(avail, header), Sense::hover());
        for (i, title) in ["Read", "Write"].into_iter().enumerate() {
            let x = hdr.left() + label_w + gap + i as f32 * (cell_w + gap) + 2.0;
            let g = ui.painter().layout_no_wrap(title.into(), theme::semibold(13.0), p.text);
            let w = g.size().x;
            ui.painter().galley(egui::pos2(x, hdr.center().y - g.size().y / 2.0), g, p.text);
            ui.painter().text(
                egui::pos2(x + w + 6.0, hdr.center().y),
                Align2::LEFT_CENTER,
                self.unit.label(),
                FontId::proportional(12.0),
                p.muted,
            );
        }
        ui.add_space(gap - ui.spacing().item_spacing.y);

        let tests = self.tests;
        for (row, spec) in tests.iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(avail, row_h), Sense::hover());
            let label_rect = Rect::from_min_size(rect.min, Vec2::new(label_w, row_h));
            if test_label(ui, p, label_rect, spec, row, !running).clicked() {
                let mut rows = [false; 4];
                rows[row] = true;
                self.start(ui.ctx(), rows);
            }
            for col in 0..2 {
                let min = rect.min + Vec2::new(label_w + gap + col as f32 * (cell_w + gap), 0.0);
                let active = self.active == Some((row, col));
                result_cell(ui, p, Rect::from_min_size(min, Vec2::new(cell_w, row_h)), self.results[row][col], self.unit, active);
            }
            ui.add_space(gap - ui.spacing().item_spacing.y);
        }
    }

    fn footer_ui(&mut self, ui: &mut egui::Ui, p: &Palette, running: bool) {
        // Status line with a thin progress track.
        ui.horizontal(|ui| {
            let (glyph, color) = if self.error.is_some() {
                (Icon::CircleAlert, p.bad)
            } else if running {
                (Icon::Activity, p.accent)
            } else {
                (Icon::CircleCheck, p.muted)
            };
            ui.label(RichText::new(theme::icon(glyph)).color(color));
            let text = self.error.clone().unwrap_or_else(|| self.status.clone());
            ui.label(RichText::new(text).color(if self.error.is_some() { p.bad } else { p.muted }));
            if running {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.0}%", self.progress * 100.0)).monospace().color(p.muted));
                });
            }
        });
        let (track, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 3.0), Sense::hover());
        ui.painter().rect_filled(track, CornerRadius::same(2), p.border);
        if self.progress > 0.0 {
            let mut fill = track;
            fill.set_width(track.width() * self.progress.clamp(0.0, 1.0));
            ui.painter().rect_filled(fill, CornerRadius::same(2), p.accent);
        }
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            let right = 300.0;
            ui.add(
                egui::TextEdit::singleline(&mut self.comment)
                    .hint_text("Comment — included when copying results")
                    .desired_width((ui.available_width() - right).max(120.0))
                    .min_size(Vec2::new(0.0, 28.0))
                    .margin(egui::Margin::symmetric(8, 5)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let copy = egui::Button::new(theme::with_icon(Icon::Copy, "Copy")).min_size(Vec2::new(0.0, 28.0));
                if ui.add(copy).on_hover_text("Copy a text report to the clipboard").clicked() {
                    ui.ctx().copy_text(self.report());
                }
                let opts: Vec<(Unit, String)> = Unit::ALL.iter().map(|u| (*u, u.label().to_string())).collect();
                theme::segmented(ui, &mut self.unit, &opts);
            });
        });
    }

    fn report(&self) -> String {
        let vol = self.volumes.get(self.volume);
        let mut s = String::new();
        s.push_str("------------------------------------------------------------\n");
        s.push_str(&format!("DiskTools {} benchmark\n", env!("CARGO_PKG_VERSION")));
        s.push_str("------------------------------------------------------------\n");
        s.push_str("* MB/s = 1,000,000 bytes/s\n\n");
        for (col, title) in [(0, "[Read]"), (1, "[Write]")] {
            s.push_str(title);
            s.push('\n');
            for (row, spec) in self.tests.iter().enumerate() {
                let line = match &self.results[row][col] {
                    Some(r) => format!(
                        "  {:<6} {:<6}: {:>10.3} MB/s [{:>10.1} IOPS] <{:>10.2} us>\n",
                        spec.name(),
                        spec.qt(),
                        r.mbps,
                        r.iops,
                        r.latency_us
                    ),
                    None => format!("  {:<6} {:<6}: -\n", spec.name(), spec.qt()),
                };
                s.push_str(&line);
            }
            s.push('\n');
        }
        s.push_str(&format!(
            "Profile: {}\n   Test: {} (x{}) [{}: {}]\n   Time: Measure {} sec / Interval {} sec\n",
            self.profile.label(),
            size_label(SIZES[self.size]),
            self.runs,
            vol.map(|v| v.letter).unwrap_or('?'),
            vol.map(|v| v.label.as_str()).unwrap_or(""),
            self.duration_secs,
            self.interval_secs,
        ));
        if !self.comment.is_empty() {
            s.push_str(&format!("Comment: {}\n", self.comment));
        }
        s
    }
}

/// Run / Stop button spanning both settings rows.
fn primary_button(ui: &mut egui::Ui, p: &Palette, running: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(128.0, 64.0), Sense::click());
    let hovered = resp.hovered();
    let radius = CornerRadius::same(8);
    let (fill, stroke, fg, label) = if running {
        let fill = if hovered { p.raised } else { p.surface };
        (fill, Stroke::new(1.0, p.border_strong), p.text, theme::with_icon(Icon::Square, "Stop"))
    } else {
        let fill = if hovered { p.accent.gamma_multiply(0.88) } else { p.accent };
        (fill, Stroke::NONE, p.on_accent, theme::with_icon(Icon::Play, "Run all"))
    };
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, stroke, StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, label, theme::semibold(14.0), fg);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

fn test_label(ui: &mut egui::Ui, p: &Palette, rect: Rect, spec: &TestSpec, row: usize, enabled: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, ui.scope_id().with(("test_label", row)), sense);
    let hovered = enabled && resp.hovered();
    let painter = ui.painter_at(rect);
    let radius = CornerRadius::same(8);
    painter.rect_filled(rect, radius, if hovered { p.raised } else { p.surface });
    painter.rect_stroke(rect, radius, Stroke::new(1.0, if hovered { p.border_strong } else { p.border }), StrokeKind::Inside);

    let (name, qt) = spec.display();
    let x = rect.left() + 14.0;
    painter.text(egui::pos2(x, rect.center().y + 1.0), Align2::LEFT_BOTTOM, name, theme::semibold(15.0), p.text);
    painter.text(egui::pos2(x, rect.center().y + 3.0), Align2::LEFT_TOP, qt, FontId::proportional(12.0), p.muted);
    if hovered {
        painter.text(
            egui::pos2(rect.right() - 12.0, rect.center().y),
            Align2::RIGHT_CENTER,
            theme::icon(Icon::Play),
            FontId::proportional(13.0),
            p.accent,
        );
    }
    if hovered {
        resp.on_hover_text("Run only this test")
    } else {
        resp
    }
}

fn result_cell(ui: &mut egui::Ui, p: &Palette, rect: Rect, score: Option<Score>, unit: Unit, active: bool) {
    let painter = ui.painter_at(rect);
    let radius = CornerRadius::same(8);
    painter.rect_filled(rect, radius, p.surface);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, if active { p.accent } else { p.border }), StrokeKind::Inside);

    // Meter along the bottom edge, log scale from 1 MB/s to 100 GB/s.
    if let Some(s) = &score {
        let frac = (s.mbps.max(1.0).log10() / 5.0).clamp(0.0, 1.0) as f32;
        let track = Rect::from_min_max(egui::pos2(rect.left() + 12.0, rect.bottom() - 9.0), egui::pos2(rect.right() - 12.0, rect.bottom() - 7.0));
        painter.rect_filled(track, CornerRadius::same(1), p.border);
        let mut bar = track;
        bar.set_width(track.width() * frac);
        painter.rect_filled(bar, CornerRadius::same(1), p.accent);
    }

    if active {
        painter.text(
            egui::pos2(rect.left() + 12.0, rect.top() + 10.0),
            Align2::LEFT_TOP,
            "measuring",
            FontId::proportional(11.5),
            p.accent,
        );
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }

    let (text, color) = match &score {
        Some(s) => (unit.format(s), p.text),
        None => ("–".to_string(), p.faint),
    };
    let size = (rect.height() * 0.38).clamp(22.0, 36.0);
    painter.text(
        egui::pos2(rect.right() - 16.0, rect.center().y - 2.0),
        Align2::RIGHT_CENTER,
        text,
        FontId::monospace(size),
        color,
    );
}
