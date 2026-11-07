use crate::compare::{Options, Thresholds};
use crate::scan::{self, Member, Msg, Phase, ScanResult};
use crate::{APP_NAME, LOGO_PNG, VERSION};
use crossbeam_channel::{Receiver, unbounded};
use eframe::egui::{
    self, Align, Color32, ColorImage, CornerRadius, Frame, Key, Layout, Margin, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

const ACCENT: Color32 = Color32::from_rgb(99, 140, 255);
const TEAL: Color32 = Color32::from_rgb(86, 214, 196);
const DANGER: Color32 = Color32::from_rgb(240, 86, 96);
const OK: Color32 = Color32::from_rgb(72, 199, 142);
const BG: Color32 = Color32::from_rgb(17, 19, 24);
const HEADER: Color32 = Color32::from_rgb(20, 23, 30);
const PANEL: Color32 = Color32::from_rgb(24, 27, 34);
const CARD: Color32 = Color32::from_rgb(31, 35, 44);
const BORDER: Color32 = Color32::from_rgb(44, 49, 60);
const MUTED: Color32 = Color32::from_rgb(140, 148, 165);

/// Which image of a group is kept when auto-marking; all others get marked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum KeepRule {
    HighestResolution,
    LargestFile,
    ShortestPath,
    Newest,
    Oldest,
}

impl KeepRule {
    const ALL: [KeepRule; 5] =
        [Self::HighestResolution, Self::LargestFile, Self::ShortestPath, Self::Newest, Self::Oldest];

    fn label(self) -> &'static str {
        match self {
            Self::HighestResolution => "Keep highest resolution",
            Self::LargestFile => "Keep largest file",
            Self::ShortestPath => "Mark longest paths",
            Self::Newest => "Mark older files",
            Self::Oldest => "Mark newer files",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::HighestResolution => "Keeps the image with the most pixels, marks the rest",
            Self::LargestFile => "Keeps the biggest file, marks the rest",
            Self::ShortestPath => "Keeps the image with the shortest path, marks deeper or longer paths",
            Self::Newest => "Keeps the most recently modified file, marks older ones",
            Self::Oldest => "Keeps the oldest file, marks newer copies",
        }
    }
}

struct Pic {
    m: Member,
    tex: Option<TextureHandle>,
    marked: bool,
    removed: bool,
}

impl Pic {
    fn pixels(&self) -> u64 {
        self.m.width as u64 * self.m.height as u64
    }
    fn path_len(&self) -> usize {
        self.m.path.as_os_str().len()
    }
}

#[derive(Default)]
struct Progress {
    phase: Option<Phase>,
    done: usize,
    total: usize,
}

struct Stats {
    scanned: usize,
    failed: usize,
    seconds: f64,
}

pub struct App {
    logo: TextureHandle,
    folders: Vec<PathBuf>,
    level: i32,
    thresholds: Thresholds,
    rotations: bool,
    background: bool,
    only_across_roots: bool,
    thumb_size: f32,
    keep_rule: KeepRule,

    rx: Option<Receiver<Msg>>,
    cancel: Arc<AtomicBool>,
    progress: Progress,

