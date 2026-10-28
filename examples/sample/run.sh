#!/usr/bin/env sh
# Runs the sample review and checks the result. Usage: examples/sample/run.sh [path-to-rv-imgauth]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-rv-imgauth}
out="$here/out"
"$bin" self-test
# Expected exit code 0: the sample set has no high-severity finding (a renamed file is medium; see examples/README.md).
rc=0
"$bin" analyze --input "$here/evidence" --hash sha256,md5 --lang all --case DEMO-001 --actor "Sample" --out "$out" --progress none || rc=$?
if [ "$rc" -ne 0 ]; then echo "unexpected exit code $rc (expected 0)" >&2; exit 1; fi
run=$(ls "$out/logs" | tail -n 1)
"$bin" verify-log "$out/logs/$run"
echo "Reports: $out/reports/$run/statistics.en.html"
