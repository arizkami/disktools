use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, RichText, Sense, Stroke, StrokeKind, Vec2,
    text::{LayoutJob, TextFormat, TextWrapping},
};
use egui_extras::{Column, TableBuilder};

use crate::diskinfo::{self, DiskInfo, HealthStatus, Smart, fmt_bytes, fmt_bytes_u128};
use crate::theme::{self, Icon, Palette, group_digits};

const AUTO_REFRESH: Duration = Duration::from_secs(60);

fn health_color(p: &Palette, h: HealthStatus) -> Color32 {
    match h {
        HealthStatus::Good => p.good,
        HealthStatus::Caution => p.caution,
        HealthStatus::Bad => p.bad,
        HealthStatus::Unknown => p.faint,
    }
}

fn temp_status(t: Option<i32>) -> (HealthStatus, &'static str) {
    match t {
        None => (HealthStatus::Unknown, "Unavailable"),
        Some(t) if t <= 50 => (HealthStatus::Good, "Normal"),
        Some(t) if t <= 60 => (HealthStatus::Caution, "Warm"),
        Some(_) => (HealthStatus::Bad, "Hot"),
    }
}

pub struct InfoUi {
    ctx: egui::Context,
    disks: Vec<DiskInfo>,
    selected: usize,
    rx: Option<Receiver<Vec<DiskInfo>>>,
    last_refresh: Option<Instant>,
    auto_refresh: bool,
    hide_serial: bool,
}

