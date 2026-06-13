//! Dithering algorithms used by the comparison sheet.
//!
//! Every method here turns a grey value into a pattern of **pure black and
//! pure white** pixels. That's the whole point on E-ink: a 2-level (black/white)
//! transition can use the panel's fast, non-flashing waveform, whereas any true
//! grey forces a waveform that drives through black to settle, which flashes.
//! Dithering buys back the illusion of grey while keeping every pixel bilevel,
//! so updates stay flash-free.
//!
//! Two families live here:
//! - *Ordered / noise* methods compare each pixel against a threshold matrix
//!   (Bayer, clustered-dot, white noise, blue noise). The pattern is a pure
//!   function of position, so the same grey always yields the same pixels —
//!   which on E-ink also means the fewest pixels flip between frames.
//! - *Error diffusion* methods (Floyd–Steinberg, Atkinson, …) push each pixel's
//!   quantisation error onto not-yet-visited neighbours.

use slint::Rgb8Pixel;

const BLACK: Rgb8Pixel = Rgb8Pixel { r: 0, g: 0, b: 0 };
const WHITE: Rgb8Pixel = Rgb8Pixel {
    r: 255,
    g: 255,
    b: 255,
};

fn grey_px(v: u8) -> Rgb8Pixel {
    Rgb8Pixel { r: v, g: v, b: v }
}

/// Where a swatch lands in the wider row image: a `w`×`h` rectangle whose
/// top-left corner is column `x0` of a `stride`-pixel-wide image. Bundling the
/// placement keeps the render functions to a sane argument count.
pub struct Band {
    pub x0: usize,
    pub stride: usize,
    pub w: usize,
    pub h: usize,
}

