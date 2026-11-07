//! Comparison engine.
//!
//! The approach follows the classic VisiPics matcher:
//! - only compare images whose brightness keys are close,
//! - reject pairs whose average colours differ too much,
//! - run a coarse test on 25 samples, then a fine test on all 2500 grid points,
//! - optionally try both 90° rotations,
//! - merge matches into groups.

use crate::signature::{GRID, Signature};

/// Largest per-pixel difference: 3·255 (channels) + 255 (brightness).
const MAX_PX: i64 = 1020;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// 5..=100; maximum distance = value / 400.
    pub sensibility: i32,
    /// 10..=75; per-channel pixel tolerance.
    pub precision: i32,
    /// 20..=875; how many of the 2500 grid points may differ strongly.
    pub tolerance: i32,
}

impl Thresholds {
    pub const SENS: (i32, i32, i32) = (5, 27, 100);
    pub const PREC: (i32, i32, i32) = (10, 50, 75);
    pub const TOL: (i32, i32, i32) = (20, 150, 875);
    pub const STEPS: i32 = 8;

    /// Main slider: 0 = strict, 4 = normal, 8 = loose.
    pub fn from_level(level: i32) -> Self {
        let level = level.clamp(0, Self::STEPS);
        let half = Self::STEPS / 2;
        let map = |(lo, mid, hi): (i32, i32, i32)| {
            if level <= half { lo + (mid - lo) * level / half } else { mid + (hi - mid) * (level - half) / half }
        };
        Self { sensibility: map(Self::SENS), precision: map(Self::PREC), tolerance: map(Self::TOL) }
    }

    pub fn max_distance(&self) -> f64 {
        self.sensibility as f64 / 400.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub thresholds: Thresholds,
    /// Also detect images rotated by 90°.
    pub rotations: bool,
    /// Give plain background areas less weight.
    pub background: bool,
    /// Only compare images from different root folders.
    pub only_across_roots: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Distance {
    /// Normalised distance 0.0..=1.0
    pub value: f64,
    /// Number of strongly differing points, scaled to 2500.
    pub strong: i32,
}

/// Rotation 0 = unchanged, 1 and 2 = the two 90° rotations of `b`.
pub fn distance(a: &Signature, b: &Signature, rot: u8, coarse: bool, use_bg: bool, tol: i32) -> Distance {
    let b = b.rotated(rot);
    let step = if coarse { 10 } else { 1 };
    let mut sum: i64 = 0;
    let mut strong: i32 = 0;
    let mut bg: i32 = 0;

    let near = |v: u8, center: u8| (center as i32 - tol) < v as i32 && (v as i32) < center as i32 + tol;

    for row in (0..GRID).step_by(step) {
        for col in (0..GRID).step_by(step) {
            let pa = a.px(row, col);
            let pb = b.px(row, col);
            if use_bg && (0..3).all(|c| near(pa[c], a.bg[c]) && near(pb[c], b.bg[c])) {
                bg += 1;
            }
            let d = [pa[0] as i32 - pb[0] as i32, pa[1] as i32 - pb[1] as i32, pa[2] as i32 - pb[2] as i32];
            let lum = (d[0] + d[1] + d[2]).abs() / 3;
            sum += (d[0].abs() + d[1].abs() + d[2].abs() + lum) as i64;
            if !d.iter().all(|x| x.abs() < tol) {
                strong += 1;
            }
        }
    }

    // Background points count only half.
    bg /= 2;
    let samples: i64 = if coarse { 25 } else { 2500 };
    let denom = (samples - bg as i64) * MAX_PX;
    let value = sum as f64 / denom as f64;
    let scaled = (strong as f64 * (2500.0 / (2500 - bg) as f64)).round_ties_even() as i32;
    Distance { value, strong: scaled }
}

/// Colour pre-filter, then coarse → fine test for each rotation.
/// Returns the fine distance when the images count as similar.
pub fn is_match(a: &Signature, b: &Signature, opt: &Options) -> Option<Distance> {
    let t = &opt.thresholds;
    let avg_tol = t.precision / 3;
    let max_d = t.max_distance();
    let rots: &[u8] = if opt.rotations { &[0, 1, 2] } else { &[0] };
    for &rot in rots {
        let br = b.rotated(rot);
        if (0..3).any(|c| (a.avg[c] as i32 - br.avg[c] as i32).abs() >= avg_tol) {
            continue;
        }
        let c = distance(a, b, rot, true, opt.background, t.precision + 8);
        if c.value > max_d + 0.1 || c.strong > (t.tolerance + 40) / 9 {
            continue;
        }
        let f = distance(a, b, rot, false, opt.background, t.precision);
        if f.value <= max_d && f.strong <= t.tolerance {
            return Some(f);
        }
    }
    None
}

/// Union-find used to merge matches into groups.
pub struct Groups {
    parent: Vec<usize>,
}

impl Groups {
    pub fn new(n: usize) -> Self {
        Self { parent: (0..n).collect() }
    }
    pub fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }
    pub fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra.max(rb)] = ra.min(rb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn gradient(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, ((x + y) * 127 / (w + h)) as u8]))
    }

    fn opts(level: i32, rotations: bool) -> Options {
        Options { thresholds: Thresholds::from_level(level), rotations, background: true, only_across_roots: false }
    }

    #[test]
    fn slider_levels() {
        assert_eq!(Thresholds::from_level(0), Thresholds { sensibility: 5, precision: 10, tolerance: 20 });
        assert_eq!(Thresholds::from_level(4), Thresholds { sensibility: 27, precision: 50, tolerance: 150 });
        assert_eq!(Thresholds::from_level(8), Thresholds { sensibility: 100, precision: 75, tolerance: 875 });
    }

    #[test]
    fn identical_and_resized_images_match() {
        let a = Signature::from_image(&gradient(640, 480));
        let b = Signature::from_image(&gradient(320, 240));
        assert_eq!(distance(&a, &a, 0, false, false, 50).value, 0.0);
        assert!(is_match(&a, &b, &opts(4, false)).is_some());
    }

    #[test]
    fn rotated_image_needs_rotation_option() {
        let img = gradient(400, 400);
        let a = Signature::from_image(&img);
        let b = Signature::from_image(&image::imageops::rotate90(&img));
        assert!(is_match(&a, &b, &opts(4, false)).is_none());
        assert!(is_match(&a, &b, &opts(4, true)).is_some());
    }

    #[test]
    fn different_images_do_not_match() {
        let a = Signature::from_image(&gradient(300, 300));
        let b = Signature::from_image(&RgbImage::from_fn(300, 300, |x, y| {
            if (x / 30 + y / 30) % 2 == 0 { Rgb([250, 20, 20]) } else { Rgb([10, 10, 200]) }
        }));
        assert!(is_match(&a, &b, &opts(8, true)).is_none());
    }

    #[test]
    fn groups_merge_transitively() {
        let mut g = Groups::new(4);
        g.union(0, 1);
        g.union(2, 1);
        assert_eq!(g.find(2), g.find(0));
        assert_ne!(g.find(3), g.find(0));
    }
}
