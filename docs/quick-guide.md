# Quick guide

RV ImgAuth looks for signals that a photo or image (JPEG, HEIC/HEIF, PNG, WebP) was modified after it was captured.
It reads each piece of evidence you give it (one file, a folder or a list of files) without changing it, hashes it,
and examines every image in a separate worker process: the file structure, the metadata (EXIF, XMP, IPTC, PNG text),
the JPEG quantization tables and their fingerprints, double compression, copy-move, the embedded thumbnail, and
optionally known reference images. It also writes visual maps (ELA, noise, JPEG ghosts) for the examiner. It writes
reports for the examiner and for the client, and records every action in a hash-chained audit log. Its findings are
**signals, not verdicts**: each one says what it shows and what it does not prove. An image without findings is not
proven authentic, and a finding does not prove manipulation, nor show who changed the image or why. The maps are
visual aids, not results. HEIC pixels are not decoded in this version. This page covers the six common tasks. The full
guide is [usage.md](usage.md), and every option is in [usage-cli.md](usage-cli.md).

```mermaid
flowchart LR
    E[/"Images<br/>file, folder or list"/] --> A["1 · analyze"]
    E --> AC["2 · analyze --case-dir"]
    A --> O[("rakvor-out/<br/>logs + reports + maps")]
    AC --> C[("Case folder<br/>registry + case log + runs")]
    O --> VL["3 · verify-log"]
    C --> VC["4 · verify-case"]
    O -. "offline run" .-> TS["5 · timestamp"]
    TS --> VL
    K[/"Photos you took<br/>with a known device"/] --> FP["6 · fingerprints learn"]
    FP -. "local database" .-> A
```

## Two ways to use it

| Mode | Program | When to use it |
| --- | --- | --- |
| Desktop app (GUI) | `rv-imgauth-gui.exe` (Start menu: *RV ImgAuth*) | Windows 10/11. You pick the evidence and the options in one window, press **Start review**, then look at the maps in the image viewer. |
| Command line (CLI) | `rv-imgauth.exe` | Scripts, batches, the verification commands (`verify-log`, `verify-case`, `timestamp`) and the fingerprint database (`fingerprints`). |

Both use the same engine. With the same evidence and options they give the same hashes, findings and maps, and both
write the same files.

## Desktop app: what each button does

Nothing is written to disk until you press **Start review**. Each change you make before that is kept in memory,
then written to the run's audit log as a `ui_*` entry when the run starts. After the run, the maps you look at in the
image viewer are recorded in the run's `review.jsonl` (`ui_map_viewed`).

