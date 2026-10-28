# Runs the sample review of the whole evidence folder against the two reference images on Windows, then
# checks the result. Usage: .\examples\sample\run.ps1 [-Bin C:\path\to\rv-imgauth.exe]
# Finding by finding check (exact codes per file, reproducibility): python tests\run_samples.py <binary>
param([string]$Bin = "rv-imgauth")
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$out = Join-Path $here "out"
$ev = Join-Path $here "evidence"
& $Bin self-test; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
# Expected exit code 3: pix-copy-move.jpg has a high-severity finding (COPY_MOVE_CANDIDATE); crash.jpg is
# damaged and must not stop the run. See examples\README.md and expected_findings.json.
& $Bin analyze --input $ev --recursive --maps ela,noise,ghosts `
  --reference (Join-Path $ev "reference\ref-1.jpg") --reference (Join-Path $ev "reference\ref-2.jpg") `
  --hash sha256,md5 --lang all --case DEMO-001 --actor "Sample" --out $out --progress none
if ($LASTEXITCODE -ne 3) { Write-Error "unexpected exit code $LASTEXITCODE (expected 3)"; exit 1 }
$run = Get-ChildItem (Join-Path $out "logs") | Sort-Object Name | Select-Object -Last 1
& $Bin verify-log $run.FullName; if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
# Compare the evidence with the independently computed hashes (unchanged by the run)
$expected = Import-Csv (Join-Path $here "expected_hashes.csv")
foreach ($row in $expected) {
  $actual = (Get-FileHash (Join-Path $ev $row.file) -Algorithm SHA256).Hash.ToLower()
  if ($actual -ne $row.sha256) { Write-Error "SHA-256 mismatch for $($row.file)"; exit 1 }
}
Write-Host "Reports: $out\reports\$($run.Name)\statistics.en.html"
