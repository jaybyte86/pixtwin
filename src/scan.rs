//! Background scan: find files → build signatures → compare → group → thumbnails.

use crate::compare::{Groups, Options, is_match};
use crate::signature::Signature;
use crossbeam_channel::Sender;
use image::{DynamicImage, ImageReader, RgbaImage};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "tif", "tiff", "webp", "tga", "ico", "pnm", "ppm", "pgm", "pbm",
    "qoi",
];
const THUMB: u32 = 240;
/// A downscaled copy is enough for the signature (area averages are preserved).
const WORK_SIZE: u32 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Searching,
    Loading,
    Comparing,
    Thumbnails,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Searching => "Finding files",
            Phase::Loading => "Analysing images",
            Phase::Comparing => "Comparing",
            Phase::Thumbnails => "Building previews",
        }
    }
}

struct Picture {
    path: PathBuf,
    root: usize,
    width: u32,
    height: u32,
    bytes: u64,
    modified: Option<SystemTime>,
    sig: Signature,
}

pub struct Member {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
    pub thumb: Option<RgbaImage>,
    /// Distance to the first (largest) image of the group, if compared directly.
    pub distance: Option<f64>,
}

pub enum Msg {
    Progress { phase: Phase, done: usize, total: usize },
    Finished(ScanResult),
    Cancelled,
}

pub struct ScanResult {
    pub groups: Vec<Vec<Member>>,
    pub scanned: usize,
    pub failed: usize,
    pub elapsed: Duration,
}

pub fn start(roots: Vec<PathBuf>, opt: Options, cancel: Arc<AtomicBool>, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let started = Instant::now();
        match run(&roots, &opt, &cancel, &tx) {
            Some(mut res) => {
                res.elapsed = started.elapsed();
                let _ = tx.send(Msg::Finished(res));
            }
            None => {
                let _ = tx.send(Msg::Cancelled);
            }
        }
    });
}