impl Band {
    fn idx(&self, bx: usize, by: usize) -> usize {
        by * self.stride + self.x0 + bx
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Real grey, no dithering — the reference row that *will* flash on E-ink.
    TrueGrey,
    /// Hard threshold at 50%. Baseline that throws away every midtone.
    Threshold,
    Bayer2,
    Bayer4,
    Bayer8,
    ClusteredDot,
    FloydSteinberg,
    Atkinson,
    Jarvis,
    Stucki,
    Burkes,
    Sierra3,
    SierraLite,
    WhiteNoise,
    BlueNoise,
}

/// The rows of the comparison sheet, top to bottom. Ordered → error-diffusion →
/// noise, with the two non-dithered references (hard threshold, true grey)
/// bracketing the set so the trade-off each technique makes is easy to read.
pub const ROWS: &[(&str, Method)] = &[
    ("Threshold", Method::Threshold),
    ("Bayer 2x2", Method::Bayer2),
    ("Bayer 4x4", Method::Bayer4),
    ("Bayer 8x8", Method::Bayer8),
    ("Clustered-dot", Method::ClusteredDot),
    ("Floyd-Steinberg", Method::FloydSteinberg),
    ("Atkinson", Method::Atkinson),
    ("Jarvis-J-N", Method::Jarvis),
    ("Stucki", Method::Stucki),
    ("Burkes", Method::Burkes),
    ("Sierra-3", Method::Sierra3),
    ("Sierra-Lite", Method::SierraLite),
    ("White noise", Method::WhiteNoise),
    ("Blue noise", Method::BlueNoise),
    ("True grey", Method::TrueGrey),
];

/// One error-diffusion filter, as `(dx, dy, weight)` taps over a shared divisor.
struct Kernel {
    div: i32,
    taps: &'static [(i32, i32, i32)],
}

const FLOYD_STEINBERG: Kernel = Kernel {
    div: 16,
    taps: &[(1, 0, 7), (-1, 1, 3), (0, 1, 5), (1, 1, 1)],
};

// Diffuses only 6/8 of the error and drops the rest. Losing a bit of error on
// purpose is what gives Atkinson its crisp, high-contrast HyperCard look.
const ATKINSON: Kernel = Kernel {
    div: 8,
    taps: &[
        (1, 0, 1),
        (2, 0, 1),
        (-1, 1, 1),
        (0, 1, 1),
        (1, 1, 1),
        (0, 2, 1),
    ],
};

const JARVIS: Kernel = Kernel {
    div: 48,
    taps: &[
        (1, 0, 7),
        (2, 0, 5),
        (-2, 1, 3),
        (-1, 1, 5),
        (0, 1, 7),
        (1, 1, 5),
        (2, 1, 3),
        (-2, 2, 1),
        (-1, 2, 3),
        (0, 2, 5),
        (1, 2, 3),
        (2, 2, 1),
    ],
};

const STUCKI: Kernel = Kernel {
    div: 42,
    taps: &[
        (1, 0, 8),
        (2, 0, 4),
        (-2, 1, 2),
        (-1, 1, 4),
        (0, 1, 8),
        (1, 1, 4),
        (2, 1, 2),
        (-2, 2, 1),
        (-1, 2, 2),
        (0, 2, 4),
        (1, 2, 2),
        (2, 2, 1),
    ],
};

const BURKES: Kernel = Kernel {
    div: 32,
    taps: &[
        (1, 0, 8),
        (2, 0, 4),
        (-2, 1, 2),
        (-1, 1, 4),
        (0, 1, 8),
        (1, 1, 4),
        (2, 1, 2),
    ],
};

const SIERRA3: Kernel = Kernel {
    div: 32,
    taps: &[
        (1, 0, 5),
        (2, 0, 3),
        (-2, 1, 2),
        (-1, 1, 4),
        (0, 1, 5),
        (1, 1, 4),
        (2, 1, 2),
        (-1, 2, 2),
        (0, 2, 3),
        (1, 2, 2),
    ],
};

const SIERRA_LITE: Kernel = Kernel {
    div: 4,
    taps: &[(1, 0, 2), (-1, 1, 1), (0, 1, 1)],
};

/// Holds the precomputed threshold matrices so the (relatively expensive)
/// blue-noise generation runs once at startup rather than per swatch.
pub struct Ditherer {
    bayer2: Vec<u32>,
    bayer4: Vec<u32>,
    bayer8: Vec<u32>,
    clustered: Vec<u32>,
    white: Vec<u32>,
    blue: Vec<u32>,
}

impl Ditherer {
    pub fn new() -> Self {
        Self {
            bayer2: bayer(2),
            bayer4: bayer(4),
            bayer8: bayer(8),
            clustered: CLUSTERED_DOT.to_vec(),
            white: white_noise(NOISE_N),
            blue: blue_noise(NOISE_N),
        }
    }

    /// Render a swatch of a single grey `shade` (0 = black, 255 = white) into
    /// `out` at the rectangle described by `band`.
    pub fn render_band(&self, method: Method, shade: u8, out: &mut [Rgb8Pixel], band: &Band) {
        match method {
            Method::TrueGrey => fill(grey_px(shade), out, band),
            Method::Threshold => {
                let px = if shade < 128 { BLACK } else { WHITE };
                fill(px, out, band);
            }
            Method::FloydSteinberg
            | Method::Atkinson
            | Method::Jarvis
            | Method::Stucki
            | Method::Burkes
            | Method::Sierra3
            | Method::SierraLite => {
                error_diffuse(shade, kernel_for(method), out, band);
            }
            // Everything else is matrix-based ordered/noise dithering.
            _ => {
                let (mat, n) = self.matrix_for(method);
                let cells = (n * n) as u64;
                for by in 0..band.h {
                    for bx in 0..band.w {
                        let m = mat[(by % n) * n + (bx % n)] as u64;
                        // Black when the shade falls below this cell's threshold.
                        // The matrix value m spans 0..cells, so over a uniform
                        // field the black fraction works out to the ink coverage
                        // (255 - shade)/255; which cells turn black is what makes
                        // the visible pattern.
                        let black = (shade as u64) * 2 * cells < (2 * m + 1) * 255;
                        out[band.idx(bx, by)] = if black { BLACK } else { WHITE };
                    }
                }
            }
        }
    }

