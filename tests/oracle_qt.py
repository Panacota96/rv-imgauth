#!/usr/bin/env python3
"""Independent Pillow oracle for the JPEG quantization tables rv-imgauth reports.

usage: python3 tests/oracle_qt.py <OUT>/reports/<RUN-ID>

For every item whose structure.json holds a JPEG, the evidence file named in technical.json is opened with
Pillow and `Image.quantization` ({table id: 64 values}, natural row-major order in current Pillow) is compared
with our DQT tables (structure.json `jpeg.qtables`, natural order; when an id is defined twice the last
definition is the one in force, as in Pillow). The table ids, the precision-independent values and the number
of distinct ids must all agree. A file Pillow cannot open (crash.jpg) is reported as skipped together with the
number of tables we read from it. Exit 0 when every JPEG agrees, 1 otherwise.
"""
import json
import sys
from pathlib import Path

import PIL
from PIL import Image


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    reports = Path(sys.argv[1])
    ok, checked = True, 0
    for item in sorted((reports / "items").iterdir()):
        st = item / "structure.json"
        if not st.is_file():
            continue
        s = json.loads(st.read_text(encoding="utf-8"))
        if s.get("format") != "jpeg" or not s.get("jpeg"):
            continue
        ours = {}
        for t in s["jpeg"].get("qtables", []):
            ours[t["id"]] = list(t["values"])
        src = Path(json.loads((item / "technical.json").read_text(encoding="utf-8"))["item"]["path"])
        try:
            with Image.open(src) as im:
                theirs = {k: list(v) for k, v in im.quantization.items()}
        except Exception as e:  # noqa: BLE001 - any decoder refusal is a skip, reported
            print(f"{item.name}: skipped, Pillow cannot open it ({e}); we read {len(ours)} table(s)")
            continue
        checked += 1
        if ours == theirs:
            print(f"{item.name}: {len(ours)} table(s) identical")
        else:
            ok = False
            print(f"{item.name}: DIFFERENT")
            for k in sorted(set(ours) | set(theirs)):
                if ours.get(k) != theirs.get(k):
                    print(f"  table {k}: ours {ours.get(k)}\n           Pillow {theirs.get(k)}")
    print(f"{checked} JPEG(s) compared with Pillow {PIL.__version__}: "
          + ("PASS" if ok and checked else "FAIL"))
    return 0 if ok and checked else 1


if __name__ == "__main__":
    sys.exit(main())
