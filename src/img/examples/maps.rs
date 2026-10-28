//! Developer example: writes the noise maps (and the decoded pixels) of one image for the numpy oracle.
//!
//! `cargo run -p rv-imgauth-img --example maps -- <in.(jpg|png|webp)> <outdir>` writes `pixels.png` (the decoded
//! RGB, lossless), `noise.png`, `noise-sigma.png` and, for a JPEG, `ela.png` (quality 90). Then
//! `python3 tests/oracle_maps.py <in> <outdir>` recomputes the residual and sigma maps independently.

use rv_imgauth_img::analysis::{ela, noise};
use rv_imgauth_img::{Format, decode_pixels};

fn png(p: &rv_imgauth_img::Pixels) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, p.width, p.height);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&p.rgb).unwrap();
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: maps <image> <outdir>");
        std::process::exit(2);
    }
    let data = std::fs::read(&args[1]).expect("read input");
    let out = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(out).expect("create outdir");
    let format = Format::sniff(&data);
    let px = decode_pixels(&data, format).expect("decode");
    std::fs::write(out.join("pixels.png"), png(&px)).unwrap();
    let t = std::time::Instant::now();
    let (signals, maps) = noise::maps(&px);
    eprintln!("noise maps in {:.2?}", t.elapsed());
    for m in maps {
        std::fs::write(out.join(&m.name), &m.png).unwrap();
    }
    if format == Format::Jpeg {
        let m = ela::map(&px, 90).expect("ela");
        std::fs::write(out.join(&m.name), &m.png).unwrap();
    }
    for s in signals {
        println!("{} {:?}", s.code, s.params);
    }
}
