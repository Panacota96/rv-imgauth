//! A small zlib/DEFLATE decoder (RFC 1950/1951) for PNG text and ICC chunks: those are a few kilobytes, so a
//! simple bit-by-bit canonical Huffman decoder is enough. Output is capped (`limit`) against decompression bombs.
//! The pixel data (IDAT) is never inflated here.

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    bit: u32,
}

impl Bits<'_> {
    fn bit(&mut self) -> Result<u32, String> {
        let byte = *self.d.get(self.pos).ok_or("compressed data ends early")?;
        let b = (byte as u32 >> self.bit) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.pos += 1;
        }
        Ok(b)
    }
    fn bits(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }
    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }
}

/// Canonical Huffman code: counts per length and symbols ordered by code.
struct Huff {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huff {
    fn new(lengths: &[u8]) -> Result<Huff, String> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + counts[i - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Huff { counts, symbols })
    }
    fn decode(&self, b: &mut Bits) -> Result<u16, String> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.bit()? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return self
                    .symbols
                    .get((index + (code - first)) as usize)
                    .copied()
                    .ok_or_else(|| "bad Huffman code".to_string());
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err("bad Huffman code".into())
    }
}

const LBASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEXT: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Inflates a zlib stream (2-byte header, DEFLATE data; the Adler-32 trailer is not checked).
pub fn zlib(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    if data.len() < 2 {
        return Err("zlib stream too short".into());
    }
    let (cmf, flg) = (data[0], data[1]);
    if cmf & 0x0f != 8 || !((cmf as u16) << 8 | flg as u16).is_multiple_of(31) || flg & 0x20 != 0 {
        return Err("not a zlib stream".into());
    }
    deflate(&data[2..], limit)
}

/// Inflates raw DEFLATE data.
pub fn deflate(data: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut b = Bits {
        d: data,
        pos: 0,
        bit: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = b.bit()?;
        match b.bits(2)? {
            0 => {
                b.align();
                let p = b.pos;
                let len = crate::parse::le16(data, p).ok_or("stored block truncated")? as usize;
                let nlen = crate::parse::le16(data, p + 2).ok_or("stored block truncated")?;
                if len as u16 != !nlen {
                    return Err("stored block length check failed".into());
                }
                let chunk = data
                    .get(p + 4..p + 4 + len)
                    .ok_or("stored block truncated")?;
                if out.len() + len > limit {
                    return Err(format!("decompressed size above the {limit}-byte limit"));
                }
                out.extend_from_slice(chunk);
                b.pos = p + 4 + len;
            }
            1 => {
                let mut l = [0u8; 288];
                l[..144].fill(8);
                l[144..256].fill(9);
                l[256..280].fill(7);
                l[280..].fill(8);
                let lit = Huff::new(&l)?;
                let dist = Huff::new(&[5u8; 30])?;
                block(&mut b, &mut out, &lit, &dist, limit)?;
            }
            2 => {
                let hlit = b.bits(5)? as usize + 257;
                let hdist = b.bits(5)? as usize + 1;
                let hclen = b.bits(4)? as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut cl = [0u8; 19];
                for &i in ORDER.iter().take(hclen) {
                    cl[i] = b.bits(3)? as u8;
                }
                let clh = Huff::new(&cl)?;
                let mut lens = vec![0u8; hlit + hdist];
                let mut i = 0;
                while i < hlit + hdist {
                    let sym = clh.decode(&mut b)?;
                    let (val, rep) = match sym {
                        0..=15 => (sym as u8, 1),
                        16 => {
                            let prev = *lens
                                .get(i.wrapping_sub(1))
                                .ok_or("repeat with no previous length")?;
                            (prev, 3 + b.bits(2)? as usize)
                        }
                        17 => (0, 3 + b.bits(3)? as usize),
                        _ => (0, 11 + b.bits(7)? as usize),
                    };
                    if i + rep > hlit + hdist {
                        return Err("code lengths overflow".into());
                    }
                    lens[i..i + rep].fill(val);
                    i += rep;
                }
                let lit = Huff::new(&lens[..hlit])?;
                let dist = Huff::new(&lens[hlit..])?;
                block(&mut b, &mut out, &lit, &dist, limit)?;
            }
            _ => return Err("invalid DEFLATE block type".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

fn block(
    b: &mut Bits,
    out: &mut Vec<u8>,
    lit: &Huff,
    dist: &Huff,
    limit: usize,
) -> Result<(), String> {
    loop {
        let sym = lit.decode(b)? as usize;
        if sym < 256 {
            if out.len() >= limit {
                return Err(format!("decompressed size above the {limit}-byte limit"));
            }
            out.push(sym as u8);
        } else if sym == 256 {
            return Ok(());
        } else {
            let s = sym - 257;
            if s >= 29 {
                return Err("invalid length symbol".into());
            }
            let len = LBASE[s] as usize + b.bits(LEXT[s] as u32)? as usize;
            let ds = dist.decode(b)? as usize;
            if ds >= 30 {
                return Err("invalid distance symbol".into());
            }
            let d = DBASE[ds] as usize + b.bits(DEXT[ds] as u32)? as usize;
            if d > out.len() {
                return Err("distance before start of output".into());
            }
            if out.len() + len > limit {
                return Err(format!("decompressed size above the {limit}-byte limit"));
            }
            let start = out.len() - d;
            for k in 0..len {
                let c = out[start + k];
                out.push(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_block() {
        // zlib header 78 01, stored final block "abc"
        let z = [
            0x78, 0x01, 0x01, 0x03, 0x00, 0xFC, 0xFF, b'a', b'b', b'c', 0, 0, 0, 0,
        ];
        assert_eq!(zlib(&z, 100).unwrap(), b"abc");
    }

    #[test]
    fn fixed_huffman_from_python_zlib() {
        // python3 -c "import zlib;print(list(zlib.compress(b'hello hello hello hello')))"
        let z = [
            120, 156, 203, 72, 205, 201, 201, 87, 200, 64, 39, 1, 104, 3, 8, 177,
        ];
        assert_eq!(zlib(&z, 100).unwrap(), b"hello hello hello hello");
    }

    #[test]
    fn dynamic_huffman_and_limit() {
        // python3: zlib.compress(bytes(range(256))*4 + b'xyz'*50, 9)
        let mut src: Vec<u8> = (0..=255u8).collect::<Vec<_>>().repeat(4);
        src.extend(b"xyz".repeat(50));
        let z = include_bytes!("../../tests/data/dynamic.zlib");
        assert_eq!(zlib(z, 10_000).unwrap(), src);
        assert!(zlib(z, 100).is_err());
    }

    #[test]
    fn garbage_never_panics() {
        let mut seed = 7u32;
        for _ in 0..2000 {
            let n = (seed % 64) as usize;
            let v: Vec<u8> = (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    (seed >> 16) as u8
                })
                .collect();
            let _ = deflate(&v, 4096);
            let _ = zlib(&v, 4096);
        }
    }
}
