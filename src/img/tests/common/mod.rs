//! Test-only minimal baseline JPEG reader (DQT, SOF0/1, DHT, DRI, one SOS, Huffman decoding to quantized
//! coefficients). Used by the sample harness until/unless the library parser is available; the library's
//! `parse` and `decode_coefficients` are preferred when they succeed.

use rv_imgauth_img::{Component, ComponentCoefficients, JpegInfo, QuantTable};

const ZZ: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Default, Clone)]
struct Huff {
    // (length, code) -> symbol
    codes: std::collections::HashMap<(u8, u16), u8>,
}

impl Huff {
    fn new(counts: &[u8], symbols: &[u8]) -> Huff {
        let mut h = Huff::default();
        let (mut code, mut k) = (0u16, 0usize);
        for (len, &n) in counts.iter().enumerate() {
            for _ in 0..n {
                h.codes.insert((len as u8 + 1, code), symbols[k]);
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        h
    }
}

struct Bits<'a> {
    d: &'a [u8],
    p: usize,
    acc: u32,
    n: u32,
}

impl Bits<'_> {
    fn bit(&mut self) -> u32 {
        if self.n == 0 {
            let mut b = self.d.get(self.p).copied().unwrap_or(0);
            if b == 0xFF {
                let nx = self.d.get(self.p + 1).copied().unwrap_or(0);
                if nx == 0 {
                    self.p += 1;
                } else {
                    b = 0; // marker: feed zeros
                    self.p -= 1;
                }
            }
            self.p += 1;
            self.acc = b as u32;
            self.n = 8;
        }
        self.n -= 1;
        (self.acc >> self.n) & 1
    }
    fn get(&mut self, s: u32) -> i32 {
        let mut v = 0i32;
        for _ in 0..s {
            v = (v << 1) | self.bit() as i32;
        }
        v
    }
    fn decode(&mut self, h: &Huff) -> u8 {
        let mut code = 0u16;
        for len in 1..=16u8 {
            code = (code << 1) | self.bit() as u16;
            if let Some(&s) = h.codes.get(&(len, code)) {
                return s;
            }
        }
        panic!("bad Huffman code");
    }
    fn extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.get(s);
        if v < (1 << (s - 1)) {
            v - (1 << s) + 1
        } else {
            v
        }
    }
    /// Skips to the byte after the next RSTn marker.
    fn restart(&mut self) {
        self.n = 0;
        while self.p + 1 < self.d.len()
            && !(self.d[self.p] == 0xFF && (0xD0..=0xD7).contains(&self.d[self.p + 1]))
        {
            self.p += 1;
        }
        self.p += 2;
    }
}

pub fn read(data: &[u8]) -> (JpegInfo, Vec<ComponentCoefficients>) {
    let mut j = JpegInfo::default();
    let mut dc = vec![Huff::default(); 4];
    let mut ac = vec![Huff::default(); 4];
    let mut p = 2;
    loop {
        assert_eq!(data[p], 0xFF);
        let m = data[p + 1];
        let len = u16::from_be_bytes([data[p + 2], data[p + 3]]) as usize;
        let seg = &data[p + 4..p + 2 + len];
        match m {
            0xDB => {
                let mut i = 0;
                while i < seg.len() {
                    let (pq, tq) = (seg[i] >> 4, seg[i] & 15);
                    let mut v = vec![0u16; 64];
                    for k in 0..64 {
                        v[ZZ[k]] = if pq == 0 {
                            seg[i + 1 + k] as u16
                        } else {
                            u16::from_be_bytes([seg[i + 1 + 2 * k], seg[i + 2 + 2 * k]])
                        };
                    }
                    j.qtables.push(QuantTable {
                        id: tq,
                        precision: pq,
                        values: v,
                    });
                    i += 1 + 64 * (pq as usize + 1);
                }
            }
            0xC0 | 0xC1 => {
                j.sof = format!("SOF{}", m - 0xC0);
                j.precision = seg[0];
                j.height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
                j.width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                for c in 0..seg[5] as usize {
                    let b = &seg[6 + 3 * c..9 + 3 * c];
                    j.components.push(Component {
                        id: b[0],
                        h: b[1] >> 4,
                        v: b[1] & 15,
                        tq: b[2],
                    });
                }
            }
            0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                panic!("only baseline JPEG in this test reader")
            }
            0xC4 => {
                let mut i = 0;
                while i < seg.len() {
                    let (tc, th) = (seg[i] >> 4, (seg[i] & 15) as usize);
                    let counts = &seg[i + 1..i + 17];
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    let h = Huff::new(counts, &seg[i + 17..i + 17 + n]);
                    if tc == 0 {
                        dc[th] = h
                    } else {
                        ac[th] = h
                    }
                    i += 17 + n;
                }
            }
            0xDD => j.restart_interval = u16::from_be_bytes([seg[0], seg[1]]),
            0xDA => {
                let ns = seg[0] as usize;
                let sel: Vec<(usize, usize, usize)> = (0..ns)
                    .map(|k| {
                        let id = seg[1 + 2 * k];
                        let ci = j.components.iter().position(|c| c.id == id).unwrap();
                        (
                            ci,
                            (seg[2 + 2 * k] >> 4) as usize,
                            (seg[2 + 2 * k] & 15) as usize,
                        )
                    })
                    .collect();
                return (j.clone(), scan(&j, &sel, &dc, &ac, &data[p + 2 + len..]));
            }
            _ => {}
        }
        p += 2 + len;
    }
}

