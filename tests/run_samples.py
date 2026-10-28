#!/usr/bin/env python3
"""Analyses the whole sample evidence folder with the real CLI and checks the result end to end.

examples/sample/expected_findings.json holds the analyze options ("args", relative to the repository root),
the expected exit code and, per evidence file (path relative to examples/sample/evidence), the exact list of
finding codes (sorted). The checker:
  - checks the evidence against examples/sample/expected_hashes.csv (computed with Python's hashlib, not by
    the tool) before and after the runs: the evidence is never modified;
  - runs `rv-imgauth analyze -i examples/sample/evidence -r <args>` and checks the exit code, that every file is
    an item (crash.jpg included: a damaged file does not stop the run) and that each item reports EXACTLY the
    expected finding codes (no missing, no extra);
  - checks every output_written entry of the audit log against the file's SHA-256, and runs `verify-log`;
  - runs the analysis a second time and checks that the maps (maps/*.png), structure.json and metadata.json of
    every item are byte-identical (reproducibility).

Usage: python tests/run_samples.py <rv-imgauth binary> [work dir] [--bless]
       --bless rewrites the per-file codes of expected_findings.json from the first run (review the diff!).
"""

import csv
import hashlib
import json
import shutil
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SAMPLES = ROOT / "examples" / "sample"
EVIDENCE = SAMPLES / "evidence"
EXPECTED = SAMPLES / "expected_findings.json"


def sha256(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def evidence_digests() -> dict:
    return {p.relative_to(EVIDENCE).as_posix(): sha256(p) for p in sorted(EVIDENCE.rglob("*")) if p.is_file()}


def run(cmd):
    return subprocess.run([str(c) for c in cmd], capture_output=True, text=True, encoding="utf-8",
                          errors="replace", cwd=ROOT)


def analyze(binary, exp: dict, out: Path):
    if out.exists():
        shutil.rmtree(out)
    r = run([binary, "analyze", "-i", EVIDENCE, "-r", *exp["args"], "--out", out, "--progress", "none",
             "--no-timestamp", "--lang", "en", "--hash", "sha256", "--actor", "sample-check"])
    run_id = sorted(p.name for p in (out / "logs").iterdir())[-1] if (out / "logs").is_dir() else None
    return r, run_id


def reproducible_files(item_dir: Path) -> dict:
    files = [item_dir / "structure.json", item_dir / "metadata.json", *sorted((item_dir / "maps").glob("*.png"))]
    return {p.relative_to(item_dir).as_posix(): sha256(p) for p in files if p.is_file()}


def main() -> int:
    args = [a for a in sys.argv[1:] if a != "--bless"]
    bless = "--bless" in sys.argv
    if not args:
        print(__doc__)
        return 2
    binary = Path(args[0]).resolve()
    work = Path(args[1] if len(args) > 1 else "sample-check").resolve()
    exp = json.loads(EXPECTED.read_text(encoding="utf-8"))
    failures = []

    expected_hashes = {row["file"]: row["sha256"] for row in
                       csv.DictReader((SAMPLES / "expected_hashes.csv").read_text(encoding="utf-8").splitlines())}
    before = evidence_digests()
    if before != expected_hashes:
        failures.append(f"evidence differs from expected_hashes.csv: "
                        f"{sorted(set(before.items()) ^ set(expected_hashes.items()))[:4]}")

    r, run_id = analyze(binary, exp, work / "a")
    if r.returncode != exp["exit_code"] or run_id is None:
        print(r.stdout, r.stderr)
        raise SystemExit(f"analyze exited {r.returncode}, expected {exp['exit_code']}")
    reports = work / "a" / "reports" / run_id
    logs = work / "a" / "logs" / run_id
    stats = json.loads((reports / "statistics.json").read_text(encoding="utf-8"))
    got = {}
    folders = {}
    for it in stats["items"]:
        rel = Path(it["path"]).resolve().relative_to(EVIDENCE.resolve()).as_posix()
        got[rel] = sorted(it["findings"])
        folders[rel] = it["report_folder"]
    if bless:
        exp["files"] = got
        EXPECTED.write_text(json.dumps(exp, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"blessed {EXPECTED.relative_to(ROOT)} ({len(got)} files)")
    want = exp["files"]
    for f in sorted(set(want) | set(got) | set(before)):
        if f not in got:
            failures.append(f"{f}: not analysed")
        elif f not in want:
            failures.append(f"{f}: not in expected_findings.json (got {got[f]})")
        elif Counter(got[f]) != Counter(want[f]):
            missing = sorted((Counter(want[f]) - Counter(got[f])).elements())
            extra = sorted((Counter(got[f]) - Counter(want[f])).elements())
            failures.append(f"{f}: missing {missing}, unexpected {extra}")
        else:
            print(f"{f}: {len(got[f])} findings as expected")

    # Every derived file is hashed in the audit log; the log verifies.
    audit = [json.loads(line) for line in (logs / "audit.jsonl").read_text(encoding="utf-8").splitlines() if line.strip()]
    written = [e for e in audit if e["action"] == "output_written"]
    bad = [e["details"]["file"] for e in written if sha256(reports / e["details"]["file"]) != e["details"]["hashes"]["sha256"]]
    if not written or bad:
        failures.append(f"output_written digests: {len(written)} entries, mismatched {bad[:4]}")
    if audit[-1]["action"] != "run_finished":
        failures.append("the run did not finish (crash.jpg must not stop it)")
    v = run([binary, "verify-log", logs])
    if v.returncode != 0:
        failures.append(f"verify-log exit {v.returncode}: {v.stdout.strip()[-300:]}")
    else:
        print(v.stdout.strip().splitlines()[0])

    # Reproducibility: a second run gives byte-identical maps and structure/metadata JSON.
    r2, run_b = analyze(binary, exp, work / "b")
    if r2.returncode != exp["exit_code"] or run_b is None:
        failures.append(f"second run exited {r2.returncode}")
    else:
        reports_b = work / "b" / "reports" / run_b
        n = 0
        for f, folder in sorted(folders.items()):
            a, b = reproducible_files(reports / folder), reproducible_files(reports_b / folder)
            n += len(a)
            if a != b:
                diff = sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k))
                failures.append(f"{f}: second run differs in {diff}")
        print(f"reproducibility: {n} maps and JSON files compared")

    if evidence_digests() != before:
        failures.append("the evidence changed during the runs")
    for x in failures:
        print("FAIL", x)
    print("all samples OK" if not failures else f"{len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