    groups: Vec<Vec<Pic>>,
    selected: Option<(usize, usize)>,
    stats: Option<Stats>,
    show_settings: bool,
    show_about: bool,
    confirm_trash: bool,
    toast: Option<(String, Color32, f64)>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        style(&cc.egui_ctx);
        let logo = image::load_from_memory(LOGO_PNG).expect("embedded logo").to_rgba8();
        let logo = cc.egui_ctx.load_texture(
            "logo",
            ColorImage::from_rgba_unmultiplied([logo.width() as usize, logo.height() as usize], logo.as_raw()),
            TextureOptions::LINEAR,
        );
        let folders = std::env::args().skip(1).map(PathBuf::from).filter(|p| p.is_dir()).collect();
        Self {
            logo,
            folders,
            level: 4,
            thresholds: Thresholds::from_level(4),
            rotations: false,
            background: true,
            only_across_roots: false,
            thumb_size: 170.0,
            keep_rule: KeepRule::HighestResolution,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Progress::default(),
            groups: Vec::new(),
            selected: None,
            stats: None,
            show_settings: false,
            show_about: false,
            confirm_trash: false,
            toast: None,
        }
    }

    fn running(&self) -> bool {
        self.rx.is_some()
    }

    fn custom_thresholds(&self) -> bool {
        self.thresholds != Thresholds::from_level(self.level)
    }

    fn start_scan(&mut self) {
        if self.folders.is_empty() || self.running() {
            return;
        }
        let (tx, rx) = unbounded();
        self.cancel = Arc::new(AtomicBool::new(false));
        let opt = Options {
            thresholds: self.thresholds,
            rotations: self.rotations,
            background: self.background,
            only_across_roots: self.only_across_roots,
        };
        scan::start(self.folders.clone(), opt, self.cancel.clone(), tx);
        self.rx = Some(rx);
        self.groups.clear();
        self.selected = None;
        self.progress = Progress::default();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        let mut finished: Option<Option<ScanResult>> = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Progress { phase, done, total } => self.progress = Progress { phase: Some(phase), done, total },
                Msg::Finished(res) => finished = Some(Some(res)),
                Msg::Cancelled => finished = Some(None),
            }
        }
        match finished {
            Some(Some(res)) => {
                self.stats =
                    Some(Stats { scanned: res.scanned, failed: res.failed, seconds: res.elapsed.as_secs_f64() });
                self.groups = res
                    .groups
                    .into_iter()
                    .map(|g| g.into_iter().map(|m| Pic { m, tex: None, marked: false, removed: false }).collect())
                    .collect();
                self.rx = None;
                let n = self.groups.len();
                let text = match n {
                    0 => "No similar images found".to_string(),
                    1 => "1 group found".to_string(),
                    _ => format!("{n} groups found"),
                };
                self.notify(text, OK, ctx);
            }
            Some(None) => {
                self.rx = None;
                self.notify("Scan cancelled".into(), MUTED, ctx);
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
        }
    }

    fn notify(&mut self, text: String, color: Color32, ctx: &egui::Context) {
        self.toast = Some((text, color, ctx.input(|i| i.time) + 3.5));
    }

    fn marked_count(&self) -> usize {
        self.groups.iter().flatten().filter(|p| p.marked && !p.removed).count()
    }

    fn visible_pics(&self) -> usize {
        self.groups.iter().flatten().filter(|p| !p.removed).count()
    }

    fn auto_mark(&mut self) {
        let rule = self.keep_rule;
        for g in &mut self.groups {
            let alive: Vec<usize> = (0..g.len()).filter(|&i| !g[i].removed).collect();
            let time = |p: &Pic| p.m.modified.unwrap_or(SystemTime::UNIX_EPOCH);
            // Ties fall back to resolution, then file size.
            let keep = alive.iter().copied().max_by(|&a, &b| {
                let (pa, pb) = (&g[a], &g[b]);
                let primary = match rule {
                    KeepRule::HighestResolution => pa.pixels().cmp(&pb.pixels()),
                    KeepRule::LargestFile => pa.m.bytes.cmp(&pb.m.bytes),
                    KeepRule::ShortestPath => pb.path_len().cmp(&pa.path_len()),
                    KeepRule::Newest => time(pa).cmp(&time(pb)),
                    KeepRule::Oldest => time(pb).cmp(&time(pa)),
                };
                primary.then(pa.pixels().cmp(&pb.pixels())).then(pa.m.bytes.cmp(&pb.m.bytes)).then(b.cmp(&a))
            });
            for &i in &alive {
                g[i].marked = Some(i) != keep;
            }
        }
    }

    fn trash_marked(&mut self, ctx: &egui::Context) {
        let mut ok = 0;
        let mut errors = Vec::new();
        for p in self.groups.iter_mut().flatten().filter(|p| p.marked && !p.removed) {
            match trash::delete(&p.m.path) {
                Ok(()) => {
                    p.removed = true;
                    p.marked = false;
                    ok += 1;
                }
                Err(e) => errors.push(format!("{}: {e}", p.m.path.display())),
            }
        }
        // Hide groups with fewer than two remaining images
        self.groups.retain(|g| g.iter().filter(|p| !p.removed).count() > 1);
        self.selected = None;
        if errors.is_empty() {
            self.notify(format!("Moved {ok} files to the trash"), OK, ctx);
        } else {
            self.notify(format!("Moved {ok}, {} failed – e.g. {}", errors.len(), errors[0]), DANGER, ctx);
        }
    }

    // ---------- Header ----------
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(Vec2::splat(38.0)));
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(APP_NAME).size(20.0).strong().color(Color32::WHITE));
                    ui.label(RichText::new(format!("v{VERSION}")).size(11.0).color(MUTED));
                });
                ui.label(RichText::new("Find similar and duplicate images").size(12.0).color(MUTED));
            });

            ui.add_space(28.0);
            if self.running() {
                let p = &self.progress;
                let frac = if p.total > 0 { p.done as f32 / p.total as f32 } else { 0.0 };
                ui.vertical(|ui| {
                    ui.add_space(4.0);
                    let phase = p.phase.map(|ph| ph.label()).unwrap_or("Starting…");
                    let detail = match (p.total, p.done) {
                        (0, 0) => phase.to_string(),
                        (0, d) => format!("{phase} · {d}"),
                        (t, d) => format!("{phase} · {d} / {t}"),
                    };
                    ui.label(RichText::new(detail).size(12.0).color(MUTED));
                    ui.add(egui::ProgressBar::new(frac).animate(p.total == 0).desired_width(320.0).desired_height(6.0));
                });
            } else if let Some(s) = &self.stats {
                chip(ui, &format!("{} images", s.scanned), MUTED);
                chip(ui, &format!("{} groups", self.groups.len()), ACCENT);
                chip(ui, &format!("{} similar", self.visible_pics()), TEAL);
                let marked = self.marked_count();
                if marked > 0 {
                    chip(ui, &format!("{marked} marked"), DANGER);
                }
                if s.failed > 0 {
                    chip(ui, &format!("{} unreadable", s.failed), MUTED);
                }
                ui.label(RichText::new(format!("{:.1} s", s.seconds)).size(11.0).color(MUTED));
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                self.scan_button(ui, 110.0);
                if ui.add(egui::Button::new("About").min_size(Vec2::new(0.0, 34.0))).clicked() {
                    self.show_about = true;
                }
                if ui.add(egui::Button::new("Settings").min_size(Vec2::new(0.0, 34.0))).clicked() {
                    self.show_settings = true;
                }
            });
        });
    }

    /// Start/Stop button, shown in the header and in the sidebar.
    fn scan_button(&mut self, ui: &mut egui::Ui, width: f32) {
        let size = Vec2::new(width, 34.0);
        if self.running() {
            let b = egui::Button::new(RichText::new("Stop").strong().color(Color32::WHITE)).fill(DANGER);
            if ui.add_sized(size, b).clicked() {
                self.cancel.store(true, Ordering::Relaxed);
            }
        } else {
            let b = egui::Button::new(RichText::new("Start scan").strong().color(Color32::WHITE)).fill(ACCENT);
            let enabled = !self.folders.is_empty();
            let r = ui.add_enabled_ui(enabled, |ui| ui.add_sized(size, b)).inner;
            if r.clicked() {
                self.start_scan();
            }
            r.on_disabled_hover_text("Add a folder first");
        }
    }

    // ---------- Sidebar ----------
    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        section(ui, "FOLDERS");
        let mut remove = None;
        for (i, f) in self.folders.iter().enumerate() {
            Frame::new().fill(CARD).corner_radius(8).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width() - 26.0);
                        let name = f
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| f.display().to_string());
                        ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
                        ui.add(
                            egui::Label::new(
                                RichText::new(tail_path(&f.display().to_string(), 38)).size(11.0).color(MUTED),
                            )
                            .truncate(),
                        )
                        .on_hover_text(f.display().to_string());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !self.running() && ui.small_button("✕").on_hover_text("Remove folder").clicked() {
                            remove = Some(i);
                        }
                    });
                });
            });
            ui.add_space(4.0);
        }
        if let Some(i) = remove {
            self.folders.remove(i);
        }
        if self.folders.is_empty() {
            ui.label(RichText::new("No folder selected yet.").color(MUTED).italics());
        }
        ui.add_space(4.0);
        ui.add_enabled_ui(!self.running(), |ui| {
            if ui.add_sized([ui.available_width(), 30.0], egui::Button::new("+  Add folder")).clicked()
                && let Some(dirs) = rfd::FileDialog::new().set_title("Choose folders").pick_folders()
            {
                for d in dirs {
                    if !self.folders.contains(&d) {
                        self.folders.push(d);
                    }
                }
            }
        });

        ui.add_space(22.0);
        section(ui, "SIMILARITY");
        ui.add_enabled_ui(!self.running(), |ui| {
            let label = match self.level {
                0..=2 => "Strict",
                3..=5 => "Normal",
                _ => "Loose",
            };
            ui.horizontal(|ui| {
                ui.label("Match level");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(label).color(ACCENT).strong());
                });
            });
            ui.spacing_mut().slider_width = ui.available_width() - 8.0;
            if ui.add(egui::Slider::new(&mut self.level, 0..=Thresholds::STEPS).show_value(false)).changed() {
                self.thresholds = Thresholds::from_level(self.level);
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("strict").size(11.0).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("loose").size(11.0).color(MUTED));
                });
            });
            if self.custom_thresholds() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Custom thresholds active").size(11.0).color(TEAL));
                    if ui.small_button("Reset").clicked() {
                        self.thresholds = Thresholds::from_level(self.level);
                    }
                });
            }
            ui.add_space(10.0);
            ui.checkbox(&mut self.rotations, "Find images rotated by 90°");
            ui.checkbox(&mut self.background, "Give plain backgrounds less weight");
            ui.checkbox(&mut self.only_across_roots, "Only compare across folders");
        });

        ui.add_space(16.0);
        self.scan_button(ui, ui.available_width());

        if !self.groups.is_empty() {
            ui.add_space(22.0);
            section(ui, "SELECTION");
            let w = ui.available_width();
            egui::ComboBox::from_id_salt("keep_rule").width(w - 8.0).selected_text(self.keep_rule.label()).show_ui(
                ui,
                |ui| {
                    for r in KeepRule::ALL {
                        ui.selectable_value(&mut self.keep_rule, r, r.label()).on_hover_text(r.hint());
                    }
                },
            );
            ui.label(RichText::new(self.keep_rule.hint()).size(11.0).color(MUTED));
            ui.add_space(4.0);
            if ui.add_sized([w, 28.0], egui::Button::new("Auto-mark")).clicked() {
                self.auto_mark();
            }
            if ui.add_sized([w, 28.0], egui::Button::new("Clear marks")).clicked() {
                self.groups.iter_mut().flatten().for_each(|p| p.marked = false);
            }
            let n = self.marked_count();
            ui.add_space(6.0);
            let b = egui::Button::new(RichText::new(format!("Move {n} to trash")).color(Color32::WHITE))
                .fill(if n > 0 { DANGER } else { CARD })
                .min_size(Vec2::new(w, 32.0));
            if ui.add_enabled(n > 0, b).clicked() {
                self.confirm_trash = true;
            }
        }

        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(12.0);
            ui.label(RichText::new("Space = mark · Del = trash · double-click = open").size(10.5).color(MUTED));
            ui.add(egui::Slider::new(&mut self.thumb_size, 110.0..=260.0).text("Preview size").show_value(false));
        });
    }

    // ---------- Groups ----------
    fn groups_view(&mut self, ui: &mut egui::Ui) {
        if self.groups.is_empty() {
            ui.centered_and_justified(|ui| {
                let text = if self.running() {
                    "Scanning…"
                } else if self.stats.is_some() {
                    "No similar images found"
                } else if self.folders.is_empty() {
                    "Add a folder (or drop one here) to get started"
                } else {
                    "Press “Start scan”"
                };
                ui.label(RichText::new(text).size(18.0).color(MUTED));
            });
            return;
        }
        let ctx = ui.ctx().clone();
        let size = self.thumb_size;
        let mut clicked: Option<(usize, usize, bool)> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.add_space(14.0);
            for (gi, group) in self.groups.iter_mut().enumerate() {
                let visible = group.iter().filter(|p| !p.removed).count();
                let marked = group.iter().filter(|p| p.marked && !p.removed).count();
                Frame::new().fill(PANEL).corner_radius(12).inner_margin(Margin::same(14)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Group {}", gi + 1)).strong().size(15.0));
                        ui.label(RichText::new(format!("· {visible} images")).color(MUTED));
                        if marked > 0 {
                            ui.label(RichText::new(format!("· {marked} marked")).color(DANGER));
                        }
                    });
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::splat(10.0);
                        for (pi, pic) in group.iter_mut().enumerate() {
                            if pic.removed {
                                continue;
                            }
                            let r = thumb_card(ui, &ctx, pic, size, pi == 0, self.selected == Some((gi, pi)));
                            if r.double_clicked() {
                                let _ = open::that_detached(&pic.m.path);
                            } else if r.secondary_clicked() {
                                clicked = Some((gi, pi, true));
                            } else if r.clicked() {
                                let toggle = ui.input(|i| i.modifiers.command || i.modifiers.shift);
                                clicked = Some((gi, pi, toggle));
                            }
                        }
                    });
                });
                ui.add_space(12.0);
            }
        });
        if let Some((gi, pi, toggle)) = clicked {
            let pic = &mut self.groups[gi][pi];
            if toggle || self.selected == Some((gi, pi)) {
                pic.marked = !pic.marked;
            }
            self.selected = Some((gi, pi));
        }
    }

    // ---------- Details ----------
    fn details(&mut self, ui: &mut egui::Ui) {
        let Some((gi, pi)) = self.selected else {
            ui.add_space(18.0);
            section(ui, "DETAILS");
            ui.label(
                RichText::new("Click an image to see its details.\nClick again or right-click to mark it.\nDouble-click opens it.")
                    .color(MUTED),
            );
            return;
        };
        let ctx = ui.ctx().clone();
        let Some(pic) = self.groups.get_mut(gi).and_then(|g| g.get_mut(pi)) else { return };
        ui.add_space(14.0);
        let w = ui.available_width();
        if let Some(tex) = texture(&ctx, pic) {
            let s = tex.size_vec2();
            let scale = (w / s.x).min(320.0 / s.y);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(&tex).fit_to_exact_size(s * scale).corner_radius(8));
            });
        }
        ui.add_space(10.0);
        ui.add(egui::Label::new(RichText::new(file_name(&pic.m.path)).strong().size(15.0)).wrap());
        ui.add(
            egui::Label::new(RichText::new(parent_dir(&pic.m.path)).size(11.0).color(MUTED)).wrap().selectable(true),
        );
        ui.add_space(8.0);
        egui::Grid::new("info").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
            ui.label(RichText::new("Resolution").color(MUTED));
            ui.label(format!("{} × {}", pic.m.width, pic.m.height));
            ui.end_row();
            ui.label(RichText::new("File size").color(MUTED));
            ui.label(human_size(pic.m.bytes));
            ui.end_row();
            ui.label(RichText::new("Modified").color(MUTED));
            ui.label(format_date(pic.m.modified));
            ui.end_row();
            if let Some(d) = pic.m.distance {
                ui.label(RichText::new("Similarity").color(MUTED));
                ui.label(format!("{:.1} %", (1.0 - d) * 100.0));
                ui.end_row();
            }
        });
        ui.add_space(12.0);
        let w = ui.available_width();
        let (txt, fill) = if pic.marked { ("Unmark", CARD) } else { ("Mark for trash", DANGER) };
        if ui
            .add(egui::Button::new(RichText::new(txt).color(Color32::WHITE)).fill(fill).min_size(Vec2::new(w, 30.0)))
            .clicked()
        {
            pic.marked = !pic.marked;
        }
        if ui.add_sized([w, 28.0], egui::Button::new("Open")).clicked() {
            let _ = open::that_detached(&pic.m.path);
        }
        if ui.add_sized([w, 28.0], egui::Button::new("Show in folder")).clicked()
            && let Some(dir) = pic.m.path.parent()
        {
            let _ = open::that_detached(dir);
        }
    }

    // ---------- Dialogs ----------
    fn settings_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::unique("settings")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Settings").strong().size(18.0));
            ui.add_space(12.0);
            section(ui, "ADVANCED THRESHOLDS");
            ui.label(
                RichText::new(
                    "The match level slider sets these values. Change them only if you want to fine-tune matching.",
                )
                .size(12.0)
                .color(MUTED),
            );
            ui.add_space(6.0);
            ui.add_enabled_ui(!self.running(), |ui| {
                let t = &mut self.thresholds;
                let (s, p, tl) = (Thresholds::SENS, Thresholds::PREC, Thresholds::TOL);
                ui.spacing_mut().slider_width = 240.0;
                ui.add(egui::Slider::new(&mut t.sensibility, s.0..=s.2).text("Max. distance"))
                    .on_hover_text("Maximum average difference (value / 400)");
                ui.add(egui::Slider::new(&mut t.precision, p.0..=p.2).text("Pixel tolerance"))
                    .on_hover_text("A channel difference above this counts as a strongly differing point");
                ui.add(egui::Slider::new(&mut t.tolerance, tl.0..=tl.2).text("Allowed outliers"))
                    .on_hover_text("How many of the 2500 grid points may differ strongly");
                if ui.button("Reset to match level").clicked() {
                    *t = Thresholds::from_level(self.level);
                }
            });
            ui.add_space(14.0);
            section(ui, "VIEW");
            ui.add(egui::Slider::new(&mut self.thumb_size, 110.0..=260.0).text("Preview size"));
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                if ui.add(egui::Button::new(RichText::new("Done").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    self.show_settings = false;
                }
            });
        });
        if modal.should_close() {
            self.show_settings = false;
        }
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        let modal = egui::Modal::new(egui::Id::unique("about")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new(&self.logo).fit_to_exact_size(Vec2::splat(96.0)));
                ui.add_space(6.0);
                ui.label(RichText::new(APP_NAME).strong().size(22.0));
                ui.label(RichText::new(format!("Version {VERSION}")).color(MUTED));
                ui.add_space(10.0);
                ui.label("Finds similar and duplicate images – even when they were resized, recompressed, slightly cropped, brightened or rotated.");
                ui.add_space(8.0);
                ui.label(RichText::new("Matching approach inspired by VisiPics.").size(12.0).color(MUTED));
                ui.label(RichText::new("Released under the MIT License.").size(12.0).color(MUTED));
                ui.add_space(12.0);
                if ui.add(egui::Button::new(RichText::new("Close").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    self.show_about = false;
                }
            });
        });
        if modal.should_close() {
            self.show_about = false;
        }
    }

    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let n = self.marked_count();
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new("Move to trash?").strong().size(17.0));
            ui.add_space(8.0);
            ui.label(format!("{n} marked files will be moved to the trash. You can restore them from there."));
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(RichText::new("Move to trash").color(Color32::WHITE)).fill(DANGER))
                    .clicked()
                {
                    self.confirm_trash = false;
                    self.trash_marked(ctx);
                }
                if ui.button("Cancel").clicked() {
                    self.confirm_trash = false;
                }
            });
        });
        if modal.should_close() {
            self.confirm_trash = false;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.show_settings || self.show_about || self.confirm_trash || ctx.text_edit_focused() {
            return;
        }
        let (space, del) = ctx.input(|i| (i.key_pressed(Key::Space), i.key_pressed(Key::Delete)));
        if space
            && let Some((gi, pi)) = self.selected
            && let Some(p) = self.groups.get_mut(gi).and_then(|g| g.get_mut(pi))
        {
            p.marked = !p.marked;
        }
        if del && self.marked_count() > 0 {
            self.confirm_trash = true;
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.shortcuts(ctx);

        // Folders via drag & drop
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if !self.running() {
            for p in dropped {
                let dir = if p.is_dir() { Some(p) } else { p.parent().map(Path::to_path_buf) };
                if let Some(d) = dir.filter(|d| !self.folders.contains(d)) {
                    self.folders.push(d);
                }
            }
        }

        egui::Panel::top("header")
            .exact_size(66.0)
            .resizable(false)
            .frame(Frame::new().fill(HEADER).inner_margin(Margin::symmetric(18, 8)).stroke(Stroke::new(1.0, BORDER)))
            .show(ui, |ui| self.header(ui));

        egui::Panel::left("side")
            .exact_size(290.0)
            .resizable(false)
            .frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(16, 8)))
            .show(ui, |ui| self.sidebar(ui));

        if !self.groups.is_empty() {
            egui::Panel::right("details")
                .default_size(300.0)
                .frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric(16, 8)))
                .show(ui, |ui| self.details(ui));
        }

        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(16, 0)))
            .show(ui, |ui| self.groups_view(ui));

        if self.show_settings {
            self.settings_dialog(ctx);
        }
        if self.show_about {
            self.about_dialog(ctx);
        }
        if self.confirm_trash {
            self.trash_dialog(ctx);
        }

        if let Some((text, color, until)) = &self.toast {
            if ctx.input(|i| i.time) > *until {
                self.toast = None;
            } else {
                egui::Area::new(egui::Id::unique("toast")).anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0]).show(
                    ctx,
                    |ui| {
                        Frame::new()
                            .fill(CARD)
                            .stroke(Stroke::new(1.0, *color))
                            .corner_radius(10)
                            .inner_margin(Margin::symmetric(16, 10))
                            .show(ui, |ui| {
                                ui.label(RichText::new(text.as_str()).color(*color));
                            });
                    },
                );
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
    }
}