fn scan(
    j: &JpegInfo,
    sel: &[(usize, usize, usize)],
    dc: &[Huff],
    ac: &[Huff],
    d: &[u8],
) -> Vec<ComponentCoefficients> {
    let hmax = j.components.iter().map(|c| c.h as u32).max().unwrap();
    let vmax = j.components.iter().map(|c| c.v as u32).max().unwrap();
    let interleaved = sel.len() > 1;
    let (mx, my) = if interleaved {
        (j.width.div_ceil(8 * hmax), j.height.div_ceil(8 * vmax))
    } else {
        let c = &j.components[sel[0].0];
        (
            j.width.div_ceil(8 * hmax / c.h as u32),
            j.height.div_ceil(8 * vmax / c.v as u32),
        )
    };
    let mut out: Vec<ComponentCoefficients> = sel
        .iter()
        .map(|&(ci, _, _)| {
            let c = &j.components[ci];
            let (bw, bh) = if interleaved {
                (mx * c.h as u32, my * c.v as u32)
            } else {
                (mx, my)
            };
            ComponentCoefficients {
                component_id: c.id,
                tq: c.tq,
                blocks_wide: bw,
                blocks_high: bh,
                coefs: vec![0; (bw * bh * 64) as usize],
            }
        })
        .collect();
    let mut bits = Bits {
        d,
        p: 0,
        acc: 0,
        n: 0,
    };
    let mut pred = vec![0i32; sel.len()];
    let ri = j.restart_interval as u32;
    for m in 0..mx * my {
        if ri > 0 && m > 0 && m % ri == 0 {
            bits.restart();
            pred.iter_mut().for_each(|p| *p = 0);
        }
        let (ux, uy) = (m % mx, m / mx);
        for (s, &(ci, td, ta)) in sel.iter().enumerate() {
            let c = &j.components[ci];
            let (h, v) = if interleaved {
                (c.h as u32, c.v as u32)
            } else {
                (1, 1)
            };
            for by in 0..v {
                for bx in 0..h {
                    let (gx, gy) = (ux * h + bx, uy * v + by);
                    let o = &mut out[s];
                    let base = ((gy * o.blocks_wide + gx) * 64) as usize;
                    let t = bits.decode(&dc[td]) as u32;
                    pred[s] += bits.extend(t);
                    o.coefs[base] = pred[s] as i16;
                    let mut k = 1;
                    while k < 64 {
                        let rs = bits.decode(&ac[ta]);
                        let (r, sz) = ((rs >> 4) as usize, (rs & 15) as u32);
                        if sz == 0 {
                            if r == 15 {
                                k += 16;
                                continue;
                            }
                            break;
                        }
                        k += r;
                        o.coefs[base + ZZ[k]] = bits.extend(sz) as i16;
                        k += 1;
                    }
                }
            }
        }
    }
    out
}
