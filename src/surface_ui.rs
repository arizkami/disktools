use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2};

use crate::bench::MIB;
use crate::diskinfo::{self, DiskInfo, fmt_bytes};
use crate::surface::{
    self, Accuracy, Cell, SCAN_CELLS, ScanConfig, ScanMode, ScanState, Task, TransferConfig, TransferState,
};
use crate::theme::{self, Icon, Palette};

const BLOCKS: [u64; 4] = [64 * 1024, 512 * 1024, MIB, 8 * MIB];
const SCAN_COLS: usize = 50;

fn block_label(b: u64) -> String {
    if b >= MIB { format!("{} MiB", b / MIB) } else { format!("{} KiB", b / 1024) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Test {
    Transfer,
    Scan,
}

pub struct SurfaceUi {
    disks: Vec<DiskInfo>,
    disk: usize,
    rx: Option<Receiver<Vec<DiskInfo>>>,

    test: Test,
    accuracy: Accuracy,
    block: usize,
    scan_mode: ScanMode,

    transfer: TransferState,
    transfer_task: Option<Task<TransferState>>,
    scan: ScanState,
    scan_task: Option<Task<ScanState>>,
}

impl SurfaceUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(diskinfo::identify_all());
            repaint.request_repaint();
        });
        #[allow(unused_mut)]
        let mut ui = Self {
            disks: Vec::new(),
            disk: 0,
            rx: Some(rx),
            test: Test::Transfer,
            accuracy: Accuracy::Balanced,
            block: 2,
            scan_mode: ScanMode::Quick,
            transfer: TransferState { status: "Ready".into(), ..Default::default() },
            transfer_task: None,
            scan: ScanState::empty(),
            scan_task: None,
        };
        #[cfg(debug_assertions)]
        ui.start_demo(&ctx);
        ui
    }

    fn running(&self) -> bool {
        self.transfer_task.is_some() || self.scan_task.is_some()
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx
            && let Ok(disks) = rx.try_recv()
        {
            self.disks = disks;
            self.rx = None;
        }
        if let Some(t) = &self.transfer_task {
            self.transfer = t.state.lock().unwrap().clone();
            if !self.transfer.running {
                self.transfer_task = None;
            }
        }
        if let Some(t) = &self.scan_task {
            self.scan = t.state.lock().unwrap().clone();
            if !self.scan.running {
                self.scan_task = None;
            }
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(d) = self.disks.get(self.disk) else { return };
        let (path, size) = (surface::disk_path(d.index), d.size_bytes);
        self.start_on(ctx, path, size);
    }

    fn start_on(&mut self, ctx: &egui::Context, path: String, disk_size: u64) {
        let repaint = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        match self.test {
            Test::Transfer => {
                let cfg = TransferConfig { path, disk_size, accuracy: self.accuracy, block: BLOCKS[self.block] };
                self.transfer_task = Some(surface::start_transfer(cfg, repaint));
            }
            Test::Scan => {
                let cfg = ScanConfig { path, disk_size, mode: self.scan_mode };
                self.scan_task = Some(surface::start_scan(cfg, repaint));
            }
        }
    }

    /// Debug builds only: `DISKTOOLS_DEMO_TRANSFER=<file>` or `DISKTOOLS_DEMO_SCAN=<file>`
    /// runs that test against a plain file at startup, so the views can be checked
    /// without administrator rights.
    #[cfg(debug_assertions)]
    fn start_demo(&mut self, ctx: &egui::Context) {
        for (var, test) in [("DISKTOOLS_DEMO_TRANSFER", Test::Transfer), ("DISKTOOLS_DEMO_SCAN", Test::Scan)] {
            if let Ok(path) = std::env::var(var)
                && let Ok(meta) = std::fs::metadata(&path)
            {
                self.test = test;
                self.start_on(ctx, path, meta.len());
                return;
            }
        }
    }

    fn stop(&self) {
        if let Some(t) = &self.transfer_task {
            t.cancel();
        }
        if let Some(t) = &self.scan_task {
            t.cancel();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, elevated: bool) {
        self.poll();
        let running = self.running();
        let p = theme::palette(ui);

        self.toolbar_ui(ui, &p, running);
        ui.add_space(12.0);
        if !elevated {
            theme::admin_notice(ui, "Raw disk tests read the physical drive and need administrator rights.");
            ui.add_space(12.0);
        }

        let footer = 40.0;
        let gap = 12.0;
        let side = 200.0;
        let h = (ui.available_height() - footer - gap).max(220.0);
        let w = ui.available_width();
        let (area, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let main = Rect::from_min_size(area.min, Vec2::new(w - side - gap, h));
        let stats = Rect::from_min_max(pos2(main.right() + gap, area.top()), area.max);

        match self.test {
            Test::Transfer => {
                transfer_chart(ui, &p, main, &self.transfer);
                transfer_stats(ui, &p, stats, &self.transfer);
            }
            Test::Scan => {
                scan_grid(ui, &p, main, &self.scan);
                scan_stats(ui, &p, stats, &self.scan);
            }
        }

        ui.add_space(gap);
        let (status, error, progress) = match self.test {
            Test::Transfer => (&self.transfer.status, self.transfer.error.as_deref(), self.transfer.progress),
            Test::Scan => (&self.scan.status, self.scan.error.as_deref(), self.scan.progress),
        };
        theme::status_footer(ui, status, error, progress, running);
        if running {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }

    fn toolbar_ui(&mut self, ui: &mut egui::Ui, p: &Palette, running: bool) {
        ui.horizontal(|ui| {
            if theme::primary_button(ui, running, "Start", Vec2::new(104.0, 30.0)).clicked() {
                if running {
                    self.stop();
                } else {
                    self.start(ui.ctx());
                }
            }
            ui.add_space(4.0);

            ui.add_enabled_ui(!running, |ui| {
                let disk_text = |d: &DiskInfo| {
                    let letters: String = d.letters.iter().map(|c| format!("  {c}:")).collect();
                    format!("{}  ·  {}{}", d.display_name(), fmt_bytes(d.size_bytes), letters)
                };
                let selected = match self.disks.get(self.disk) {
                    Some(d) => disk_text(d),
                    None if self.rx.is_some() => "Scanning drives…".into(),
                    None => "No drives".into(),
                };
                egui::ComboBox::from_id_salt("surface_disk").selected_text(selected).width(300.0).show_ui(ui, |ui| {
                    for (i, d) in self.disks.iter().enumerate() {
                        ui.selectable_value(&mut self.disk, i, disk_text(d));
                    }
                });

                theme::segmented(
                    ui,
                    &mut self.test,
                    &[
                        (Test::Transfer, theme::with_icon(Icon::ChartLine, "Transfer rate")),
                        (Test::Scan, theme::with_icon(Icon::Grid3x3, "Error scan")),
                    ],
                );

                match self.test {
                    Test::Transfer => {
                        egui::ComboBox::from_id_salt("accuracy")
                            .selected_text(self.accuracy.label())
                            .width(88.0)
                            .show_ui(ui, |ui| {
                                for a in Accuracy::ALL {
                                    ui.selectable_value(&mut self.accuracy, a, a.label());
                                }
                            })
                            .response
                            .on_hover_text("More sample points and more data per point = smoother graph, longer test");
                        egui::ComboBox::from_id_salt("block")
                            .selected_text(block_label(BLOCKS[self.block]))
                            .width(72.0)
                            .show_ui(ui, |ui| {
                                for (i, b) in BLOCKS.iter().enumerate() {
                                    ui.selectable_value(&mut self.block, i, block_label(*b));
                                }
                            })
                            .response
                            .on_hover_text("Read request size");
                    }
                    Test::Scan => {
                        theme::segmented(
                            ui,
                            &mut self.scan_mode,
                            &[(ScanMode::Quick, "Quick".into()), (ScanMode::Full, "Full".into())],
                        );
                        ui.label(RichText::new(theme::icon(Icon::Info)).color(p.muted)).on_hover_text(
                            "Quick reads the first 8 MiB of every block.\nFull reads every byte — hours on large hard drives.",
                        );
                    }
                }
            });
        });
    }
}

/// 1, 2, 2.5 or 5 × 10^n step that gives roughly `target` ticks up to `max`.
fn nice_step(max: f64, target: f64) -> f64 {
    let raw = (max / target).max(1e-9);
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    let step = if norm <= 1.0 {
        1.0
    } else if norm <= 2.0 {
        2.0
    } else if norm <= 2.5 {
        2.5
    } else if norm <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * mag
}

/// Axis label: up to two decimals, trailing zeros dropped (0.25, 0.5, 600).
fn fmt_tick(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn card_bg(ui: &egui::Ui, p: &Palette, rect: Rect) {
    let radius = CornerRadius::same(8);
    ui.painter().rect_filled(rect, radius, p.surface);
    ui.painter().rect_stroke(rect, radius, Stroke::new(1.0, p.border), StrokeKind::Inside);
}

fn transfer_chart(ui: &mut egui::Ui, p: &Palette, rect: Rect, st: &TransferState) {
    card_bg(ui, p, rect);
    let painter = ui.painter_at(rect);
    let small = FontId::proportional(11.0);
    let plot = Rect::from_min_max(pos2(rect.left() + 56.0, rect.top() + 34.0), pos2(rect.right() - 48.0, rect.bottom() - 30.0));

    // Legend
    let ly = rect.top() + 17.0;
    let lx = plot.left();
    painter.line_segment([pos2(lx, ly), pos2(lx + 16.0, ly)], Stroke::new(2.0, p.accent));
    let g = painter.layout_no_wrap("Transfer rate  MB/s".into(), small.clone(), p.muted);
    let tw = g.size().x;
    painter.galley(pos2(lx + 22.0, ly - g.size().y / 2.0), g, p.muted);
    let ax = lx + 22.0 + tw + 18.0;
    painter.circle_filled(pos2(ax + 4.0, ly), 2.5, p.info);
    painter.text(pos2(ax + 14.0, ly), Align2::LEFT_CENTER, "Access time  ms", small.clone(), p.muted);

    let size = st.disk_size.max(1) as f64;
    let rate_max = st.rate.iter().map(|r| r.1).fold(0.0, f64::max);
    let y_step = nice_step(if rate_max > 0.0 { rate_max * 1.1 } else { 500.0 }, 5.0);
    let y_max = ((if rate_max > 0.0 { rate_max * 1.1 } else { 500.0 }) / y_step).ceil() * y_step;
    // The ms axis shares the MB/s gridlines, so pick a round ms step for that many intervals.
    let ticks = (y_max / y_step).round().max(1.0);
    let acc_max = st.access.iter().map(|a| a.1).fold(0.0, f64::max);
    let a_max = nice_step(if acc_max > 0.0 { acc_max * 1.05 } else { 20.0 }, ticks) * ticks;

    let to_x = |pos: u64| plot.left() + (pos as f64 / size) as f32 * plot.width();
    let to_y = |v: f64, max: f64| plot.bottom() - (v / max).clamp(0.0, 1.0) as f32 * plot.height();

    // Horizontal grid + left (MB/s) labels.
    let mut v = 0.0;
    while v <= y_max + 1e-9 {
        let y = to_y(v, y_max);
        painter.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, p.border));
        painter.text(pos2(plot.left() - 8.0, y), Align2::RIGHT_CENTER, fmt_tick(v), small.clone(), p.muted);
        v += y_step;
    }
    // Right (ms) labels, on the same gridlines.
    for i in 0..=ticks as i32 {
        let frac = i as f64 / ticks;
        let y = plot.bottom() - frac as f32 * plot.height();
        painter.text(pos2(plot.right() + 8.0, y), Align2::LEFT_CENTER, fmt_tick(frac * a_max), small.clone(), p.info);
    }
    // Vertical grid + position labels.
    let gb = size / 1e9;
    let x_step = nice_step(gb, 6.0);
    let mut x = 0.0;
    while x <= gb + 1e-9 {
        let px = plot.left() + (x / gb) as f32 * plot.width();
        painter.line_segment([pos2(px, plot.top()), pos2(px, plot.bottom())], Stroke::new(1.0, p.border.gamma_multiply(0.6)));
        let label = if x == 0.0 { "0".to_string() } else { format!("{} GB", fmt_tick(x)) };
        painter.text(pos2(px, plot.bottom() + 8.0), Align2::CENTER_TOP, label, small.clone(), p.muted);
        x += x_step;
    }

    if st.rate.is_empty() && st.access.is_empty() {
        let msg = if st.running { "Measuring burst rate…" } else { "Reads the whole drive surface and plots speed by position" };
        painter.text(plot.center(), Align2::CENTER_CENTER, msg, FontId::proportional(13.0), p.muted);
        return;
    }

    for &(pos, ms) in &st.access {
        painter.circle_filled(pos2(to_x(pos), to_y(ms, a_max)), 1.8, p.info.gamma_multiply(0.85));
    }
    let line: Vec<Pos2> = st.rate.iter().map(|&(pos, r)| pos2(to_x(pos), to_y(r, y_max))).collect();
    if line.len() > 1 {
        painter.line(line, Stroke::new(1.75, p.accent));
    } else if let Some(pt) = line.first() {
        painter.circle_filled(*pt, 2.0, p.accent);
    }

    // Hover readout
    if let Some(hover) = ui.ctx().pointer_hover_pos()
        && plot.contains(hover)
        && let Some(&(pos, rate)) = st.rate.iter().min_by(|a, b| {
            (to_x(a.0) - hover.x).abs().total_cmp(&(to_x(b.0) - hover.x).abs())
        })
    {
        let x = to_x(pos);
        painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, p.border_strong));
        painter.circle_filled(pos2(x, to_y(rate, y_max)), 3.5, p.accent);
        let text = format!("{:.1} GB   {:.1} MB/s", pos as f64 / 1e9, rate);
        let g = painter.layout_no_wrap(text, FontId::proportional(12.0), p.text);
        let mut tip = Rect::from_min_size(pos2(x + 10.0, plot.top() + 6.0), g.size() + Vec2::new(16.0, 10.0));
        if tip.right() > plot.right() {
            tip = tip.translate(Vec2::new(-tip.width() - 20.0, 0.0));
        }
        painter.rect_filled(tip, CornerRadius::same(6), p.raised);
        painter.rect_stroke(tip, CornerRadius::same(6), Stroke::new(1.0, p.border_strong), StrokeKind::Inside);
        painter.galley(tip.min + Vec2::new(8.0, 5.0), g, p.text);
    }
}