fn texture(ctx: &egui::Context, pic: &mut Pic) -> Option<TextureHandle> {
    if pic.tex.is_none() {
        let t = pic.m.thumb.as_ref()?;
        let img = ColorImage::from_rgba_unmultiplied([t.width() as usize, t.height() as usize], t.as_raw());
        pic.tex = Some(ctx.load_texture(pic.m.path.to_string_lossy(), img, TextureOptions::LINEAR));
    }
    pic.tex.clone()
}

fn thumb_card(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    pic: &mut Pic,
    size: f32,
    best: bool,
    selected: bool,
) -> egui::Response {
    let stroke = if pic.marked {
        Stroke::new(2.0, DANGER)
    } else if selected {
        Stroke::new(2.0, ACCENT)
    } else {
        Stroke::new(2.0, BORDER)
    };
    let tex = texture(ctx, pic);
    let resp = Frame::new().fill(CARD).stroke(stroke).corner_radius(10).inner_margin(Margin::same(8)).show(ui, |ui| {
        ui.set_width(size);
        ui.vertical(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(6), Color32::from_gray(22));
            if let Some(tex) = &tex {
                let s = tex.size_vec2();
                let scale = (size / s.x).min(size / s.y);
                let r = egui::Rect::from_center_size(rect.center(), s * scale);
                egui::Image::new(tex).corner_radius(6).paint_at(ui, r);
            } else {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "?",
                    egui::FontId::proportional(28.0),
                    MUTED,
                );
            }
            if pic.marked {
                badge(ui, rect.left_top() + Vec2::new(6.0, 6.0), "marked", DANGER);
            } else if best {
                badge(ui, rect.left_top() + Vec2::new(6.0, 6.0), "largest", OK);
            }
            ui.add_space(6.0);
            ui.add(egui::Label::new(RichText::new(file_name(&pic.m.path)).size(12.0).strong()).truncate());
            let dir = tail_path(&parent_dir(&pic.m.path), (size / 6.0) as usize);
            ui.add(egui::Label::new(RichText::new(dir).size(10.5).color(MUTED)).truncate());
            ui.label(
                RichText::new(format!("{}×{} · {}", pic.m.width, pic.m.height, human_size(pic.m.bytes)))
                    .size(11.0)
                    .color(MUTED),
            );
            ui.label(RichText::new(format_date(pic.m.modified)).size(10.5).color(MUTED));
        });
    });
    resp.response.interact(Sense::click()).on_hover_text(pic.m.path.display().to_string())
}

