//! Test-only helpers: a deterministic PRNG, a "natural-like" synthetic image (gradients, shapes, texture and
//! sensor-like noise; no real photograph), and a reference JPEG luma pipeline (forward DCT + IJG quantization,
//! optionally repeated) that produces `ComponentCoefficients` without a JPEG parser.

use crate::model::{ComponentCoefficients, Pixels};
use std::f64::consts::PI;

/// SplitMix64 (Steele, Lea, Flood 2014): small, fast, deterministic on every platform.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f()
    }
    /// Approximately N(0, 1) (sum of 4 uniforms, scaled).
    pub fn gauss(&mut self) -> f64 {
        (self.f() + self.f() + self.f() + self.f() - 2.0) * (3.0f64).sqrt()
    }
}

/// A photo-like synthetic RGB image: smooth illumination gradients, filled shapes with hard edges, a few
/// textured patches, and per-pixel noise of standard deviation `noise`.
pub fn natural_like(w: u32, h: u32, seed: u64, noise: f64) -> Pixels {
    let (wu, hu) = (w as usize, h as usize);
    let mut r = Rng::new(seed);
    let mut img = vec![[0f64; 3]; wu * hu];
    // Smooth field: gradients + low-frequency cosines.
    let base: [f64; 3] = [
        r.range(70.0, 150.0),
        r.range(70.0, 150.0),
        r.range(70.0, 150.0),
    ];
    let grad: Vec<(f64, f64)> = (0..3)
        .map(|_| (r.range(-60.0, 60.0), r.range(-60.0, 60.0)))
        .collect();
    let waves: Vec<(f64, f64, f64, f64)> = (0..6)
        .map(|_| {
            (
                r.range(0.5, 3.0),
                r.range(0.5, 3.0),
                r.range(0.0, std::f64::consts::TAU),
                r.range(5.0, 20.0),
            )
        })
        .collect();
    for y in 0..hu {
        for x in 0..wu {
            let (u, v) = (x as f64 / w as f64, y as f64 / h as f64);
            let wv: f64 = waves
                .iter()
                .map(|(a, b, ph, amp)| amp * (2.0 * PI * (a * u + b * v) + ph).cos())
                .sum();
            for c in 0..3 {
                img[y * wu + x][c] = base[c] + grad[c].0 * (u - 0.5) + grad[c].1 * (v - 0.5) + wv;
            }
        }
    }
    // Shapes: rectangles and discs with a uniform tint and a slow gradient inside.
    for i in 0..14 {
        let (cx, cy) = (r.range(0.0, w as f64), r.range(0.0, h as f64));
        let (rx, ry) = (r.range(15.0, w as f64 / 6.0), r.range(15.0, h as f64 / 6.0));
        let col = [
            r.range(20.0, 235.0),
            r.range(20.0, 235.0),
            r.range(20.0, 235.0),
        ];
        let disc = i % 2 == 0;
        let (x0, x1) = ((cx - rx).max(0.0) as usize, ((cx + rx) as usize).min(wu));
        let (y0, y1) = ((cy - ry).max(0.0) as usize, ((cy + ry) as usize).min(hu));
        for y in y0..y1 {
            for x in x0..x1 {
                let (dx, dy) = ((x as f64 - cx) / rx, (y as f64 - cy) / ry);
                if !disc || dx * dx + dy * dy <= 1.0 {
                    for c in 0..3 {
                        img[y * wu + x][c] = col[c] + 12.0 * dx;
                    }
                }
            }
        }
    }
    // Textured patches: bilinear value noise on a 5-pixel lattice.
    for _ in 0..4 {
        let pw = r.range(60.0, w as f64 / 3.0) as usize;
        let ph = r.range(60.0, h as f64 / 3.0) as usize;
        let x0 = r.range(0.0, (wu - pw.min(wu)) as f64) as usize;
        let y0 = r.range(0.0, (hu - ph.min(hu)) as f64) as usize;
        let cell = 5usize;
        let (gw, gh) = (pw / cell + 2, ph / cell + 2);
        let lattice: Vec<f64> = (0..gw * gh).map(|_| r.range(-35.0, 35.0)).collect();
        let tint = [r.range(0.6, 1.2), r.range(0.6, 1.2), r.range(0.6, 1.2)];
        for y in 0..ph.min(hu - y0) {
            for x in 0..pw.min(wu - x0) {
                let (gx, gy) = (x as f64 / cell as f64, y as f64 / cell as f64);
                let (ix, iy) = (gx as usize, gy as usize);
                let (fx, fy) = (gx - ix as f64, gy - iy as f64);
                let l = |a: usize, b: usize| lattice[b * gw + a];
                let v = l(ix, iy) * (1.0 - fx) * (1.0 - fy)
                    + l(ix + 1, iy) * fx * (1.0 - fy)
                    + l(ix, iy + 1) * (1.0 - fx) * fy
                    + l(ix + 1, iy + 1) * fx * fy;
                for c in 0..3 {
                    img[(y0 + y) * wu + x0 + x][c] += v * tint[c];
                }
            }
        }
    }
    let mut rgb = Vec::with_capacity(wu * hu * 3);
    for px in &img {
        for c in px {
            rgb.push((c + noise * r.gauss()).round().clamp(0.0, 255.0) as u8);
        }
    }
    Pixels {
        width: w,
        height: h,
        rgb,
    }
}