    fn matrix_for(&self, method: Method) -> (&[u32], usize) {
        match method {
            Method::Bayer2 => (&self.bayer2, 2),
            Method::Bayer4 => (&self.bayer4, 4),
            Method::Bayer8 => (&self.bayer8, 8),
            Method::ClusteredDot => (&self.clustered, CLUSTERED_N),
            Method::WhiteNoise => (&self.white, NOISE_N),
            Method::BlueNoise => (&self.blue, NOISE_N),
            _ => unreachable!("matrix_for called on a non-matrix method"),
        }
    }
}

fn kernel_for(method: Method) -> &'static Kernel {
    match method {
        Method::FloydSteinberg => &FLOYD_STEINBERG,
        Method::Atkinson => &ATKINSON,
        Method::Jarvis => &JARVIS,
        Method::Stucki => &STUCKI,
        Method::Burkes => &BURKES,
        Method::Sierra3 => &SIERRA3,
        Method::SierraLite => &SIERRA_LITE,
        _ => unreachable!("kernel_for called on a non-diffusion method"),
    }
}

fn fill(px: Rgb8Pixel, out: &mut [Rgb8Pixel], band: &Band) {
    for by in 0..band.h {
        for bx in 0..band.w {
            out[band.idx(bx, by)] = px;
        }
    }
}

/// Run error diffusion over a uniform `shade` field and write the bilevel
/// result into the row image. Each swatch is diffused independently so error
/// never bleeds across band boundaries — every band is a faithful rendering of
/// its own shade.
fn error_diffuse(shade: u8, k: &Kernel, out: &mut [Rgb8Pixel], band: &Band) {
    let (w, h) = (band.w, band.h);
    let mut buf = vec![shade as i32; w * h];
    for y in 0..h {
        for x in 0..w {
            let old = buf[y * w + x];
            let new = if old < 128 { 0 } else { 255 };
            out[band.idx(x, y)] = if new == 0 { BLACK } else { WHITE };
            let err = old - new;
            for &(dx, dy, weight) in k.taps {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && (nx as usize) < w && ny >= 0 && (ny as usize) < h {
                    buf[ny as usize * w + nx as usize] += err * weight / k.div;
                }
            }
        }
    }
}

/// Recursive Bayer (ordered) threshold matrix of side `order` (a power of two).
/// Values are a permutation of `0..order*order`.
fn bayer(order: usize) -> Vec<u32> {
    let mut size = 1usize;
    let mut mat = vec![0u32];
    while size < order {
        let next = size * 2;
        let mut grown = vec![0u32; next * next];
        for y in 0..next {
            for x in 0..next {
                let base = mat[(y % size) * size + (x % size)];
                // Standard 2x2 Bayer recurrence: each quadrant adds a fixed
                // offset so the interleaved pattern stays maximally spread.
                let offset = match (x / size, y / size) {
                    (0, 0) => 0,
                    (1, 0) => 2,
                    (0, 1) => 3,
                    _ => 1,
                };
                grown[y * next + x] = 4 * base + offset;
            }
        }
        mat = grown;
        size = next;
    }
    mat
}

const CLUSTERED_N: usize = 4;

// Classic 4x4 clustered-dot ("halftone") matrix: low values cluster in the
// centre so ink grows as a single dot per cell, like newsprint.
#[rustfmt::skip]
const CLUSTERED_DOT: [u32; 16] = [
    12, 5, 6, 13,
    4, 0, 1, 7,
    11, 3, 2, 8,
    15, 10, 9, 14,
];

/// Side length of the noise tiles. 64x64 is large enough that the repeat isn't
/// obvious, and the blue-noise tile is generated toroidally so it tiles seamlessly.
const NOISE_N: usize = 64;