fn badge(ui: &egui::Ui, pos: egui::Pos2, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
    let rect = egui::Rect::from_min_size(pos, galley.size() + Vec2::new(12.0, 4.0));
    ui.painter().rect_filled(rect, CornerRadius::same(6), color);
    ui.painter().galley(rect.min + Vec2::new(6.0, 2.0), galley, Color32::WHITE);
}

fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), egui::FontId::proportional(12.0), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + Vec2::new(20.0, 8.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(12),
        color.gamma_multiply(0.14),
        Stroke::new(1.0, color.gamma_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + Vec2::new(10.0, 4.0), galley, color);
}

/// Shortens a path from the front so the end stays visible: `…/photos/2024`.
fn tail_path(path: &str, max_chars: usize) -> String {
    if path.chars().count() <= max_chars {
        return path.to_string();
    }
    // Windows paths may use `\` or `/`; keep whichever the path itself uses.
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let mut tail = String::new();
    for part in path.rsplit(['/', '\\']).filter(|s| !s.is_empty()) {
        let candidate = if tail.is_empty() { part.to_string() } else { format!("{part}{sep}{tail}") };
        if candidate.chars().count() + 2 > max_chars {
            break;
        }
        tail = candidate;
    }
    if tail.is_empty() {
        let last: String = path.chars().rev().take(max_chars.saturating_sub(1)).collect();
        return format!("…{}", last.chars().rev().collect::<String>());
    }
    format!("…{sep}{tail}")
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).size(11.0).color(MUTED).strong());
    ui.add_space(6.0);
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn parent_dir(p: &Path) -> String {
    p.parent().map(|d| d.display().to_string()).unwrap_or_default()
}

fn format_date(t: Option<SystemTime>) -> String {
    t.map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "–".into())
}

fn human_size(b: u64) -> String {
    match b {
        0..1024 => format!("{b} B"),
        1024..1_048_576 => format!("{:.0} KB", b as f64 / 1024.0),
        _ => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(12);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_fill = Color32::from_rgb(45, 50, 62);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(42, 47, 59);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(12.0, 5.0);
    });
}

#[cfg(test)]
mod tests {
    use super::tail_path;

    #[test]
    fn tail_path_keeps_the_end() {
        assert_eq!(tail_path("/a/b", 10), "/a/b");
        assert_eq!(tail_path("/home/user/photos/2024/summer", 16), "…/2024/summer");
        assert_eq!(tail_path("/x/averyveryverylongfoldername", 10), "…oldername");
        assert_eq!(tail_path(r"C:\Users\me\Pictures\2024\summer", 16), r"…\2024\summer");
    }
}