| Button or box | What it does | What it writes |
| --- | --- | --- |
| **Add files…** | Opens the Windows file picker and adds the chosen files to the evidence list. | `ui_input_added` (one per file) |
| **Add folder…** | Adds a whole folder. | `ui_input_added` |
| **Add path** | Adds the path typed in the box next to it (pressing Enter does the same). | `ui_input_added` |
| Drag and drop | Files or folders dropped onto the window are added to the evidence list, as with **Add files…**. | `ui_input_added` |
| **x** (next to an evidence entry) | Removes that entry from the evidence list. | `ui_input_removed` |
| **Include subfolders** | On by default: the folders are read with their subfolders. | `ui_recursive_changed` |
| **Browse…** next to **Case folder** | Chooses the shared case folder. The output folder then becomes `<case>/runs/rv-imgauth/<RUN-ID>` and you can no longer edit it. | `ui_case_dir_changed` |
| **Register the evidence with RV HashInv first** | Shown once a case folder is set. Runs rv-hashinv on the same evidence before the analysis, so the case gets its evidence registry (Task 2). The line next to it shows the rv-hashinv version found, or in red why none can be used. | `ui_intake_changed` |
| **Error level (ELA)**, **Noise**, **JPEG ghosts** (Analysis maps) | The maps written for each image. ELA and Noise are on by default; JPEG ghosts is off. | `ui_maps_changed` |
| **Copy-move detection** | On by default. Looks for regions copied within the image. | `ui_copy_move_changed` |
| **ELA JPEG quality** and **Largest image** (selectors) | The re-compression quality of the ELA map (95) and the largest image decoded (100 Mpx). | `ui_ela_quality_changed`, `ui_max_pixels_changed` |
| **Worker limits** (Timeout, Memory selectors) | Time (120 s) and memory (2048 MB) allowed to the worker process for one image. Beyond them the image gets `PARSER_FAILED` and the review goes on. | `ui_worker_timeout_changed`, `ui_worker_memory_changed` |
| **All items** / **Flagged items only** (Item reports) | Per-file reports for every file, or only for files with a finding above informational. | `ui_item_reports_changed` |
| **Add reference** and **Browse…** next to **Reference images** | Add known originals from the claimed device (typed paths, separated by `;`, or picked files). Each is listed with an **x** that removes it. | `ui_references_changed` |
| **Browse…** next to **Extra fingerprint DB** | Chooses an extra fingerprint database (`.json`, as written by `fingerprints export`), used on top of the bundled and local ones. | `ui_fingerprints_db_changed` |
| **Browse…** next to **Output folder** | Chooses where `logs/` and `reports/` are written (default `rakvor-out`). | Nothing until the run: the folder used is recorded in `environment.json` |
| **Examiner** and **Case ID** (text fields) | The examiner's name (default: your Windows user) and an optional case reference. | Nothing until the run: the examiner is written in every audit entry and report, the case ID on the reports |
| **Trusted timestamp** (tick box and address) | On by default. Asks the RFC 3161 authority at that address to timestamp the audit log. | `ui_timestamp_changed` |
| **Browse…** next to **Signing certificate (.p12/.pfx)** | Chooses your certificate. Each PDF report is then signed with it. Type the **Certificate password** in the field below it. The password is kept in memory only and is never logged. | `ui_signing_certificate_changed` (the path only) |
| **Start review** | Starts the run with the settings shown. | `ui_start_clicked`, then the whole run (see [Where the outputs go](#where-the-outputs-go)) |
| **Cancel** | Stops after the file being read and writes partial reports. | `item_cancelled`, `run_cancelled` |
| **Open report** | Opens the statistics report (`statistics.<LANG>.html`) of the finished run. | Nothing |
| **Open log folder** | Opens the run's log folder. | Nothing |
| **Open hash report** | After a run with **Register the evidence with RV HashInv first**: opens rv-hashinv's statistics report. | Nothing |
| **View image** (one per row of the Results table) | Opens the image viewer on that item: the original preview and a map side by side or blended, the findings, the file structure and the metadata. | `ui_map_viewed` in the run's `review.jsonl`, once per item, map and view mode shown |
| **Back to results** | Leaves the image viewer. | Nothing |
| **-**, **+** and **Fit** (image viewer) | Zoom out, zoom in, and reset the shared zoom of the original and the overlay. The mouse wheel zooms and dragging pans too. | Nothing |
| **Evidence IDs** (result panel) | After a run in a case folder, lists each item's evidence id (E0001…) and its registry check. Read-only. | Nothing |
| **Warnings** (result panel) | Lists the errors met during the run. Read-only. | Nothing here: the run logs each error as `item_error` |

The timestamp is eIDAS-qualified only if the TSA address is that of a qualified trust service provider (QTSP) on an EU
trusted list; the default address (DigiCert) is not one.

The selectors (hash algorithms, report language, interface language, custody form) are recorded too:
`ui_hash_selection_changed`, `ui_language_changed`, `ui_interface_language_changed` and `ui_jurisdiction_changed`.
In the image viewer, the **Item** and **Overlay** selectors, the **Side by side** / **Blended** view and the
**Overlay opacity** slider choose what is shown; before showing a picture, the viewer checks it against the digest in
the audit log (green: matches; red: changed after the run).

## Task 1: Analyse an image or a folder

1. Check the build once on this PC: `rv-imgauth self-test` must print `PASS` for every algorithm (exit code 0).
2. Run the analysis on a verified copy of the evidence (the native files, not screenshots or re-saved copies), never
   on the original media. Leave out `--no-timestamp` when the PC can reach the TSA: the run then also obtains the
   timestamp (`timestamp: ok …`).

   ```powershell
   rv-imgauth analyze -i evidence -r --hash sha256,md5 --lang es --case CASE-2026-014 --actor "Ana Ruiz" --no-timestamp
   ```

3. Read the summary. On a copy of the sample evidence (`examples\sample\evidence`, 15 synthetic images) in
   `evidence`, offline (`--no-timestamp`), the command above prints (the elapsed time varies):

   ```text
   DONE (high-severity findings)  run 20261126T132030Z-ab84ac
     items: 15  bytes: 1330737  errors: 0  elapsed: 2 min 33 s
     findings: high 1, medium 10, low 11, info 45
     timestamp: off
     report: rakvor-out\reports\20261126T132030Z-ab84ac\statistics.es.html
     logs:   rakvor-out\logs\20261126T132030Z-ab84ac
   ```

4. Open the `report:` file. Then, for each item, open `technical.<LANG>.html` (for the examiner) or
   `summary.<LANG>.html` (for the client), and look at the maps with **View image** in the desktop app (or in
   `maps/`, but that viewing is not recorded).

**What it writes:** `rakvor-out/logs/<RUN-ID>/` (audit log, `hashes.csv`, `environment.json`) and
`rakvor-out/reports/<RUN-ID>/` (statistics report, and per item a technical review, a summary, a custody form,
`structure.json`, `metadata.json` and `maps/`).

**How to read the result:** `DONE` with exit code 0 means no high-severity finding. `DONE (high-severity findings)`
with exit code 3 is a normal outcome for evidence, not an error: here the sample `pix-copy-move.jpg` gives
`COPY_MOVE_CANDIDATE` on purpose. Open the items whose status is `problem`. Each finding states what it shows and what
it does not prove; [usage.md](usage.md#5-interpreting-each-finding) says what to check before relying on each code.
`timestamp: pending` means that the authority could not be reached (see Task 5).

Analysis options:

| Option | What it sets |
| --- | --- |
| `--input` (`-i`) | A file or folder to review. Repeat it for several. |
| `--input-list` (`-L`) | A text file with one path per line. |
| `--recursive` (`-r`) | Also read subfolders. |
| `--hash` (`-H`) | One or two of md5, sha1, sha256, sha512, sha3-256, blake3 (default sha256). |
| `--out` (`-o`) | Output folder (default `rakvor-out`). |
| `--lang` (`-l`) | Report language: en, es, fr or all. |
| `--actor` (`-a`) | Examiner name in the audit log and the reports. |
| `--case` (`-c`) | Case identifier printed on the reports. |
| `--progress` (`-p`) | bar, jsonl (one JSON event per line) or none. |
| `--jurisdiction` (`-j`) | Custody form: co (Colombia) or fr (France). |
| `--tsa-url` | RFC 3161 authority (default `http://timestamp.digicert.com`). |
| `--no-timestamp` | No timestamp request (offline labs). |
| `--sign-cert` | Your `.p12`/`.pfx` certificate: each PDF report is signed with it. |
| `--sign-pass-env` | The environment variable that holds its password (default `RV_SIGN_PASSWORD`). |
| `--maps` | The maps per image: ela, noise, ghosts or none (default `ela,noise`). |
| `--copy-move` | on (default) or off: the copy-move search. |
| `--ela-quality` | Re-compression quality of the ELA map, 1 to 100 (default 95). |
| `--max-pixels` | Images with more pixels are not decoded (default 100 000 000). |
| `--reference` | A known original from the claimed device to compare every item with. Repeat it for several. |
| `--worker-timeout` | Seconds allowed for one image (default 120). |
| `--worker-memory` | Memory limit of the worker in MiB (default 2048). |
| `--fingerprints-db` | An extra fingerprint database (Task 6). |
| `--item-reports` | all (default) or flagged: per-file reports only for files with a finding above informational. |
| `--case-dir` | The shared case folder (Task 2). |
| `--evidence` (`-E`) | Registered evidence ids to analyse (Task 2). |
| `--intake` | hashinv: register the evidence with rv-hashinv first (Task 2). |
| `--hashinv-exe` | The rv-hashinv program to use with `--intake hashinv` (Task 2). |

## Task 2: Analyse evidence in a case folder

1. Choose one folder for the case, on the case volume, for example `cases\CASE-2026-014`. rv-imgauth is an analysis
   tool: it verifies each file against the evidence registry that the intake tool, rv-hashinv, writes. It never writes
   the registry itself.
2. Register and analyse in one step: `--intake hashinv` runs rv-hashinv on the same evidence first (it creates the
   case, so give `--case` and `--jurisdiction`). rv-hashinv is searched in `--hashinv-exe`, `RV_HASHINV`, next to
   rv-imgauth, the RV HashInv install folder, then `PATH`. If the case was registered before, leave out
   `--intake hashinv`.

   ```powershell
   rv-imgauth analyze --case-dir cases\CASE-2026-014 --case CASE-2026-014 --jurisdiction co -i evidence -r --intake hashinv --no-timestamp
   ```

3. Read the last line of the summary:

   ```text
     case:   CASE-2026-014 (analysis) verified 15, mismatched 0, not in registry 0
   ```

4. Later, to analyse some registered items again, give their ids:

   ```powershell
   rv-imgauth analyze --case-dir cases\CASE-2026-014 --evidence E0001,E0002 --no-timestamp
   ```

**What it writes:** the run under `runs/rv-imgauth/<RUN-ID>/logs` and `/reports`, and its `run_started` and
`run_finished` entries in `case-log.jsonl` (with `--intake hashinv`, rv-hashinv also writes `case.json`, the registry
`evidence/registry.json` and `.csv` and its own run under `runs/rv-hashinv/`).

**How to read the result:** `verified` counts the files whose hashes match the registry. A file that differs from the
registry gets the high finding `EVIDENCE_HASH_MISMATCH_WITH_CASE` (it is still analysed); a file not in the registry
gets `EVIDENCE_NOT_IN_CASE_REGISTRY` (medium). Exit code 3 before anything is read means that the case folder failed
verification: nothing was written to it (run Task 4). Exit code 2 means a usage error, for example a folder that
belongs to another case or an unknown evidence id.

## Task 3: Verify a run's audit log

1. Point `verify-log` at the run's log folder (or at the `audit.jsonl` in it):

   ```powershell
   rv-imgauth verify-log rakvor-out\logs\20261126T132030Z-ab84ac
   ```

2. Read the output:

   ```text
   OK  159 entries, chain intact: rakvor-out\logs\20261126T132030Z-ab84ac\audit.jsonl
       outputs: 248 verified of 248 recorded (md5, sha256)
   ```

3. If the run has a timestamp, add `--tsa-ca <TSA roots.pem>` to also check the TSA certificate chain against the
   TSA's own root certificates. If maps were viewed in the GUI, the output also gives `review entries: N`.

**What it writes:** nothing. It only reads.

**How to read the result:** `OK` (exit code 0) means that no entry was edited, deleted or re-ordered, that each
timestamp token matches, that the review log (if any) is intact and anchored to this run, and that each report, map,
`structure.json`, `metadata.json`, `hashes.csv` and `environment.json` still has the digest the log recorded. `BROKEN`
(exit code 3) lists one problem per line. Exit code 1 means that the log or the `--tsa-ca` file cannot be read.
Revocation of the TSA certificate is never checked.

## Task 4: Verify a whole case

1. Run `verify-case` on the folder that holds `case.json`:

   ```powershell
   rv-imgauth verify-case cases\CASE-2026-014
   ```

2. Read the output (after Task 2: the rv-hashinv intake run and the two rv-imgauth runs):

   ```text
   OK  case CASE-2026-014: 8 case-log entries, 15 registered items, 3 runs verified: cases\CASE-2026-014
       outputs: 454 verified of 454 recorded by the runs' audit logs
   ```

**What it writes:** nothing. It only reads.

**How to read the result:** `OK` (exit code 0) means that the case log, the registry, every run's audit log (of
every Rakvor tool) and review log, and the runs' recorded outputs are intact. Warnings (for example a run still in
progress) are printed but do not fail the check. `BROKEN` (exit code 3) lists each failed check. Exit code 1 means
that the folder is not a case folder (no `case.json`).

## Task 5: Add the timestamp after offline work

1. Offline, run the analysis as usual, or with `--no-timestamp` (or `RV_NO_TIMESTAMP=1`) so that no request is sent.
   Without `--no-timestamp`, the summary shows `timestamp: pending`.

   ```powershell
   rv-imgauth analyze -i evidence\edited-gimp.jpg -o offline --no-timestamp
   ```

2. Once the network is back, timestamp the run's log folder:

   ```powershell
   rv-imgauth timestamp offline\logs\20261126T132521Z-c392d6
   ```

3. Read the result: `OK  timestamp 2026-11-26T13:25:34Z by http://timestamp.digicert.com covers entry #18`. Use
   `--tsa-url` to choose another authority, and `--actor` to name the examiner in the new audit entry.
4. Run Task 3 on the same folder. It now shows the timestamp line and `TSA signature valid`:

   ```text
   OK  19 entries, chain intact: offline\logs\20261126T132521Z-c392d6\audit.jsonl
       timestamp 2026-11-26T13:25:34Z by http://timestamp.digicert.com covers entry #18: TSA signature valid (signer: CN=DigiCert SHA256 RSA4096 Timestamp Responder 2026 1,O=DigiCert\, Inc.,C=US); chain not checked (pass --tsa-ca <CA bundle> to validate it)
       outputs: 23 verified of 24 recorded (sha256), 1 skipped
       not re-hashed: logs/timestamp-00018.tsr (RFC 3161 token: checked by the timestamp verification)
   ```

**What it writes:** `timestamp-<SEQ>.tsr` (the RFC 3161 token) and `timestamp-<SEQ>.json` in the log folder, and
the `timestamp_obtained` entry in `audit.jsonl`.

**How to read the result:** exit code 0 means that the timestamp was obtained and logged. Exit code 1 means that the
authority could not be reached; nothing changed, so try again later. Exit code 3 means that the log is missing or
broken; no timestamp is added to a broken log. The timestamp proves when the log existed, not that the evidence is
authentic. The review log (`review.jsonl`) is written after the run and is not covered by the run's timestamp.

## Task 6: Teach the tool a camera or an editor

1. Make known originals yourself: take photos with the device, or save files with the app or editor. Learn only from
   files whose origin you can prove. The starter database of this version is empty: until you learn originals, every
   JPEG gives `QT_UNKNOWN`.
2. Learn the device. Here the two sample reference images stand for such photos:

   ```powershell
   rv-imgauth fingerprints learn evidence\reference\ref-1.jpg evidence\reference\ref-2.jpg --label "Phone X camera" --category camera --case LAB-2026-03 --actor "Ana Ruiz"
   ```

3. Read the result (first lines; the next ones give the database and audit-log paths and its SHA-256 before and
   after):

   ```text
   LEARNED local-phone-x-camera (Phone X camera, camera) from 2 file(s)
     producer contains: (none)
     creator contains:  Canon | Canon EOS R6
     traits:            qt_sha256=cfe3218ae132279888e3a2b4c6949ea5cfa8df2c92a42e1dac9597d5b622b545, subsampling=4:2:0
   ```

4. See what this workstation has learned with `rv-imgauth fingerprints list --db local` (`--db starter` for the
   bundled entries, no `--db` for both).
5. Export the local database for another PC or for disclosure (an existing file is never overwritten); `--db starter`
   exports the bundled one:

   ```powershell
   rv-imgauth fingerprints export --out lab-fingerprints-2026-11.json
   ```

   On the other PC, give it to the analysis with `--fingerprints-db lab-fingerprints-2026-11.json`.

**What it writes:** `%LOCALAPPDATA%\Rakvor\rv-imgauth\fingerprints.local.json` (the local database, used by every
later analysis) and its hash-chained change log `fingerprints.audit.jsonl` in the same folder; `export` writes the
file named by `--out`. Nothing is sent anywhere.

**How to read the result:** a match in a later analysis (`QT_MATCHES_DEVICE`, `QT_MATCHES_EDITOR`) is a signal about
the last encoder of the image, not about a person or a specific device; software strings can be forged. Categories:
camera, os, app, editor, messenger, library. Exit code 1 means a file that cannot be read or learned from, an
unreadable database, or an export file that already exists; exit code 2 a usage error (unknown category, missing
`--label`).

## Other commands

| Command | What it does |
| --- | --- |
| `rv-imgauth self-test` | Checks every hash algorithm against published test values. |
| `rv-imgauth fingerprints` | Groups `learn`, `list` and `export` (Task 6). |
| `rv-imgauth update-check` | Tells you whether a newer release is published. It only notifies: nothing is downloaded. `RV_NO_UPDATE_CHECK=1` turns it off. |
| `rv-imgauth describe --json` | Prints the tool's description for RV Hub. It reads and writes nothing. |

## Exit codes

| Code | Meaning | Returned by |
| --- | --- | --- |
| 0 | Completed; no high-severity finding (verification commands: everything checked is intact). | every command |
| 1 | Runtime error: input not found, output not writable, case log locked, wrong certificate password, missing `--fingerprints-db` file, TSA unreachable (`timestamp` only; `analyze` shows `timestamp: pending` and goes on), not a case folder, a failed self-test, a file that cannot be learned from or an export file that exists, or an update check that could not be made. | `analyze`, `verify-log`, `verify-case`, `timestamp`, `self-test`, `fingerprints`, `update-check` |
| 2 | Usage error: unknown option, bad value, more than two algorithms, case folder of another case, unknown evidence id, unknown fingerprint category. | every command (unknown option); `analyze`, `fingerprints learn` |
| 3 | At least one high-severity finding, a broken audit or review log, a failed case check, or (for `update-check`) a newer release. | `analyze`, `verify-log`, `verify-case`, `timestamp`, `update-check` |
| 130 | Cancelled with Ctrl+C: partial reports are written and the cancellation is in the audit log. | `analyze` |

## Where the outputs go

```mermaid
flowchart TB
    OUT["--out (default rakvor-out)"] --> L["logs/&lt;RUN-ID&gt;/<br/>audit.jsonl · audit.log · hashes.csv<br/>environment.json · timestamp-SEQ.tsr/.json<br/>review.jsonl (maps viewed)"]
    OUT --> R["reports/&lt;RUN-ID&gt;/<br/>statistics.json/.LANG.html/.LANG.pdf"]
    R --> I["items/NNNNN-name/<br/>technical · summary · custody<br/>structure.json · metadata.json · maps/"]
    CASE["--case-dir CASE"] --> CJ["case-log.jsonl<br/>(case.json, registry: rv-hashinv)"]
    CASE --> RUN["runs/rv-imgauth/&lt;RUN-ID&gt;/<br/>logs/ + reports/"]
    FP["fingerprints learn"] --> DB["%LOCALAPPDATA%\Rakvor\rv-imgauth\<br/>fingerprints.local.json · fingerprints.audit.jsonl"]
```

Without `--case-dir`, each run creates `logs/<RUN-ID>/` and `reports/<RUN-ID>/` under `--out` (default
`.\rakvor-out`, relative to the current folder). With `--case-dir`, they go to `<CASE>/runs/rv-imgauth/<RUN-ID>/`.
Keep the log folder with the reports: it is the proof of what was done.
