#!/usr/bin/env python3
"""Independent ExifTool oracle for the metadata rv-imgauth reports (EXIF, XMP, IPTC, PNG text, JPEG comment).

usage: python3 tests/oracle_exiftool.py <OUT>/reports/<RUN-ID>      (needs `exiftool` on PATH; Debian/Ubuntu:
       apt-get install libimage-exiftool-perl)

For every item folder with a metadata.json, the evidence file named in technical.json is read with
`exiftool -j -G1 -a -n -D` (group 1 names, duplicates kept, numeric values, tag ids) and EVERY tag we report is
looked up and compared; tags ExifTool reports and we do not are not an error (metadata.json is not meant to
list ExifTool's composite, maker-note or file-system tags). Matching:
  EXIF   our group IFD0 / Exif / GPS / IFD1 / Interop and numeric tag id  ->  IFD0 / ExifIFD / GPS / IFD1 /
         InteropIFD with the same id (names differ by design: we keep the EXIF 2.32 names, e.g. DateTime where
         ExifTool says ModifyDate, PixelXDimension where it says ExifImageWidth);
  IPTC   dataset number (our id is record*1000 + dataset);
  XMP    "prefix:Name"  ->  "XMP-prefix:Name" (case-insensitive: ExifTool re-capitalises, GIMP:API -> Api);
  PNG    tEXt / zTXt / iTXt keyword  ->  "PNG:<keyword without spaces>"; tIME  ->  PNG:ModifyDate;
  COM    JPEG comment  ->  File:Comment;
  xmpMM:History  ->  the XMP-xmpMM:History* lists, event by event (action, when, software agent, changed,
         instance id) from metadata.json's xmp_history.
Intentional differences (normalised, documented here, never silently ignored):
  - enumerated values: we write "1 (row 0 at top and column 0 at left)", ExifTool -n writes 1: the number
    before " (" is compared;
  - rationals: we write "1/250", ExifTool -n writes 0.004: compared as numbers (relative tolerance 1e-6);
  - version tags (ExifVersion, FlashpixVersion, InteropVersion): we write "2.32", ExifTool "0232": compared
    without dots and leading zeros;
  - XMP and PNG dates: we keep the stored text ("2026-05-16T18:02:10+02:00"), ExifTool rewrites it in EXIF
    style ("2026:05:16 18:02:10+02:00"): compared after mapping '-' to ':' and 'T' to ' ';
  - JPEGInterchangeFormat / StripOffsets: we report the offset as stored (relative to the TIFF header);
    ExifTool adds the header's position in the file: ours + position of the TIFF header must equal ExifTool's;
  - binary values (MakerNote, undefined arrays) are reported by us as "<N bytes>" and are not compared (ExifTool
    decodes maker notes instead); WebP and HEIF container facts have no ExifTool equivalent and are skipped.
Exit 0 when every compared tag agrees, 1 otherwise.
"""
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

EXIF_GROUPS = {"IFD0": "IFD0", "Exif": "ExifIFD", "GPS": "GPS", "IFD1": "IFD1", "Interop": "InteropIFD"}
OFFSET_IDS = {513, 273}
VERSION_IDS = {36864, 40960, 2}  # ExifVersion, FlashpixVersion, InteropVersion (Interop id 2)


def num(s):
    s = str(s).strip()
    m = re.fullmatch(r"(-?\d+)/(\d+)", s)
    if m:
        return int(m.group(1)) / int(m.group(2)) if int(m.group(2)) else None
    try:
        return float(s)
    except ValueError:
        return None


def date_norm(s: str) -> str:
    return str(s).replace("-", ":").replace("T", " ").strip()


def same(ours: str, theirs, tag_id=None, group="") -> bool:
    if isinstance(theirs, list):
        theirs = ", ".join(str(x) for x in theirs)
    o = str(ours).strip().rstrip("\x00")
    t = str(theirs).strip().rstrip("\x00")
    if o == t:
        return True
    if tag_id in VERSION_IDS and group in ("IFD0", "Exif", "Interop"):
        return o.replace(".", "").lstrip("0") == t.replace(".", "").lstrip("0")
    head = o.split(" (", 1)[0]
    a, b = num(head), num(t)
    if a is not None and b is not None:
        return abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))
    if date_norm(o) == date_norm(t):
        return True
    # Multi-valued EXIF numbers: we join with ", ", ExifTool -n with spaces.
    if re.fullmatch(r"[-\d./, ]+", o) and re.fullmatch(r"[-\d. ]+", t):
        xs = [num(x) for x in re.split(r"[ ,]+", o) if x]
        ys = [num(y) for y in t.split()]
        return len(xs) == len(ys) and all(x is not None and y is not None and abs(x - y) <= 1e-6 * max(1.0, abs(x)) for x, y in zip(xs, ys))
    return False