fn is_picture(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn decode(path: &Path) -> Option<DynamicImage> {
    ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()
}

fn run(roots: &[PathBuf], opt: &Options, cancel: &AtomicBool, tx: &Sender<Msg>) -> Option<ScanResult> {
    let progress = |phase, done, total| {
        let _ = tx.send(Msg::Progress { phase, done, total });
    };
    let cancelled = || cancel.load(Ordering::Relaxed);

    // 1. Collect files (skip duplicates caused by nested root folders)
    progress(Phase::Searching, 0, 0);
    let mut files: Vec<(PathBuf, usize)> = Vec::new();
    let mut seen = HashSet::new();
    for (ri, root) in roots.iter().enumerate() {
        for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter().flatten() {
            if cancelled() {
                return None;
            }
            let p = entry.path();
            if entry.file_type().is_file() && is_picture(p) && seen.insert(p.to_path_buf()) {
                files.push((p.to_path_buf(), ri));
                if files.len().is_multiple_of(200) {
                    progress(Phase::Searching, files.len(), 0);
                }
            }
        }
    }

    // 2. Build signatures in parallel
    let total = files.len();
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    progress(Phase::Loading, 0, total);
    let pictures: Vec<Picture> = files
        .par_iter()
        .filter_map(|(path, root)| {
            if cancelled() {
                return None;
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_multiple_of(16) || n == total {
                progress(Phase::Loading, n, total);
            }
            let Some(img) = decode(path).filter(|i| i.width() > 0 && i.height() > 0) else {
                failed.fetch_add(1, Ordering::Relaxed);
                return None;
            };
            let (width, height) = (img.width(), img.height());
            let small = if width > WORK_SIZE || height > WORK_SIZE { img.thumbnail(WORK_SIZE, WORK_SIZE) } else { img };
            let sig = Signature::from_image(&small.to_rgb8());
            let meta = std::fs::metadata(path).ok();
            Some(Picture {
                path: path.clone(),
                root: *root,
                width,
                height,
                bytes: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                modified: meta.and_then(|m| m.modified().ok()),
                sig,
            })
        })
        .collect();
    if cancelled() {
        return None;
    }

    // 3. Compare: sort by brightness key and only test neighbours whose
    //    keys differ by less than the pixel tolerance.
    let mut order: Vec<usize> = (0..pictures.len()).collect();
    order.sort_by_key(|&i| pictures[i].sig.key);
    let window = opt.thresholds.precision as u16;
    let n = order.len();
    let done = AtomicUsize::new(0);
    progress(Phase::Comparing, 0, n);
    let pairs: Vec<(usize, usize, f64)> = (0..n)
        .into_par_iter()
        .flat_map_iter(|oi| {
            let mut found = Vec::new();
            if cancelled() {
                return found.into_iter();
            }
            let a = &pictures[order[oi]];
            for &bj in &order[oi + 1..] {
                let b = &pictures[bj];
                if b.sig.key - a.sig.key >= window {
                    break;
                }
                if opt.only_across_roots && a.root == b.root {
                    continue;
                }
                if let Some(d) = is_match(&a.sig, &b.sig, opt) {
                    found.push((order[oi], bj, d.value));
                }
            }
            let c = done.fetch_add(1, Ordering::Relaxed) + 1;
            if c.is_multiple_of(64) || c == n {
                progress(Phase::Comparing, c, n);
            }
            found.into_iter()
        })
        .collect();
    if cancelled() {
        return None;
    }

    // 4. Build groups, largest image first (resolution, then file size)
    let mut uf = Groups::new(pictures.len());
    for &(a, b, _) in &pairs {
        uf.union(a, b);
    }
    let mut by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..pictures.len() {
        by_root.entry(uf.find(i)).or_default().push(i);
    }
    let mut raw_groups: Vec<Vec<usize>> = by_root.into_values().filter(|g| g.len() > 1).collect();
    for g in &mut raw_groups {
        g.sort_by_key(|&i| {
            let p = &pictures[i];
            std::cmp::Reverse((p.width as u64 * p.height as u64, p.bytes))
        });
    }
    raw_groups.sort_by(|a, b| b.len().cmp(&a.len()).then(pictures[a[0]].path.cmp(&pictures[b[0]].path)));
    let pair_dist: HashMap<(usize, usize), f64> = pairs.iter().map(|&(a, b, d)| ((a.min(b), a.max(b)), d)).collect();

    // 5. Thumbnails only for group members
    let members: Vec<usize> = raw_groups.iter().flatten().copied().collect();
    let total_thumbs = members.len();
    let done = AtomicUsize::new(0);
    progress(Phase::Thumbnails, 0, total_thumbs);
    let mut thumbs: HashMap<usize, RgbaImage> = members
        .par_iter()
        .filter_map(|&i| {
            if cancelled() {
                return None;
            }
            let c = done.fetch_add(1, Ordering::Relaxed) + 1;
            if c.is_multiple_of(8) || c == total_thumbs {
                progress(Phase::Thumbnails, c, total_thumbs);
            }
            decode(&pictures[i].path).map(|img| (i, img.thumbnail(THUMB, THUMB).to_rgba8()))
        })
        .collect();
    if cancelled() {
        return None;
    }

    let groups = raw_groups
        .iter()
        .map(|g| {
            let first = g[0];
            g.iter()
                .map(|&i| {
                    let p = &pictures[i];
                    Member {
                        path: p.path.clone(),
                        width: p.width,
                        height: p.height,
                        bytes: p.bytes,
                        modified: p.modified,
                        thumb: thumbs.remove(&i),
                        distance: if i == first {
                            Some(0.0)
                        } else {
                            pair_dist.get(&(first.min(i), first.max(i))).copied()
                        },
                    }
                })
                .collect()
        })
        .collect();

    Some(ScanResult {
        groups,
        scanned: pictures.len(),
        failed: failed.load(Ordering::Relaxed),
        elapsed: Duration::ZERO,
    })
}
