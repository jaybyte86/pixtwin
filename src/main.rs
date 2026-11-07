#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod compare;
mod scan;
mod signature;

use std::path::PathBuf;

pub const APP_NAME: &str = "PixTwin";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const LOGO_PNG: &[u8] = include_bytes!("../assets/logo-256.png");

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--cli") => {
            run_cli(&args[1..]);
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("{APP_NAME} {VERSION}");
            return Ok(());
        }
        Some("--help" | "-h") => {
            println!("{}", HELP);
            return Ok(());
        }
        _ => {}
    }

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id("pixtwin")
        .with_inner_size([1380.0, 880.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(LOGO_PNG) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(APP_NAME, options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

const HELP: &str = "\
PixTwin – find similar and duplicate images

Usage:
  pixtwin [FOLDER…]                 open the window, optionally with folders preselected
  pixtwin --cli [OPTIONS] FOLDER…   scan without a window and print the groups

CLI options:
  --level 0..8   similarity: 0 = strict, 4 = normal (default), 8 = loose
  --rot          also find images rotated by 90°
  --no-bg        do not give plain backgrounds less weight
  --across       only compare images from different folders
  -V, --version  print the version";

/// Headless mode: prints the groups as text.
fn run_cli(args: &[String]) {
    let mut level = 4;
    let mut opt = compare::Options {
        thresholds: compare::Thresholds::from_level(level),
        rotations: false,
        background: true,
        only_across_roots: false,
    };
    let mut roots = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--level" => level = it.next().and_then(|v| v.parse().ok()).unwrap_or(4),
            "--rot" => opt.rotations = true,
            "--no-bg" => opt.background = false,
            "--across" => opt.only_across_roots = true,
            p => roots.push(PathBuf::from(p)),
        }
    }
    if roots.is_empty() {
        eprintln!("{HELP}");
        std::process::exit(2);
    }
    opt.thresholds = compare::Thresholds::from_level(level);

    let (tx, rx) = crossbeam_channel::unbounded();
    scan::start(roots, opt, Default::default(), tx);
    for msg in rx {
        if let scan::Msg::Finished(res) = msg {
            println!("{} images analysed, {} unreadable, {:.2} s", res.scanned, res.failed, res.elapsed.as_secs_f64());
            for (gi, g) in res.groups.iter().enumerate() {
                println!("Group {}:", gi + 1);
                for m in g {
                    let sim = m.distance.map(|d| format!("{:.1} %", (1.0 - d) * 100.0)).unwrap_or_else(|| "–".into());
                    println!("  {:>5}×{:<5} {:>8}  {}", m.width, m.height, sim, m.path.display());
                }
            }
            if res.groups.is_empty() {
                println!("No similar images found.");
            }
            break;
        }
    }
}
