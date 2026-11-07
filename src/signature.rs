//! Image signature: a 50×50 grid with separate R/G/B planes.
//!
//! Steps:
//! 1. Box-average the image into 50×50 cells.
//! 2. Stretch contrast per row (columns 4..=45 only), then per column.
//! 3. Derive the average colour, a brightness key and the most frequent colour (background).

use image::RgbImage;

pub const GRID: usize = 50;
pub const CELLS: usize = GRID * GRID;

#[derive(Clone)]
pub struct Signature {
    /// R, G and B planes, `CELLS` bytes each, row by row.
    pub planes: [[u8; CELLS]; 3],
    /// Average colour after normalisation.
    pub avg: [u8; 3],
    /// Sort key: avg R + avg G + avg B (0..=765).
    pub key: u16,
    /// Most frequent grid colour, treated as background.
    pub bg: [u8; 3],
    /// Both 90° rotations, rotated *before* normalisation so that the
    /// asymmetric row/column contrast stretch does not break rotation matching.
    pub rotations: Option<Box<[Signature; 2]>>,
}

impl Signature {
    #[inline]
    pub fn px(&self, row: usize, col: usize) -> [u8; 3] {
        let i = row * GRID + col;
        [self.planes[0][i], self.planes[1][i], self.planes[2][i]]
    }

    pub fn from_image(img: &RgbImage) -> Self {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let mut planes = [[0u8; CELLS]; 3];

        // Each cell is the mean of all pixels inside it (at least one pixel for tiny images).
        let bounds = |cell: usize, size: usize| -> (usize, usize) {
            let a = (cell as f64 * size as f64 / GRID as f64).round() as usize;
            let b = ((cell + 1) as f64 * size as f64 / GRID as f64).round() as usize;
            let a = a.min(size - 1);
            (a, b.max(a + 1).min(size))
        };
        let raw = img.as_raw();
        for row in 0..GRID {
            let (y0, y1) = bounds(row, h);
            for col in 0..GRID {
                let (x0, x1) = bounds(col, w);
                let mut sum = [0u64; 3];
                for y in y0..y1 {
                    let line = &raw[(y * w + x0) * 3..(y * w + x1) * 3];
                    for p in line.chunks_exact(3) {
                        sum[0] += p[0] as u64;
                        sum[1] += p[1] as u64;
                        sum[2] += p[2] as u64;
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u64;
                let i = row * GRID + col;
                for c in 0..3 {
                    planes[c][i] = (sum[c] / n) as u8;
                }
            }
        }

        // Rotation 1: new[r][c] = old[49-c][r]; rotation 2: new[r][c] = old[c][49-r]
        let rotate = |f: &dyn Fn(usize, usize) -> usize| {
            let mut out = [[0u8; CELLS]; 3];
            for r in 0..GRID {
                for c in 0..GRID {
                    let src = f(r, c);
                    for p in 0..3 {
                        out[p][r * GRID + c] = planes[p][src];
                    }
                }
            }
            Self::finish(out, None)
        };
        let r1 = rotate(&|r, c| (GRID - 1 - c) * GRID + r);
        let r2 = rotate(&|r, c| c * GRID + (GRID - 1 - r));
        Self::finish(planes, Some(Box::new([r1, r2])))
    }

    fn finish(planes: [[u8; CELLS]; 3], rotations: Option<Box<[Signature; 2]>>) -> Self {
        let mut sig = Signature { planes, avg: [0; 3], key: 0, bg: [0; 3], rotations };
        sig.normalize();
        sig.compute_stats();
        sig
    }

    /// Rotation 0 = unchanged, 1 and 2 = the two 90° rotations.
    pub fn rotated(&self, rot: u8) -> &Signature {
        match (rot, &self.rotations) {
            (1, Some(r)) => &r[0],
            (2, Some(r)) => &r[1],
            _ => self,
        }
    }

    /// Stretches contrast row-wise (columns 4..=45 only), then column-wise.
    /// Skipped when the range already is 0..255 or is 10 or less.
    fn normalize(&mut self) {
        let stretch = |v: u8, lo: u8, range: u8| -> u8 { (((v as i32 - lo as i32) * 255) / range as i32) as u8 };

        for row in 0..GRID {
            let (mut lo, mut hi) = (255u8, 0u8);
            for col in 4..=45 {
                for c in self.px(row, col) {
                    lo = lo.min(c);
                    hi = hi.max(c);
                }
            }
            if lo >= hi || (lo == 0 && hi == 255) || hi - lo <= 10 {
                continue;
            }
            let range = hi - lo;
            for col in 4..=45 {
                let i = row * GRID + col;
                for c in 0..3 {
                    self.planes[c][i] = stretch(self.planes[c][i], lo, range);
                }
            }
        }

        for col in 0..GRID {
            let (mut lo, mut hi) = (255u8, 0u8);
            for row in 0..GRID {
                for c in self.px(row, col) {
                    lo = lo.min(c);
                    hi = hi.max(c);
                }
                if lo == 0 && hi == 255 {
                    break;
                }
            }
            if (lo == 0 && hi == 255) || hi < lo || hi - lo <= 10 {
                continue;
            }
            let range = hi - lo;
            for row in 0..GRID {
                let i = row * GRID + col;
                for c in 0..3 {
                    self.planes[c][i] = stretch(self.planes[c][i], lo, range);
                }
            }
        }
    }

    fn compute_stats(&mut self) {
        let mut sum = [0u32; 3];
        let mut colors: Vec<u32> = Vec::with_capacity(CELLS);
        for i in 0..CELLS {
            let (r, g, b) = (self.planes[0][i], self.planes[1][i], self.planes[2][i]);
            sum[0] += r as u32;
            sum[1] += g as u32;
            sum[2] += b as u32;
            colors.push(r as u32 | (g as u32) << 8 | (b as u32) << 16);
        }
        self.avg = sum.map(|s| (s / CELLS as u32) as u8);
        self.key = self.avg.iter().map(|&v| v as u16).sum();

        // Most frequent colour = longest run in the sorted list.
        colors.sort_unstable();
        let (mut best, mut best_len, mut run) = (colors[0], 0usize, 0usize);
        let mut prev = u32::MAX;
        for &c in &colors {
            run = if c == prev { run + 1 } else { 1 };
            if run > best_len {
                best_len = run;
                best = c;
            }
            prev = c;
        }
        self.bg = [best as u8, (best >> 8) as u8, (best >> 16) as u8];
    }
}