/// SplitMix64 — a tiny deterministic PRNG. We seed it with a fixed constant so
/// the noise tiles are identical every run (and on every device), avoiding a
/// dependency on the `rand` crate, which the musl cross-build would rather not
/// pull in.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// White-noise threshold matrix: a uniform random permutation of `0..n*n`.
/// Uniform amplitude but with all its energy spread across every frequency,
/// so dithered midtones look grainy and clumpy — the foil to blue noise.
fn white_noise(n: usize) -> Vec<u32> {
    let count = n * n;
    let mut rank: Vec<u32> = (0..count as u32).collect();
    let mut seed = 0xC0FF_EE12_3456_789A;
    // Fisher–Yates shuffle.
    for i in (1..count).rev() {
        let j = (splitmix64(&mut seed) % (i as u64 + 1)) as usize;
        rank.swap(i, j);
    }
    rank
}

fn wrap_index(x: i32, y: i32, n: usize) -> usize {
    let xx = x.rem_euclid(n as i32) as usize;
    let yy = y.rem_euclid(n as i32) as usize;
    yy * n + xx
}

/// Add (or, with a negative `sign`, subtract) one pixel's Gaussian energy
/// contribution to/from the energy map, wrapping toroidally.
fn splat(energy: &mut [f32], p: usize, sign: f32, n: usize, kernel: &[(i32, i32, f32)]) {
    let px = (p % n) as i32;
    let py = (p / n) as i32;
    for &(dx, dy, w) in kernel {
        energy[wrap_index(px + dx, py + dy, n)] += sign * w;
    }
}

/// Location of the "tightest cluster": the set pixel most surrounded by other
/// set pixels (highest energy).
fn tightest_cluster(pattern: &[bool], energy: &[f32]) -> usize {
    let mut best = 0;
    let mut best_energy = f32::MIN;
    for (i, &on) in pattern.iter().enumerate() {
        if on && energy[i] > best_energy {
            best_energy = energy[i];
            best = i;
        }
    }
    best
}

/// Location of the "largest void": the unset pixel in the emptiest region
/// (lowest energy).
fn largest_void(pattern: &[bool], energy: &[f32]) -> usize {
    let mut best = 0;
    let mut best_energy = f32::MAX;
    for (i, &on) in pattern.iter().enumerate() {
        if !on && energy[i] < best_energy {
            best_energy = energy[i];
            best = i;
        }
    }
    best
}