/// Copies the `size`x`size` square at (sx, sy) to (sx+dx, sy+dy).
pub fn plant_copy(p: &mut Pixels, sx: usize, sy: usize, size: usize, dx: usize, dy: usize) {
    let w = p.width as usize;
    for y in 0..size {
        for x in 0..size {
            let s = ((sy + y) * w + sx + x) * 3;
            let d = ((sy + dy + y) * w + sx + dx + x) * 3;
            for c in 0..3 {
                p.rgb[d + c] = p.rgb[s + c];
            }
        }
    }
}

fn dct_basis() -> [[f64; 8]; 8] {
    let mut c = [[0f64; 8]; 8];
    for (u, row) in c.iter_mut().enumerate() {
        let a = if u == 0 {
            (1.0f64 / 8.0).sqrt()
        } else {
            (2.0f64 / 8.0).sqrt()
        };
        for (x, v) in row.iter_mut().enumerate() {
            *v = a * ((2 * x + 1) as f64 * u as f64 * PI / 16.0).cos();
        }
    }
    c
}

/// Orthonormal 8x8 DCT-II (the JPEG FDCT, T.81 A.3.3).
pub fn fdct(block: &[f64; 64]) -> [f64; 64] {
    let c = dct_basis();
    let mut out = [0f64; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut s = 0.0;
            for y in 0..8 {
                for x in 0..8 {
                    s += c[v][y] * c[u][x] * block[y * 8 + x];
                }
            }
            out[v * 8 + u] = s;
        }
    }
    out
}

pub fn idct(coef: &[f64; 64]) -> [f64; 64] {
    let c = dct_basis();
    let mut out = [0f64; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0;
            for v in 0..8 {
                for u in 0..8 {
                    s += c[v][y] * c[u][x] * coef[v * 8 + u];
                }
            }
            out[y * 8 + x] = s;
        }
    }
    out
}

/// Luma JPEG pipeline on 8-bit grey samples (`w`, `h` multiples of 8): for each table in `tables` in turn,
/// level shift, FDCT, quantize (round half away from zero); between compressions decode back to 8-bit pixels
/// (dequantize, IDCT, round, clamp). Returns the quantized coefficients of the last compression.
pub fn luma_coefficients(
    grey: &[u8],
    w: usize,
    h: usize,
    tables: &[[u16; 64]],
) -> ComponentCoefficients {
    assert!(w.is_multiple_of(8) && h.is_multiple_of(8) && !tables.is_empty());
    let mut pix: Vec<f64> = grey.iter().map(|&v| v as f64).collect();
    let (bw, bh) = (w / 8, h / 8);
    let mut coefs = vec![0i16; bw * bh * 64];
    for (k, q) in tables.iter().enumerate() {
        let last = k + 1 == tables.len();
        for by in 0..bh {
            for bx in 0..bw {
                let mut b = [0f64; 64];
                for y in 0..8 {
                    for x in 0..8 {
                        b[y * 8 + x] = pix[(by * 8 + y) * w + bx * 8 + x] - 128.0;
                    }
                }
                let f = fdct(&b);
                let mut qd = [0f64; 64];
                for i in 0..64 {
                    let v = (f[i] / q[i] as f64).round();
                    if last {
                        coefs[(by * bw + bx) * 64 + i] = v as i16;
                    }
                    qd[i] = v * q[i] as f64;
                }
                if !last {
                    let r = idct(&qd);
                    for y in 0..8 {
                        for x in 0..8 {
                            pix[(by * 8 + y) * w + bx * 8 + x] =
                                (r[y * 8 + x] + 128.0).round().clamp(0.0, 255.0);
                        }
                    }
                }
            }
        }
    }
    ComponentCoefficients {
        component_id: 1,
        tq: 0,
        blocks_wide: bw as u32,
        blocks_high: bh as u32,
        coefs,
    }
}
