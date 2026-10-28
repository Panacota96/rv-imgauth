#!/usr/bin/env python3
"""Deterministic generator of the synthetic structure/metadata samples (agent A).

Writes, from generated scenes only (no real photos, no third-party images):
  examples/sample/evidence/{camera-original,edited-gimp,no-metadata,progressive,crash}.jpg,
  examples/sample/evidence/reference/ref-{1,2}.jpg, bad-order.png, screenshot.png, photo.webp,
  iphone-like.heic
  examples/sample/expected/<name>.json   expected parse result (format, size, signal codes, coefficients,
                                         thumbnail) checked by src/img/tests/parsers.rs
  tests/samples/fixtures/<name>.png      Pillow's (libjpeg) decoded pixels of three baseline JPEGs
                                         (camera-original, edited-gimp, no-metadata), stored as lossless PNG
                                         instead of raw .rgb to keep the repository small
  tests/samples/fixtures/<name>.qt.json  Pillow's quantization tables (Pillow >= 8.3 returns NATURAL order)

Requires Python 3, Pillow and numpy. Run from anywhere: python3 tests/samples/make_structure.py
Every byte is a function of the fixed seeds below, so re-running reproduces the same files (given the same
Pillow/libjpeg/libwebp versions; the expected JSON and fixtures are regenerated with the samples).
"""

import io
import json
import os
import struct
import zlib

import numpy as np
from PIL import Image

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
EVID = os.path.join(ROOT, "examples", "sample", "evidence")
EXPECT = os.path.join(ROOT, "examples", "sample", "expected")
FIX = os.path.join(ROOT, "tests", "samples", "fixtures")

# T.81 Annex K tables (natural order)
LUMA = [16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
        14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92,
        49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99]
CHROMA = [17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
          47, 66, 99, 99, 99, 99, 99, 99] + [99] * 32