fn stat_list(ui: &egui::Ui, p: &Palette, rect: Rect, rows: &[(&str, String, &str, Color32)]) {
    let painter = ui.painter_at(rect);
    let h = 62.0;
    let gap = 8.0;
    for (i, (label, value, unit, color)) in rows.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left(), rect.top() + i as f32 * (h + gap)), Vec2::new(rect.width(), h));
        if r.bottom() > rect.bottom() {
            break;
        }
        card_bg(ui, p, r);
        painter.text(pos2(r.left() + 14.0, r.top() + 11.0), Align2::LEFT_TOP, *label, FontId::proportional(12.0), p.muted);
        let g = painter.layout_no_wrap(value.clone(), theme::semibold(19.0), *color);
        let vw = g.size().x;
        let base = pos2(r.left() + 14.0, r.bottom() - 10.0 - g.size().y);
        painter.galley(base, g, *color);
        painter.text(
            pos2(base.x + vw + 5.0, r.bottom() - 13.0),
            Align2::LEFT_BOTTOM,
            *unit,
            FontId::proportional(12.0),
            p.muted,
        );
    }
}

fn transfer_stats(ui: &mut egui::Ui, p: &Palette, rect: Rect, st: &TransferState) {
    let dash = || "—".to_string();
    let mma = st.min_max_avg();
    let val = |v: Option<f64>| v.map(|v| format!("{v:.1}")).unwrap_or_else(dash);
    let color = |v: Option<f64>, c: Color32| if v.is_some() { c } else { p.faint };
    let (min, max, avg) = (mma.map(|m| m.0), mma.map(|m| m.1), mma.map(|m| m.2));
    let access = st.access_avg_ms();
    stat_list(
        ui,
        p,
        rect,
        &[
            ("Minimum", val(min), "MB/s", color(min, p.text)),
            ("Maximum", val(max), "MB/s", color(max, p.text)),
            ("Average", val(avg), "MB/s", color(avg, p.accent)),
            ("Access time", access.map(|a| format!("{a:.2}")).unwrap_or_else(dash), "ms", color(access, p.info)),
            ("Burst rate", val(st.burst_mbps), "MB/s", color(st.burst_mbps, p.text)),
        ],
    );
}

