#!/usr/bin/env sh
# Runs the sample review of the whole evidence folder against the two reference images, then checks the
# result. Usage: examples/sample/run.sh [path-to-rv-imgauth]
# Finding by finding check (exact codes per file, reproducibility): python3 tests/run_samples.py <binary>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-rv-imgauth}
out="$here/out"
ev="$here/evidence"
"$bin" self-test
# Expected exit code 3: pix-copy-move.jpg has a high-severity finding (COPY_MOVE_CANDIDATE); crash.jpg is
# damaged and must not stop the run. See examples/README.md and expected_findings.json.
rc=0
"$bin" analyze --input "$ev" --recursive --maps ela,noise,ghosts \
  --reference "$ev/reference/ref-1.jpg" --reference "$ev/reference/ref-2.jpg" \
  --hash sha256,md5 --lang all --case DEMO-001 --actor "Sample" --out "$out" --progress none || rc=$?
if [ "$rc" -ne 3 ]; then echo "unexpected exit code $rc (expected 3)" >&2; exit 1; fi
run=$(ls "$out/logs" | tail -n 1)
"$bin" verify-log "$out/logs/$run"
# Compare the evidence with the independently computed hashes (unchanged by the run).
if command -v sha256sum >/dev/null 2>&1; then
  tail -n +2 "$here/expected_hashes.csv" | while IFS=, read -r file size md5 sha1 sha256 rest; do
    actual=$(sha256sum "$ev/$file" | cut -d' ' -f1)
    if [ "$actual" != "$sha256" ]; then echo "SHA-256 mismatch for $file" >&2; exit 1; fi
  done
fi
echo "Reports: $out/reports/$run/statistics.en.html"
