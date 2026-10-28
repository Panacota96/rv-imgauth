#!/usr/bin/env python3
"""Independent numpy oracle for the noise maps of rv-imgauth-img.

usage: python3 tests/oracle_maps.py <input image> <outdir written by `cargo run -p rv-imgauth-img --example maps`>
       python3 tests/oracle_maps.py --reports <OUT>/reports/<RUN-ID>   (every item folder with noise maps)

Recomputes, from the same RGB pixels, (1) the median residual map noise.png = min(255, 10*|Y - median3x3(Y)|)
with replicated borders and (2) the Mahdian-Saic sigma map noise-sigma.png: one-level db8 DWT (PyWavelets,
mode "periodization", or a numpy implementation of the same filter when PyWavelets is missing), HH1 sub-band,
sigma = median(|HH1|)/0.6745 per 40x40 block, grey = min(255, round(25.5*sigma)). Both maps must agree within
+-1 grey level. Pixels: a PNG input is decoded with Pillow; for a JPEG the example's lossless pixels.png is used,
because JPEG decoders differ by rounding (the oracle checks the map computation, not the decoder); in an item
folder of a run that file is maps/original.png, the lossless preview of the decoded pixels. ELA is not
compared (Pillow's JPEG encoder differs from jpeg-encoder).
"""
import sys
from pathlib import Path

import numpy as np
from PIL import Image

DB8 = [-0.00011747678412476953, 0.0006754494064505693, -0.00039174037337694705, -0.004870352993451574,
       0.008746094047405777, 0.013981027917398282, -0.044088253930794755, -0.017369301001807547,
       0.12874742662047847, 0.0004724845739132828, -0.2840155429615469, -0.015829105256349306,
       0.5853546836542067, 0.6756307362972898, 0.31287159091429995, 0.05441584224310401]


def luma(rgb):
    r, g, b = (rgb[..., i].astype(np.int64) for i in range(3))
    return ((19595 * r + 38470 * g + 7471 * b + 32768) >> 16).astype(np.int64)


def residual(y):
    p = np.pad(y, 1, mode="edge")
    h, w = y.shape
    stack = np.stack([p[dy:dy + h, dx:dx + w] for dy in range(3) for dx in range(3)])
    med = np.median(stack, axis=0).astype(np.int64)
    return np.minimum(255, 10 * np.abs(y - med)).astype(np.uint8)


def hh1(y):
    h, w = y.shape[0] & ~1, y.shape[1] & ~1
    x = y[:h, :w].astype(np.float64)
    try:
        import pywt
        _, (_, _, hh) = pywt.dwt2(x, "db8", mode="periodization")
        return hh, "pywt " + pywt.__version__
    except ImportError:
        lo = np.array(DB8)
        hi = np.array([(-1) ** (j + 1) * lo[15 - j] for j in range(16)])

        def filt(a, axis):
            n = a.shape[axis]
            out = 0
            for j in range(16):
                idx = (2 * np.arange(n // 2) + 8 - j) % n
                out = out + hi[j] * np.take(a, idx, axis=axis)
            return out
        return filt(filt(x, 1), 0), "numpy"


def sigma_map(y):
    hh, how = hh1(y)
    hh = np.abs(hh)
    H, W = hh.shape
    out = np.zeros((H, W), dtype=np.uint8)
    bw = W // 40 + (W % 40 >= 10)
    bh = H // 40 + (H % 40 >= 10)
    for by in range(bh):
        for bx in range(bw):
            blk = hh[by * 40:(by + 1) * 40, bx * 40:(bx + 1) * 40]
            s = np.median(blk) / 0.6745
            out[by * 40:(by + 1) * 40, bx * 40:(bx + 1) * 40] = np.clip(np.round(s * 25.5), 0, 255)
    return out, how


def check(src, outdir):
    """True when noise.png and noise-sigma.png in `outdir` match the oracle within 1 grey level."""
    if src.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n":
        pix = src
    else:
        pix = outdir / "pixels.png" if (outdir / "pixels.png").is_file() else outdir / "original.png"
    rgb = np.asarray(Image.open(pix).convert("RGB"))
    y = luma(rgb)
    ok = True
    for name, (ref, how) in (("noise.png", (residual(y), "numpy")), ("noise-sigma.png", sigma_map(y))):
        got = np.asarray(Image.open(outdir / name)).astype(np.int64)
        if got.shape != ref.shape:
            print(f"{name}: shape {got.shape} vs oracle {ref.shape}")
            ok = False
            continue
        d = np.abs(got - ref.astype(np.int64))
        print(f"{name}: max |diff| {d.max()} grey levels, {np.count_nonzero(d)} pixels differ ({how}; pixels {pix.name})")
        ok &= d.max() <= 1
    return ok


def main():
    if sys.argv[1:2] == ["--reports"]:
        import json
        items = sorted(p for p in (Path(sys.argv[2]) / "items").iterdir() if (p / "maps" / "noise.png").is_file())
        ok = bool(items)
        for item in items:
            src = Path(json.loads((item / "technical.json").read_text(encoding="utf-8"))["item"]["path"])
            print(f"== {item.name}")
            ok &= check(src, item / "maps")
        print(f"{len(items)} item(s): " + ("PASS" if ok else "FAIL"))
        sys.exit(0 if ok else 1)
    ok = check(Path(sys.argv[1]), Path(sys.argv[2]))
    print("PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
