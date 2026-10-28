#!/usr/bin/env python3
"""Deterministic synthetic samples for the pixel analyses (no real photographs).

Writes into examples/sample/evidence/ (run from the repository root, or pass the output folder):
  pix-natural.jpg         no manipulation, single save at IJG q=92
  pix-spliced-ghost.jpg   a 320x320 region (grid-aligned at 320,224) taken from a q=60 save, pasted back,
                          the whole saved at q=90 (JPEG ghost ground truth, Farid 2009)
  pix-copy-move.jpg       a 128x128 textured patch at (180,300) duplicated 200 px to the right, saved q=90
  pix-whatsapp-like.jpg   "camera" save q=95 at 2400x1800, decoded, resized to long side 1600 (Lanczos),
                          saved at IJG q=80 without EXIF (messenger-like re-encoding)
Requires numpy and Pillow. The bytes depend on the Pillow/libjpeg build; the committed files are the reference
and examples/sample/expected/pix-*.json are computed from them by src/img/tests/analyses.rs (RV_BLESS=1).
"""
import io
import sys
from pathlib import Path

import numpy as np
from PIL import Image


def natural_like(w, h, seed, noise=3.0):
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w].astype(np.float64)
    u, v = x / w, y / h
    img = np.zeros((h, w, 3))
    base = rng.uniform(70, 150, 3)
    waves = sum(
        rng.uniform(5, 20) * np.cos(2 * np.pi * (rng.uniform(0.5, 3) * u + rng.uniform(0.5, 3) * v) + rng.uniform(0, 6.28))
        for _ in range(6)
    )
    for c in range(3):
        img[..., c] = base[c] + rng.uniform(-60, 60) * (u - 0.5) + rng.uniform(-60, 60) * (v - 0.5) + waves
    for i in range(14):
        cx, cy = rng.uniform(0, w), rng.uniform(0, h)
        rx, ry = rng.uniform(15, w / 6), rng.uniform(15, h / 6)
        col = rng.uniform(20, 235, 3)
        dx, dy = (x - cx) / rx, (y - cy) / ry
        m = (dx * dx + dy * dy <= 1) if i % 2 == 0 else ((np.abs(dx) <= 1) & (np.abs(dy) <= 1))
        for c in range(3):
            img[..., c] = np.where(m, col[c] + 12 * dx, img[..., c])
    for _ in range(4):
        pw, ph = int(rng.uniform(60, w / 3)), int(rng.uniform(60, h / 3))
        x0, y0 = int(rng.uniform(0, w - pw)), int(rng.uniform(0, h - ph))
        lat = rng.uniform(-35, 35, (ph // 5 + 2, pw // 5 + 2))
        small = Image.fromarray(lat.astype(np.float32), mode="F")
        tex = np.asarray(small.resize((small.width * 5, small.height * 5), Image.BILINEAR))[:ph, :pw]
        tint = rng.uniform(0.6, 1.2, 3)
        for c in range(3):
            img[y0:y0 + ph, x0:x0 + pw, c] += tex * tint[c]
    img += rng.normal(0, noise, img.shape)
    return np.clip(np.round(img), 0, 255).astype(np.uint8)


def jpeg(arr, q, **kw):
    b = io.BytesIO()
    Image.fromarray(arr).save(b, "JPEG", quality=q, **kw)
    return b.getvalue()


def decode(data):
    return np.asarray(Image.open(io.BytesIO(data)).convert("RGB"))


def main(out):
    out.mkdir(parents=True, exist_ok=True)
    (out / "pix-natural.jpg").write_bytes(jpeg(natural_like(1024, 768, 101), 92))

    base = natural_like(1024, 768, 102)
    low = decode(jpeg(base, 60))
    comp = base.copy()
    comp[224:544, 320:640] = low[224:544, 320:640]
    (out / "pix-spliced-ghost.jpg").write_bytes(jpeg(comp, 90))

    cm = natural_like(1024, 768, 103)
    rng = np.random.default_rng(7)
    lat = rng.uniform(0, 255, (34, 34)).astype(np.float32)
    patch = np.asarray(Image.fromarray(lat, mode="F").resize((136, 136), Image.BICUBIC))[:128, :128]
    for c in range(3):
        cm[300:428, 180:308, c] = np.clip(patch * (0.5 + 0.2 * c) + 40, 0, 255).astype(np.uint8)
    cm[300:428, 380:508] = cm[300:428, 180:308]
    (out / "pix-copy-move.jpg").write_bytes(jpeg(cm, 90))

    cam = decode(jpeg(natural_like(2400, 1800, 104), 95))
    small = Image.fromarray(cam).resize((1600, 1200), Image.LANCZOS)
    (out / "pix-whatsapp-like.jpg").write_bytes(jpeg(np.asarray(small), 80))


if __name__ == "__main__":
    main(Path(sys.argv[1]) if len(sys.argv) > 1 else Path("examples/sample/evidence"))