fn scan_grid(ui: &mut egui::Ui, p: &Palette, rect: Rect, st: &ScanState) {
    card_bg(ui, p, rect);
    let inner = rect.shrink(16.0);
    let rows = SCAN_CELLS.div_ceil(SCAN_COLS);
    let gap = 2.0;
    let cell = ((inner.width() - gap * (SCAN_COLS as f32 - 1.0)) / SCAN_COLS as f32)
        .min((inner.height() - 24.0 - gap * (rows as f32 - 1.0)) / rows as f32)
        .floor()
        .max(3.0);
    let grid_w = cell * SCAN_COLS as f32 + gap * (SCAN_COLS as f32 - 1.0);
    let grid_h = cell * rows as f32 + gap * (rows as f32 - 1.0);
    let origin = pos2(inner.center().x - grid_w / 2.0, inner.top() + (inner.height() - 24.0 - grid_h) / 2.0);
    let painter = ui.painter_at(rect);
    let radius = CornerRadius::same(if cell >= 8.0 { 2 } else { 1 });

    let cell_rect = |i: usize| {
        let (r, c) = (i / SCAN_COLS, i % SCAN_COLS);
        Rect::from_min_size(origin + Vec2::new(c as f32 * (cell + gap), r as f32 * (cell + gap)), Vec2::splat(cell))
    };
    for (i, state) in st.cells.iter().enumerate() {
        let fill = match state {
            Cell::Pending => p.border_strong.gamma_multiply(0.6),
            Cell::Ok => p.good.gamma_multiply(0.8),
            Cell::Bad => p.bad,
        };
        painter.rect_filled(cell_rect(i), radius, fill);
    }
    if let Some(cur) = st.current {
        painter.rect_stroke(cell_rect(cur).expand(1.0), radius, Stroke::new(1.5, p.text), StrokeKind::Outside);
    }

    // Legend
    let ly = rect.bottom() - 22.0;
    let mut x = origin.x;
    for (label, color) in [("OK", p.good.gamma_multiply(0.8)), ("Damaged", p.bad), ("Not scanned", p.border_strong.gamma_multiply(0.6))] {
        painter.rect_filled(Rect::from_center_size(pos2(x + 5.0, ly), Vec2::splat(10.0)), CornerRadius::same(2), color);
        let g = painter.layout_no_wrap(label.into(), FontId::proportional(12.0), p.muted);
        let w = g.size().x;
        painter.galley(pos2(x + 16.0, ly - g.size().y / 2.0), g, p.muted);
        x += 16.0 + w + 18.0;
    }
    if st.cell_bytes > 0 {
        painter.text(
            pos2(origin.x + grid_w, ly),
            Align2::RIGHT_CENTER,
            format!("1 block = {}", fmt_bytes(st.cell_bytes)),
            FontId::proportional(12.0),
            p.muted,
        );
    }

    if let Some(hover) = ui.ctx().pointer_hover_pos()
        && let Some(i) = (0..SCAN_CELLS).find(|&i| cell_rect(i).contains(hover))
        && st.cell_bytes > 0
    {
        let start = i as u64 * st.cell_bytes;
        let state = match st.cells[i] {
            Cell::Pending => "not scanned",
            Cell::Ok => "OK",
            Cell::Bad => "read error",
        };
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.scope_id().with("scan_tip"), hover).show(|ui| {
            ui.label(format!(
                "Block {}  ·  {:.2}–{:.2} GB  ·  {state}",
                i + 1,
                start as f64 / 1e9,
                (start + st.cell_bytes) as f64 / 1e9
            ));
        });
    }
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn scan_stats(ui: &mut egui::Ui, p: &Palette, rect: Rect, st: &ScanState) {
    let started = st.bytes_read > 0;
    let remaining = if started && st.progress > 0.0 && st.progress < 1.0 {
        let total = st.elapsed.as_secs_f64() / st.progress as f64;
        fmt_duration(Duration::from_secs_f64((total - st.elapsed.as_secs_f64()).max(0.0)))
    } else {
        "—".into()
    };
    let faint_or = |c: Color32| if started { c } else { p.faint };
    stat_list(
        ui,
        p,
        rect,
        &[
            (
                "Scanned",
                if started { fmt_bytes(st.bytes_read) } else { "—".into() },
                "",
                faint_or(p.text),
            ),
            (
                "Speed",
                if started { format!("{:.1}", st.speed_mbps()) } else { "—".into() },
                "MB/s",
                faint_or(p.text),
            ),
            ("Elapsed", if started { fmt_duration(st.elapsed) } else { "—".into() }, "", faint_or(p.text)),
            ("Remaining", remaining, "", faint_or(p.text)),
            (
                "Damaged blocks",
                if started { st.bad.to_string() } else { "—".into() },
                "",
                if st.bad > 0 { p.bad } else { faint_or(p.good) },
            ),
        ],
    );
}