def exiftool(path: Path) -> dict:
    r = subprocess.run(["exiftool", "-j", "-G1", "-a", "-n", "-D", str(path)], capture_output=True, text=True,
                       encoding="utf-8", errors="replace")
    data = json.loads(r.stdout or "[{}]")[0]
    out = {}
    for k, v in data.items():
        if isinstance(v, dict) and "val" in v:
            out[k] = v
        else:
            out[k] = {"id": None, "val": v}
    return out


def tiff_base(path: Path) -> int:
    b = path.read_bytes()
    i = b.find(b"Exif\x00\x00")
    if i >= 0:
        return i + 6
    for magic in (b"II*\x00", b"MM\x00*"):
        j = b.find(magic)
        if j >= 0:
            return j
    return 0


def check_item(item: Path):
    md = json.loads((item / "metadata.json").read_text(encoding="utf-8"))
    tech = json.loads((item / "technical.json").read_text(encoding="utf-8"))
    src = Path(tech["item"]["path"])
    et = exiftool(src)
    by_id = {}
    by_name = {}
    for k, v in et.items():
        g, _, name = k.partition(":")
        if v.get("id") is not None:
            by_id.setdefault((g, str(v["id"])), []).append(v["val"])
        by_name.setdefault((g.lower(), name.lower()), []).append(v["val"])
    problems, compared, skipped = [], 0, []
    base = None
    for t in md.get("tags", []):
        g, name, value, tid = t["group"], t["name"], t["value"], t.get("id", 0)
        if re.fullmatch(r"<\d+ bytes>", value):
            skipped.append(f"{g}:{name} (binary)")
            continue
        if g == "XMP" and name == "xmpMM:History":
            continue  # compared event by event below
        if g in EXIF_GROUPS:
            cands = by_id.get((EXIF_GROUPS[g], str(tid)), [])
            if tid in OFFSET_IDS and cands:
                base = tiff_base(src) if base is None else base
                ok = any(num(c) == num(value) + base for c in cands)
                compared += 1
                if not ok:
                    problems.append(f"{g}:{name} = {value} (+{base}) vs ExifTool {cands}")
                continue
        elif g == "IPTC":
            cands = by_id.get(("IPTC", str(tid % 1000)), [])
        elif g == "XMP":
            prefix, _, local = name.partition(":")
            cands = by_name.get((f"xmp-{prefix}".lower(), local.lower()), [])
        elif g.startswith("PNG-"):
            key = "modifydate" if name == "tIME" else name.replace(" ", "").lower()
            cands = by_name.get(("png", key), [])
        elif g == "COM":
            cands = by_name.get(("file", "comment"), [])
        else:
            skipped.append(f"{g}:{name} (no ExifTool equivalent)")
            continue
        compared += 1
        if not cands:
            problems.append(f"{g}:{name} = {value!r}: not reported by ExifTool")
        elif not any(same(value, c, tid, g) for c in cands):
            problems.append(f"{g}:{name} = {value!r} vs ExifTool {cands}")
    hist = md.get("xmp_history", [])
    if hist:
        fields = {"action": "HistoryAction", "when": "HistoryWhen", "software_agent": "HistorySoftwareAgent",
                  "changed": "HistoryChanged", "instance_id": "HistoryInstanceID"}
        for ours, theirs in fields.items():
            vals = (by_name.get(("xmp-xmpmm", theirs.lower())) or [[]])[0]
            vals = vals if isinstance(vals, list) else [vals]
            mine = [h.get(ours, "") for h in hist]
            present = [m for m in mine if m]
            compared += 1
            if len(present) and (len(vals) != len(present) or not all(same(a, b) for a, b in zip(present, vals))):
                problems.append(f"xmpMM:History {ours}: {mine} vs ExifTool {vals}")
    return compared, problems, skipped


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    if not shutil.which("exiftool"):
        print("exiftool not found on PATH (apt-get install libimage-exiftool-perl)")
        return 2
    reports = Path(sys.argv[1])
    ok = True
    total = 0
    for item in sorted((reports / "items").iterdir()):
        if not (item / "metadata.json").is_file():
            continue
        n, problems, skipped = check_item(item)
        total += n
        note = f"; skipped {', '.join(skipped)}" if skipped else ""
        print(f"{item.name}: {n} tags compared, {len(problems)} differ{note}")
        for p in problems:
            print(f"  DIFF {p}")
        ok &= not problems
    print(f"{total} tags compared: " + ("PASS" if ok else "FAIL"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