/// Blue-noise threshold matrix via Ulichney's void-and-cluster method.
///
/// Blue noise is random but with its energy concentrated at high frequencies:
/// no low-frequency clumps, so dithered midtones look smooth and grain-free
/// without the rigid texture of an ordered matrix. We build a ranking where
/// every position gets a unique rank `0..n*n`; ranking is driven by a
/// Gaussian-filtered energy map so each newly placed point lands in the
/// current largest void.
fn blue_noise(n: usize) -> Vec<u32> {
    let count = n * n;
    let sigma = 1.5f32;
    let radius = (3.0 * sigma).ceil() as i32;

    // Precompute the Gaussian splat kernel once.
    let mut kernel = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let w = (-((dx * dx + dy * dy) as f32) / (2.0 * sigma * sigma)).exp();
            kernel.push((dx, dy, w));
        }
    }

    // Initial binary pattern: ~10% of pixels placed at random, then relaxed.
    let ones = (count / 10).max(1);
    let mut pattern = vec![false; count];
    let mut energy = vec![0f32; count];
    let mut seed = 0x1234_5678_9ABC_DEF0;
    let mut placed = 0;
    while placed < ones {
        let r = (splitmix64(&mut seed) % count as u64) as usize;
        if !pattern[r] {
            pattern[r] = true;
            splat(&mut energy, r, 1.0, n, &kernel);
            placed += 1;
        }
    }

    // Relax: repeatedly move the tightest cluster's point into the largest void
    // until that move would put it back where it started (the pattern is now as
    // homogeneous as it gets).
    for _ in 0..count * 4 {
        let c = tightest_cluster(&pattern, &energy);
        pattern[c] = false;
        splat(&mut energy, c, -1.0, n, &kernel);
        let v = largest_void(&pattern, &energy);
        pattern[v] = true;
        splat(&mut energy, v, 1.0, n, &kernel);
        if v == c {
            break;
        }
    }

    let prototype = pattern.clone();
    let prototype_energy = energy.clone();
    let mut rank = vec![0u32; count];

    // Phase 1: rank the prototype's points from `ones - 1` down to 0 by
    // repeatedly removing the tightest cluster.
    let mut p1 = prototype.clone();
    let mut e1 = prototype_energy.clone();
    for r in (0..ones).rev() {
        let c = tightest_cluster(&p1, &e1);
        p1[c] = false;
        splat(&mut e1, c, -1.0, n, &kernel);
        rank[c] = r as u32;
    }

    // Phase 2: from the prototype, fill the remaining voids, ranking each new
    // point from `ones` up to `count - 1`.
    let mut p2 = prototype;
    let mut e2 = prototype_energy;
    for r in ones..count {
        let v = largest_void(&p2, &e2);
        p2[v] = true;
        splat(&mut e2, v, 1.0, n, &kernel);
        rank[v] = r as u32;
    }

    rank
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A threshold matrix must assign every rank in `0..count` exactly once;
    /// gaps or repeats would skew the tonal response and, for blue noise, would
    /// signal the void-and-cluster passes left some pixel unranked.
    fn assert_permutation(matrix: &[u32], count: usize) {
        assert_eq!(matrix.len(), count);
        let mut seen = vec![false; count];
        for &v in matrix {
            let v = v as usize;
            assert!(v < count, "rank {v} out of range 0..{count}");
            assert!(!seen[v], "rank {v} appears more than once");
            seen[v] = true;
        }
    }

    #[test]
    fn matrices_are_permutations() {
        assert_permutation(&bayer(2), 4);
        assert_permutation(&bayer(4), 16);
        assert_permutation(&bayer(8), 64);
        assert_permutation(&CLUSTERED_DOT, CLUSTERED_N * CLUSTERED_N);
        assert_permutation(&white_noise(NOISE_N), NOISE_N * NOISE_N);
        // Implicitly checks void-and-cluster terminates and ranks every pixel.
        assert_permutation(&blue_noise(NOISE_N), NOISE_N * NOISE_N);
    }

    #[test]
    fn dithered_bands_are_strictly_bilevel() {
        let ditherer = Ditherer::new();
        let (w, h) = (16usize, 16usize);
        for &(_, method) in ROWS {
            if method == Method::TrueGrey {
                continue;
            }
            for shade in [10u8, 64, 128, 200, 245] {
                let mut out = vec![WHITE; w * h];
                let band = Band {
                    x0: 0,
                    stride: w,
                    w,
                    h,
                };
                ditherer.render_band(method, shade, &mut out, &band);
                for px in &out {
                    assert!(
                        (px.r == 0 || px.r == 255) && px.r == px.g && px.g == px.b,
                        "non-bilevel pixel {:?} from a dithering method",
                        (px.r, px.g, px.b)
                    );
                }
            }
        }
    }

    #[test]
    fn coverage_tracks_shade() {
        // Darker shades must lay down at least as much ink as lighter ones.
        let ditherer = Ditherer::new();
        let (w, h) = (64usize, 64usize);
        let black_count = |method, shade| {
            let mut out = vec![WHITE; w * h];
            let band = Band {
                x0: 0,
                stride: w,
                w,
                h,
            };
            ditherer.render_band(method, shade, &mut out, &band);
            out.iter().filter(|p| p.r == 0).count()
        };
        for &(name, method) in ROWS {
            if method == Method::TrueGrey || method == Method::Threshold {
                continue;
            }
            let light = black_count(method, 210);
            let dark = black_count(method, 45);
            assert!(dark >= light, "{name}: dark shade had less ink than light");
        }
    }
}