impl InfoUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut s = Self {
            ctx: ctx.clone(),
            disks: Vec::new(),
            selected: 0,
            rx: None,
            last_refresh: None,
            auto_refresh: true,
            hide_serial: true,
        };
        s.refresh();
        s
    }

    fn refresh(&mut self) {
        if self.rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(diskinfo::enumerate());
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx
            && let Ok(disks) = rx.try_recv()
        {
            self.disks = disks;
            self.selected = self.selected.min(self.disks.len().saturating_sub(1));
            self.rx = None;
            self.last_refresh = Some(Instant::now());
        }
        if self.auto_refresh && self.last_refresh.is_some_and(|t| t.elapsed() > AUTO_REFRESH) {
            self.refresh();
        }
        self.ctx.request_repaint_after(AUTO_REFRESH);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, elevated: bool) {
        self.poll();
        let p = theme::palette(ui);

        egui::Panel::left("drive_list")
            .resizable(false)
            .exact_size(244.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().inner_margin(Margin { left: 0, right: 16, top: 0, bottom: 0 }))
            .show(ui, |ui| self.sidebar_ui(ui, &p));

        egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| {
            if self.disks.is_empty() {
                ui.centered_and_justified(|ui| {
                    let msg = if self.rx.is_some() { "Scanning drives…" } else { "No physical drives found" };
                    ui.label(RichText::new(msg).color(p.muted));
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.details_ui(ui, &p, elevated);
            });
        });
    }

    fn sidebar_ui(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Drives").font(theme::semibold(13.0)).color(p.text));
            ui.label(RichText::new(self.disks.len().to_string()).size(12.0).color(p.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.rx.is_some() {
                    ui.add(egui::Spinner::new().size(14.0).color(p.muted));
                } else {
                    let btn = egui::Button::new(RichText::new(theme::icon(Icon::RefreshCw)).color(p.muted)).frame(false);
                    if ui.add(btn).on_hover_text("Rescan").clicked() {
                        self.refresh();
                    }
                }
            });
        });
        ui.add_space(2.0);

        egui::ScrollArea::vertical().id_salt("drive_scroll").max_height(ui.available_height() - 36.0).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for i in 0..self.disks.len() {
                if drive_row(ui, p, &self.disks[i], i == self.selected).clicked() {
                    self.selected = i;
                }
            }
        });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.checkbox(&mut self.auto_refresh, RichText::new("Refresh every minute").color(p.muted));
        });
    }

    fn details_ui(&mut self, ui: &mut egui::Ui, p: &Palette, elevated: bool) {
        let d = self.disks[self.selected].clone();

        ui.label(RichText::new(d.display_name()).font(theme::semibold(22.0)).color(p.text));
        let mut facts = vec![d.bus.to_string()];
        if let Some(ssd) = d.ssd {
            facts.push(if ssd { "SSD".into() } else { "HDD".into() });
        }
        facts.push(fmt_bytes(d.size_bytes));
        if !d.letters.is_empty() {
            facts.push(d.letters.iter().map(|c| format!("{c}:")).collect::<Vec<_>>().join(" "));
        }
        ui.label(RichText::new(facts.join("  ·  ")).color(p.muted));
        ui.add_space(10.0);

        if !elevated {
            theme::admin_notice(ui, "Health data needs administrator rights.");
            ui.add_space(12.0);
        }

        // Stat tiles
        let gap = 8.0;
        let w = ((ui.available_width() - gap * 3.0) / 4.0).max(120.0);
        let (temp_state, temp_note) = temp_status(d.temperature_c);
        let health_value = match d.health {
            HealthStatus::Unknown => "—".to_string(),
            h => h.label().to_string(),
        };
        let health_note = match (d.life_percent, d.health) {
            (Some(life), _) => format!("{life}% life remaining"),
            (None, HealthStatus::Unknown) => if elevated { "Not reported" } else { "Unavailable" }.to_string(),
            (None, _) => "From S.M.A.R.T. thresholds".to_string(),
        };
        let tiles = [
            (Icon::Activity, "Health", health_value, health_note, health_color(p, d.health)),
            (
                Icon::Thermometer,
                "Temperature",
                d.temperature_c.map(|t| format!("{t} °C")).unwrap_or_else(|| "—".into()),
                temp_note.to_string(),
                if d.temperature_c.is_some() { health_color(p, temp_state) } else { p.faint },
            ),
            (
                Icon::Clock,
                "Powered on",
                d.power_on_hours.map(|h| format!("{} h", group_digits(h))).unwrap_or_else(|| "—".into()),
                d.power_cycles.map(|c| format!("{} power cycles", group_digits(c))).unwrap_or_else(|| "—".into()),
                if d.power_on_hours.is_some() { p.text } else { p.faint },
            ),
            (
                Icon::ArrowUpFromLine,
                "Host writes",
                d.host_writes.map(fmt_bytes_u128).unwrap_or_else(|| "—".into()),
                d.host_reads.map(|r| format!("{} read", fmt_bytes_u128(r))).unwrap_or_else(|| "—".into()),
                if d.host_writes.is_some() { p.text } else { p.faint },
            ),
        ];
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (icon, label, value, note, color) in tiles {
                stat_tile(ui, p, Vec2::new(w, 92.0), icon, label, &value, &note, color);
            }
        });
        ui.add_space(12.0);

        // Properties
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let serial = if self.hide_serial && !d.serial.is_empty() {
                "•".repeat(d.serial.len().min(16))
            } else if d.serial.is_empty() {
                "—".into()
            } else {
                d.serial.clone()
            };
            let yes_no = |b: Option<bool>| match b {
                Some(true) => "Supported",
                Some(false) => "Not supported",
                None => "—",
            };
            egui::Grid::new("props").num_columns(4).spacing([16.0, 10.0]).min_col_width(80.0).show(ui, |ui| {
                let key = |ui: &mut egui::Ui, k: &str| {
                    ui.label(RichText::new(k).color(p.muted));
                };
                key(ui, "Firmware");
                ui.label(RichText::new(if d.firmware.is_empty() { "—" } else { &d.firmware }).monospace());
                key(ui, "TRIM");
                ui.label(yes_no(d.trim));
                ui.end_row();

                key(ui, "Serial");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(serial).monospace());
                    let eye = if self.hide_serial { Icon::Eye } else { Icon::EyeOff };
                    let btn = egui::Button::new(RichText::new(theme::icon(eye)).size(12.0).color(p.muted)).frame(false);
                    if ui.add(btn).on_hover_text(if self.hide_serial { "Show" } else { "Hide" }).clicked() {
                        self.hide_serial = !self.hide_serial;
                    }
                });
                key(ui, "Removable");
                ui.label(if d.removable { "Yes" } else { "No" });
                ui.end_row();

                key(ui, "Device");
                ui.label(RichText::new(format!("PhysicalDrive{}", d.index)).monospace());
                key(ui, "Vendor");
                ui.label(if d.vendor.is_empty() { "—" } else { &d.vendor });
                ui.end_row();
            });
        });
        ui.add_space(16.0);

        // S.M.A.R.T. — the admin notice above already explains why it is missing.
        if !elevated {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("S.M.A.R.T.").font(theme::semibold(14.0)).color(p.text));
            let sub = match &d.smart {
                Smart::Ata(a) => format!("ATA · {} attributes", a.len()),
                Smart::Nvme(_) => "NVMe health log".into(),
                Smart::None => String::new(),
            };
            ui.label(RichText::new(sub).size(12.0).color(p.muted));
        });
        ui.add_space(4.0);
        match &d.smart {
            Smart::None => {
                theme::card(ui).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(theme::icon(Icon::Info)).color(p.muted));
                        let note = d.smart_note.clone().unwrap_or_else(|| "No data".into());
                        ui.label(RichText::new(note).color(p.muted));
                    });
                });
            }
            smart => {
                theme::card(ui).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    smart_table(ui, p, d.index, smart);
                });
            }
        }
    }
}

fn elided(text: &str, font: FontId, color: Color32, width: f32) -> LayoutJob {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat { font_id: font, color, ..Default::default() });
    job.wrap = TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    job
}