# Synthetic "camera" tables: not IJG-scaled (a smooth ramp, like many camera firmwares)
CAM_LUMA = [max(1, min(255, 2 + (r + c) + (r * c) // 6)) for r in range(8) for c in range(8)]
CAM_CHROMA = [max(1, min(255, 3 + 2 * (r + c) + (r * c) // 3)) for r in range(8) for c in range(8)]


def scene(w, h, seed, paint=None):
    """Gradients, shapes and mild noise; `paint` = (x0, y0, x1, y1) painted over with a flat colour."""
    rng = np.random.default_rng(seed)
    y, x = np.mgrid[0:h, 0:w].astype(np.float32)
    r = 40 + 160 * x / w + 20 * np.sin(y / 37.0 + seed)
    g = 60 + 120 * y / h + 25 * np.cos(x / 53.0)
    b = 120 + 80 * np.sin((x + y) / 91.0 + seed / 3)
    img = np.stack([r, g, b], -1)
    for _ in range(9):
        cx, cy = rng.uniform(0, w), rng.uniform(0, h)
        rad = rng.uniform(min(w, h) * 0.04, min(w, h) * 0.18)
        col = rng.uniform(0, 255, 3)
        mask = (x - cx) ** 2 + (y - cy) ** 2 < rad ** 2
        img[mask] = img[mask] * 0.3 + col * 0.7
    for _ in range(6):
        x0, y0 = int(rng.uniform(0, w * 0.8)), int(rng.uniform(0, h * 0.8))
        x1, y1 = x0 + int(rng.uniform(20, w * 0.2)), y0 + int(rng.uniform(20, h * 0.2))
        img[y0:y1, x0:x1] = img[y0:y1, x0:x1] * 0.5 + rng.uniform(0, 255, 3) * 0.5
    img += rng.normal(0, 3.0, img.shape)
    if paint:
        x0, y0, x1, y1 = paint
        img[y0:y1, x0:x1] = (128, 140, 150)
    return Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), "RGB")


# ------------------------------------------------------------------------------------------- TIFF/EXIF

def tiff(ifd0, exif, ifd1=None, thumb=None, gps=None):
    """Little-endian TIFF: IFD0 (-> Exif IFD, GPS IFD), IFD1 with a JPEG thumbnail. Entries (tag, type, value)."""
    sizes = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 7: 1}

    def enc(typ, val):
        if typ == 2:
            b = val.encode() + b"\0"
            return b, len(b)
        if typ == 7:
            return val, len(val)
        if typ == 5:
            vals = val if isinstance(val, list) else [val]
            return b"".join(struct.pack("<II", n, d) for n, d in vals), len(vals)
        vals = val if isinstance(val, list) else [val]
        fmt = {1: "B", 3: "H", 4: "I"}[typ]
        return b"".join(struct.pack("<" + fmt, v) for v in vals), len(vals)

    def size(entries):
        n = 2 + 12 * len(entries) + 4
        for t, typ, v in entries:
            b, _ = enc(typ, v)
            if len(b) > 4:
                n += len(b) + (len(b) & 1)
        return n

    ifd0 = list(ifd0) + [(0x8769, 4, 0)] + ([(0x8825, 4, 0)] if gps else [])
    ifd1 = list(ifd1 or [])
    if thumb is not None:
        ifd1 += [(0x0201, 4, 0), (0x0202, 4, len(thumb))]
    o0 = 8
    oe = o0 + size(ifd0)
    og = oe + size(exif)
    o1 = og + (size(gps) if gps else 0)
    ot = o1 + (size(ifd1) if ifd1 else 0)

    def fix(entries, tag, val):
        return [(t, ty, val if t == tag else v) for t, ty, v in entries]

    ifd0 = fix(ifd0, 0x8769, oe)
    if gps:
        ifd0 = fix(ifd0, 0x8825, og)
    if thumb is not None:
        ifd1 = fix(ifd1, 0x0201, ot)

    def build(entries, off, nxt):
        entries = sorted(entries)
        head = struct.pack("<H", len(entries))
        data = b""
        dbase = off + 2 + 12 * len(entries) + 4
        for t, typ, v in entries:
            b, cnt = enc(typ, v)
            if len(b) <= 4:
                head += struct.pack("<HHI", t, typ, cnt) + b.ljust(4, b"\0")
            else:
                head += struct.pack("<HHII", t, typ, cnt, dbase + len(data))
                data += b + (b"\0" if len(b) & 1 else b"")
        return head + struct.pack("<I", nxt) + data

    out = b"II*\0" + struct.pack("<I", 8)
    out += build(ifd0, o0, o1 if ifd1 else 0)
    out += build(exif, oe, 0)
    if gps:
        out += build(gps, og, 0)
    if ifd1:
        out += build(ifd1, o1, 0)
    if thumb is not None:
        assert len(out) == ot
        out += thumb
    return out


def seg(marker, payload):
    return bytes([0xFF, marker]) + struct.pack(">H", len(payload) + 2) + payload


def jpeg_bytes(im, **kw):
    b = io.BytesIO()
    im.save(b, "JPEG", **kw)
    return b.getvalue()


def strip_app0(j):
    """Removes a leading APP0 segment after SOI."""
    assert j[:2] == b"\xff\xd8"
    if j[2:4] == b"\xff\xe0":
        n = struct.unpack(">H", j[4:6])[0]
        return j[:2] + j[4 + n:]
    return j


def insert_after_soi(j, segs, keep_app0=True):
    """Inserts segments after SOI (after APP0 when kept)."""
    if keep_app0 and j[2:4] == b"\xff\xe0":
        n = struct.unpack(">H", j[4:6])[0]
        return j[:4 + n] + b"".join(segs) + j[4 + n:]
    return j[:2] + b"".join(segs) + strip_app0(j)[2:]


def makernote(seed, n=256):
    rng = np.random.default_rng(seed)
    return b"SYNTHETIC-MN\0" + rng.integers(0, 256, n, dtype=np.uint8).tobytes()


def camera_exif(w, h, when, thumb, software=None, changed=None):
    ifd0 = [(0x010F, 2, "Canon"), (0x0110, 2, "Canon EOS R6"), (0x0112, 3, 1),
            (0x011A, 5, (72, 1)), (0x011B, 5, (72, 1)), (0x0128, 3, 2),
            (0x0132, 2, changed or when)]
    if software:
        ifd0.append((0x0131, 2, software))
    exif = [(0x829A, 5, (1, 250)), (0x829D, 5, (56, 10)), (0x8827, 3, 400), (0x9000, 7, b"0232"),
            (0x9003, 2, when), (0x9004, 2, when), (0x9010, 2, "+02:00"), (0x9011, 2, "+02:00"),
            (0x9012, 2, "+02:00"), (0x927C, 7, makernote(7)), (0xA001, 3, 1),
            (0xA002, 4, w), (0xA003, 4, h)]
    ifd1 = [(0x0103, 3, 6), (0x011A, 5, (72, 1)), (0x011B, 5, (72, 1)), (0x0128, 3, 2)]
    return tiff(ifd0, exif, ifd1, thumb)


def camera_jpeg(im, when):
    w, h = im.size
    th = im.resize((160, 120), Image.BILINEAR)
    thumb = jpeg_bytes(th, qtables=[CAM_LUMA, CAM_CHROMA], subsampling=2)
    thumb = strip_app0(thumb)
    body = jpeg_bytes(im, qtables=[CAM_LUMA, CAM_CHROMA], subsampling=2, restart_marker_rows=1)
    exif = b"Exif\0\0" + camera_exif(w, h, when, thumb)
    return insert_after_soi(body, [seg(0xE1, exif)], keep_app0=False), thumb


XMP_GIMP = """<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:stEvt="http://ns.adobe.com/xap/1.0/sType/ResourceEvent#"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:GIMP="http://www.gimp.org/xmp/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
   xmpMM:DocumentID="gimp:docid:gimp:0b6c1a59-synthetic"
   xmpMM:InstanceID="xmp.iid:5e1f-synthetic-2"
   xmpMM:OriginalDocumentID="xmp.did:5e1f-synthetic-0"
   dc:Format="image/jpeg"
   GIMP:API="2.0"
   GIMP:Platform="Linux"
   GIMP:Version="2.10.36"
   xmp:CreatorTool="GIMP 2.10"
   xmp:ModifyDate="2026-05-16T18:02:10+02:00"
   xmp:MetadataDate="2026-05-16T18:02:10+02:00">
   <xmpMM:History>
    <rdf:Seq>
     <rdf:li
      stEvt:action="saved"
      stEvt:changed="/"
      stEvt:instanceID="xmp.iid:5e1f-synthetic-1"
      stEvt:softwareAgent="Gimp 2.10 (Linux)"
      stEvt:when="2026-05-16T17:40:02+02:00"/>
     <rdf:li
      stEvt:action="saved"
      stEvt:changed="/"
      stEvt:instanceID="xmp.iid:5e1f-synthetic-2"
      stEvt:softwareAgent="Gimp 2.10 (Linux)"
      stEvt:when="2026-05-16T18:02:10+02:00"/>
    </rdf:Seq>
   </xmpMM:History>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"""


# ------------------------------------------------------------------------------------------------ PNG

def chunk(ty, body):
    return struct.pack(">I", len(body)) + ty + body + struct.pack(">I", zlib.crc32(ty + body) & 0xFFFFFFFF)


def png_parts(im):
    b = io.BytesIO()
    im.save(b, "PNG", optimize=False, compress_level=6)
    d = b.getvalue()
    pos, out = 8, []
    while pos < len(d):
        n = struct.unpack(">I", d[pos:pos + 4])[0]
        out.append((d[pos + 4:pos + 8], d[pos + 8:pos + 8 + n]))
        pos += 12 + n
    return out


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data)


def fixtures(name, data, pixels=True):
    im = Image.open(io.BytesIO(data))
    im.load()
    q = {str(k): list(v) for k, v in im.quantization.items()}
    write(os.path.join(FIX, name + ".qt.json"), json.dumps(q, indent=0).encode())
    if not pixels:
        return
    b = io.BytesIO()
    im.convert("RGB").save(b, "PNG", compress_level=9)
    write(os.path.join(FIX, name + ".png"), b.getvalue())


def expected(name, fmt, w, h, signals, coefficients, thumbnail):
    write(os.path.join(EXPECT, os.path.basename(name) + ".json"), (json.dumps({
        "file": name, "format": fmt, "width": w, "height": h, "signals": sorted(signals),
        "coefficients": coefficients, "thumbnail": thumbnail}, indent=2) + "\n").encode())


# ---------------------------------------------------------------------------------------------- HEIF

def box(ty, body):
    return struct.pack(">I", len(body) + 8) + ty + body


def fbox(ty, v, flags, body):
    return box(ty, bytes([v]) + flags.to_bytes(3, "big") + body)


def heic():
    rng = np.random.default_rng(2026)
    tiles = [rng.integers(0, 256, 2000, dtype=np.uint8).tobytes() for _ in range(4)]
    thumb = rng.integers(0, 256, 800, dtype=np.uint8).tobytes()
    ex = tiff([(0x010F, 2, "Apple"), (0x0110, 2, "iPhone 13"), (0x0112, 3, 1), (0x0131, 2, "17.5.1"),
               (0x0132, 2, "2026:04:02 15:10:44")],
              [(0x9003, 2, "2026:04:02 15:10:44"), (0x9004, 2, "2026:04:02 15:10:44"),
               (0x9010, 2, "+02:00"), (0x9011, 2, "+02:00"), (0x9012, 2, "+02:00"),
               (0x927C, 7, b"Apple iOS\0" + makernote(13, 120)), (0xA002, 4, 1024), (0xA003, 4, 768)])
    exif_item = struct.pack(">I", 6) + b"Exif\0\0" + ex
    junk = b"\xA5" * 123  # bytes inside mdat that no item uses
    grid = bytes([0, 0, 1, 1]) + struct.pack(">HH", 1024, 768)  # 2x2 grid, 16-bit output size

    ftyp = box(b"ftyp", b"heic" + b"\0\0\0\0" + b"mif1" + b"miaf" + b"MiHB" + b"heic")
    hdlr = fbox(b"hdlr", 0, 0, b"\0\0\0\0pict" + b"\0" * 12 + b"\0")
    pitm = fbox(b"pitm", 0, 0, struct.pack(">H", 1))

    def infe(i, ty, hidden, name=b""):
        return fbox(b"infe", 2, 1 if hidden else 0, struct.pack(">HH", i, 0) + ty + name + b"\0")

    items = [infe(1, b"grid", False), *[infe(i, b"hvc1", True) for i in range(2, 6)],
             infe(6, b"hvc1", False), infe(7, b"Exif", True)]
    iinf = fbox(b"iinf", 0, 0, struct.pack(">H", len(items)) + b"".join(items))

    def ref(ty, frm, tos):
        return box(ty, struct.pack(">HH", frm, len(tos)) + b"".join(struct.pack(">H", t) for t in tos))

    iref = fbox(b"iref", 0, 0, ref(b"dimg", 1, [2, 3, 4, 5]) + ref(b"thmb", 6, [1]) + ref(b"cdsc", 7, [1]))
    ispe = lambda w, h: fbox(b"ispe", 0, 0, struct.pack(">II", w, h))
    hvcc = box(b"hvcC", bytes([1, 1, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 90, 0xF0, 0, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 0x0F, 0]))
    colr = box(b"colr", b"nclx" + struct.pack(">HHHB", 12, 13, 6, 0x80))
    pixi = fbox(b"pixi", 0, 0, bytes([3, 8, 8, 8]))
    # property indices (1-based): 1 hvcC, 2 ispe 512x512, 3 ispe 512x384, 4 ispe 1024x768, 5 ispe 320x240,
    # 6 colr, 7 pixi
    ipco = box(b"ipco", hvcc + ispe(512, 512) + ispe(512, 384) + ispe(1024, 768) + ispe(320, 240) + colr + pixi)
    assoc = {1: [4, 6, 7], 2: [0x81, 2], 3: [0x81, 2], 4: [0x81, 2], 5: [0x81, 3], 6: [0x81, 5, 6], 7: []}
    ipma_b = struct.pack(">I", len(assoc))
    for i, a in assoc.items():
        ipma_b += struct.pack(">HB", i, len(a)) + bytes(a)
    ipma = fbox(b"ipma", 0, 0, ipma_b)
    iprp = box(b"iprp", ipco + ipma)
    idat = box(b"idat", grid)

    def iloc(mdat_payload_start):
        # version 1: offset 4, length 4, base 0, index 0
        entries = []
        entries.append((1, 1, [(0, len(grid))]))  # grid in idat (construction method 1)
        off = mdat_payload_start
        for i, t in enumerate(tiles):
            entries.append((2 + i, 0, [(off, len(t))]))
            off += len(t)
        entries.append((6, 0, [(off, len(thumb))]))
        off += len(thumb)
        entries.append((7, 0, [(off, len(exif_item))]))
        b = bytes([0x44, 0x00]) + struct.pack(">H", len(entries))
        for i, m, ext in entries:
            b += struct.pack(">HHHH", i, m, 0, len(ext))
            for o, l in ext:
                b += struct.pack(">II", o, l)
        return fbox(b"iloc", 1, 0, b)

    def meta(mdat_start):
        return fbox(b"meta", 0, 0, hdlr + pitm + iloc(mdat_start) + iinf + iref + iprp + idat)

    m = meta(0)
    mdat_payload = b"".join(tiles) + thumb + exif_item + junk
    start = len(ftyp) + len(m) + 8
    m = meta(start)
    assert len(ftyp) + len(m) + 8 == start
    return ftyp + m + box(b"mdat", mdat_payload)


# ---------------------------------------------------------------------------------------------- main

def main():
    for old in ("acta.txt", "acta-copia.txt", "foto-vacaciones.jpg"):
        p = os.path.join(EVID, old)
        if os.path.exists(p):
            os.remove(p)
    W, H = 1200, 900

    # camera original
    cam = scene(W, H, 1)
    data, thumb = camera_jpeg(cam, "2026:05:14 10:21:33")
    write(os.path.join(EVID, "camera-original.jpg"), data)
    fixtures("camera-original", data)
    expected("camera-original.jpg", "jpeg", W, H, [], "ok", True)

    # references: same camera tables and EXIF layout, other scenes
    for i, when in ((1, "2026:05:14 10:25:02"), (2, "2026:05:14 10:31:47")):
        d, _ = camera_jpeg(scene(W, H, 10 + i), when)
        write(os.path.join(EVID, "reference", f"ref-{i}.jpg"), d)
        fixtures(f"ref-{i}", d, pixels=False)
        expected(f"reference/ref-{i}.jpg", "jpeg", W, H, [], "ok", True)

    # edited in "GIMP": painted rectangle, IJG q85, old thumbnail kept, Software + XMP history
    ed = scene(W, H, 1, paint=(700, 300, 900, 450))
    body = jpeg_bytes(ed, quality=85, subsampling=2)
    exif = b"Exif\0\0" + camera_exif(W, H, "2026:05:14 10:21:33", thumb, software="GIMP 2.10.36",
                                     changed="2026:05:16 18:02:10")
    xmp = b"http://ns.adobe.com/xap/1.0/\0" + XMP_GIMP.encode()
    d = insert_after_soi(body, [seg(0xE1, exif), seg(0xE1, xmp), seg(0xFE, b"Created with GIMP")])
    write(os.path.join(EVID, "edited-gimp.jpg"), d)
    fixtures("edited-gimp", d)
    expected("edited-gimp.jpg", "jpeg", W, H,
             ["EXIF_SOFTWARE_EDITOR", "XMP_EDIT_HISTORY", "METADATA_DATES_INCONSISTENT"], "ok", True)

    # no metadata (JFIF only)
    d = jpeg_bytes(scene(800, 600, 3), quality=75)
    write(os.path.join(EVID, "no-metadata.jpg"), d)
    fixtures("no-metadata", d)
    expected("no-metadata.jpg", "jpeg", 800, 600, ["METADATA_STRIPPED"], "ok", False)

    # progressive web export
    d = jpeg_bytes(scene(800, 600, 4), quality=80, progressive=True, optimize=True)
    write(os.path.join(EVID, "progressive.jpg"), d)
    expected("progressive.jpg", "jpeg", 800, 600, ["METADATA_STRIPPED", "PROGRESSIVE_ENCODING"],
             "unsupported", False)

    # crash: SOI + DQT whose length runs past the end of the file
    d = b"\xff\xd8\xff\xdb\x00\x43\x00" + bytes(range(1, 9))
    write(os.path.join(EVID, "crash.jpg"), d)
    expected("crash.jpg", "jpeg", 0, 0, ["METADATA_PROBLEMS"], "error", False)

    # bad-order PNG: iCCP after PLTE, IDAT split by tEXt, private chunk, data after IEND
    parts = png_parts(scene(320, 240, 5))
    ihdr = [b for t, b in parts if t == b"IHDR"][0]
    idat = b"".join(b for t, b in parts if t == b"IDAT")
    half = len(idat) // 2
    plte = bytes(np.arange(48, dtype=np.uint8) * 5)
    icc = b"synthetic ICC profile bytes (not a real profile)" * 4
    d = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"PLTE", plte) \
        + chunk(b"iCCP", b"synthetic\0\0" + zlib.compress(icc)) \
        + chunk(b"zTXt", b"Comment\0\0" + zlib.compress(b"synthetic sample for rv-imgauth")) \
        + chunk(b"IDAT", idat[:half]) + chunk(b"tEXt", b"Title\0split") + chunk(b"IDAT", idat[half:]) \
        + chunk(b"rkVr", b"private data") + chunk(b"IEND", b"") + b"synthetic trailer after IEND\n"
    write(os.path.join(EVID, "bad-order.png"), d)
    expected("bad-order.png", "png", 320, 240,
             ["PNG_CHUNK_ORDER_INVALID", "PNG_PRIVATE_CHUNKS", "DATA_AFTER_END_MARKER"], "unsupported", False)

    # screenshot PNG: eXIf dates 3 days before tIME
    parts = png_parts(scene(400, 300, 6))
    ihdr = [b for t, b in parts if t == b"IHDR"][0]
    idat = b"".join(b for t, b in parts if t == b"IDAT")
    ex = tiff([(0x0131, 2, "Screenshot tool (synthetic)"), (0x0132, 2, "2026:03:01 09:15:00")],
              [(0x9003, 2, "2026:03:01 09:15:00"), (0xA002, 4, 400), (0xA003, 4, 300)])
    xmp = ('<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
           '<rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/"><exif:UserComment><rdf:Alt>'
           '<rdf:li xml:lang="x-default">Screenshot</rdf:li></rdf:Alt></exif:UserComment></rdf:Description>'
           '</rdf:RDF></x:xmpmeta>').encode()
    d = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"pHYs", struct.pack(">IIB", 5669, 5669, 1)) \
        + chunk(b"eXIf", ex) + chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0" + xmp) \
        + chunk(b"tEXt", b"Software\0Screenshot tool (synthetic)") \
        + chunk(b"tIME", struct.pack(">HBBBBB", 2026, 3, 4, 8, 0, 0)) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")
    write(os.path.join(EVID, "screenshot.png"), d)
    expected("screenshot.png", "png", 400, 300, ["METADATA_DATES_INCONSISTENT"], "unsupported", False)

    # WebP: VP8X says EXIF but there is no EXIF chunk
    b = io.BytesIO()
    scene(640, 480, 7).save(b, "WEBP", quality=80, method=4)
    w = b.getvalue()
    assert w[12:16] == b"VP8 ", w[12:16]
    n = struct.unpack("<I", w[16:20])[0]
    vp8 = w[12:20 + n + (n & 1)]
    vp8x = b"VP8X" + struct.pack("<I", 10) + bytes([0x08, 0, 0, 0]) + (639).to_bytes(3, "little") \
        + (479).to_bytes(3, "little")
    body = b"WEBP" + vp8x + vp8
    d = b"RIFF" + struct.pack("<I", len(body)) + body
    write(os.path.join(EVID, "photo.webp"), d)
    expected("photo.webp", "webp", 640, 480, ["WEBP_FLAGS_MISMATCH"], "unsupported", False)

    # HEIC (structure only; payloads are random bytes, no real HEVC)
    d = heic()
    write(os.path.join(EVID, "iphone-like.heic"), d)
    expected("iphone-like.heic", "heif", 1024, 768, ["HEIF_STRUCTURE_ANOMALY", "HEIF_UNREFERENCED_DATA"],
             "unsupported", False)


if __name__ == "__main__":
    main()