fn drive_row(ui: &mut egui::Ui, p: &Palette, d: &DiskInfo, selected: bool) -> egui::Response {
    let size = Vec2::new(ui.available_width(), 52.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(rect);
    let radius = CornerRadius::same(7);
    if selected {
        painter.rect_filled(rect, radius, p.raised);
    } else if resp.hovered() {
        painter.rect_filled(rect, radius, p.raised.gamma_multiply(0.5));
    }

    let dot = egui::pos2(rect.left() + 14.0, rect.top() + 18.0);
    painter.circle_filled(dot, 4.0, health_color(p, d.health));

    let x = rect.left() + 28.0;
    let name = elided(&d.display_name(), theme::semibold(13.0), p.text, rect.right() - x - 10.0);
    let galley = painter.layout_job(name);
    painter.galley(egui::pos2(x, rect.top() + 9.0), galley, p.text);

    let mut meta = Vec::new();
    if !d.letters.is_empty() {
        meta.push(d.letters.iter().map(|c| format!("{c}:")).collect::<Vec<_>>().join(" "));
    }
    meta.push(fmt_bytes(d.size_bytes));
    if let Some(t) = d.temperature_c {
        meta.push(format!("{t} °C"));
    }
    painter.text(
        egui::pos2(x, rect.bottom() - 9.0),
        Align2::LEFT_BOTTOM,
        meta.join("  ·  "),
        FontId::proportional(12.0),
        p.muted,
    );
    resp
}

#[allow(clippy::too_many_arguments)]
fn stat_tile(ui: &mut egui::Ui, p: &Palette, size: Vec2, icon: Icon, label: &str, value: &str, note: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let radius = CornerRadius::same(8);
    painter.rect_filled(rect, radius, p.surface);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, p.border), StrokeKind::Inside);

    let x = rect.left() + 14.0;
    painter.text(
        egui::pos2(x, rect.top() + 12.0),
        Align2::LEFT_TOP,
        theme::with_icon(icon, label),
        FontId::proportional(12.0),
        p.muted,
    );
    painter.text(egui::pos2(x, rect.top() + 32.0), Align2::LEFT_TOP, value, theme::semibold(22.0), color);
    let note = elided(note, FontId::proportional(12.0), p.muted, rect.width() - 28.0);
    let g = painter.layout_job(note);
    painter.galley(egui::pos2(x, rect.bottom() - 12.0 - g.size().y), g, p.muted);
}

fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
}

fn header(ui: &mut egui::Ui, p: &Palette, t: &str) {
    ui.label(RichText::new(t).size(12.0).color(p.muted));
}

fn smart_table(ui: &mut egui::Ui, p: &Palette, index: u32, smart: &Smart) {
    let row_h = 24.0;
    match smart {
        Smart::None => {}
        Smart::Ata(attrs) => {
            TableBuilder::new(ui)
                .id_salt(("smart", index))
                .vscroll(false)
                .striped(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(18.0))
                .column(Column::exact(36.0))
                .column(Column::remainder().at_least(180.0))
                .column(Column::exact(60.0))
                .column(Column::exact(56.0))
                .column(Column::exact(72.0))
                .column(Column::exact(116.0))
                .header(row_h, |mut h| {
                    for t in ["", "ID", "Attribute", "Current", "Worst", "Threshold", "Raw"] {
                        h.col(|ui| header(ui, p, t));
                    }
                })
                .body(|mut body| {
                    for a in attrs {
                        body.row(row_h, |mut row| {
                            let status = if a.failing() {
                                HealthStatus::Bad
                            } else if matches!(a.id, 0x05 | 0xC5 | 0xC6) && a.raw & 0xFFFF_FFFF > 0 {
                                HealthStatus::Caution
                            } else {
                                HealthStatus::Good
                            };
                            row.col(|ui| status_dot(ui, health_color(p, status)));
                            row.col(|ui| {
                                ui.label(RichText::new(format!("{:02X}", a.id)).monospace().color(p.muted));
                            });
                            row.col(|ui| {
                                ui.label(a.name());
                            });
                            for v in [a.current, a.worst, a.threshold] {
                                row.col(|ui| {
                                    ui.label(RichText::new(v.to_string()).monospace());
                                });
                            }
                            row.col(|ui| {
                                ui.label(RichText::new(format!("{:012X}", a.raw)).monospace().color(p.muted));
                            });
                        });
                    }
                });
        }
        Smart::Nvme(h) => {
            TableBuilder::new(ui)
                .id_salt(("nvme", index))
                .vscroll(false)
                .striped(false)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::exact(18.0))
                .column(Column::exact(36.0))
                .column(Column::remainder().at_least(240.0))
                .column(Column::exact(220.0))
                .header(row_h, |mut hdr| {
                    for t in ["", "ID", "Attribute", "Value"] {
                        hdr.col(|ui| header(ui, p, t));
                    }
                })
                .body(|mut body| {
                    for (id, name, value) in h.rows() {
                        body.row(row_h, |mut row| {
                            let bad = (id == 0x01 && h.critical_warning != 0)
                                || (id == 0x03 && h.spare_threshold > 0 && h.available_spare < h.spare_threshold);
                            let caution = id == 0x0E && h.media_errors > 0;
                            let status = if bad {
                                HealthStatus::Bad
                            } else if caution {
                                HealthStatus::Caution
                            } else {
                                HealthStatus::Good
                            };
                            row.col(|ui| status_dot(ui, health_color(p, status)));
                            row.col(|ui| {
                                ui.label(RichText::new(format!("{id:02X}")).monospace().color(p.muted));
                            });
                            row.col(|ui| {
                                ui.label(name);
                            });
                            row.col(|ui| {
                                ui.label(RichText::new(value).monospace());
                            });
                        });
                    }
                });
        }
    }
}
