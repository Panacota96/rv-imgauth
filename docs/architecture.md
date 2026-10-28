# Architecture

One engine, two front-ends. The engine (`src/core`) does all the work and never prints; the CLI and the GUI only
turn user input into a `RunConfig` and render the `Event`s the engine emits, so both interfaces produce identical
logs and reports. The image layer (`src/img`) parses and analyses bytes and pixels; it does no I/O and writes
nothing.

## Crates

```mermaid
flowchart TB
    subgraph Front-ends
        CLI["rv-imgauth-cli → rv-imgauth<br/>(src/cli) clap · indicatif<br/>analyze · fingerprints · verify-log<br/>timestamp · verify-case · self-test"]
        GUI["rv-imgauth-gui (src/gui)<br/>egui · state.rs (testable rules)<br/>image viewer"]
    end
    subgraph Core["rv-imgauth-core (src/core)"]
        RUN["run.rs<br/>pipeline"]
        WK["worker.rs<br/>isolation"]
        IS["imgscan.rs<br/>image item analysis"]
        FP["fingerprints.rs"]
        AU["audit.rs · review.rs"]
        CA["case.rs · intake.rs"]
        TR["trust.rs<br/>RFC 3161 · CMS"]
        REP["report/<br/>JSON · HTML · PDF<br/>imgview.rs"]
    end
    subgraph Img["rv-imgauth-img (src/img) — forbid(unsafe), no I/O"]
        MOD["model.rs<br/>FROZEN image model"]
        PARSE["parse/<br/>jpeg · png · webp · heif · meta · signals"]
        ANA["analysis/<br/>qt · double · pixels · ela · noise<br/>ghosts · copymove · thumbnail · reference"]
    end
    CLI -- RunConfig --> RUN
    GUI -- "RunConfig + UI actions" --> RUN
    RUN -- events --> CLI & GUI
    RUN --> WK --> IS
    IS --> PARSE & ANA
    PARSE & ANA --> MOD
    IS --> FP
    RUN --> AU & CA & TR & REP
    GUI -- "ui_map_viewed" --> AU
```

| Crate / module | Responsibility |
| --- | --- |
| `src/img` `parse::jpeg` | JPEG marker parser (T.81), segment tree without gaps, MPF/trailer detection; baseline Huffman decoder to quantized DCT coefficients |
| `src/img` `parse::png`, `webp`, `heif`, `inflate` | PNG chunk walker (own zlib inflater for text chunks), WebP RIFF walker, HEIF/ISOBMFF box walker with item graph and extents |
| `src/img` `parse::meta`, `signals` | EXIF (kamadak-exif), XMP (quick-xml) with `xmpMM:History`, IPTC-IIM, PNG text; structure and metadata signals |
| `src/img` `analysis::*` | Pixel decoding (zune-jpeg, png, image-webp), quantization tables, double compression, ELA, noise maps, JPEG ghosts, copy-move, thumbnail and reference comparison |
| `src/core` `imgscan.rs` | One item: second read and re-hash, parse, signals → findings, fingerprint lookup, maps written into the item folder, reference comparison |
| `src/core` `analyzer.rs` | Dispatch: image candidates to `imgscan` (in the worker), other files to inventory findings |
| `src/core` `worker.rs` | Child-process isolation, limits, reply protocol `rv-imgauth-worker/1`, `PARSER_FAILED` |
| `src/core` `fingerprints.rs` | Starter, local and extra fingerprint databases; identity (name, version, SHA-256) for the audit |
| `src/core` `audit.rs`, `review.rs` | Hash-chained `audit.jsonl`; post-run `review.jsonl` anchored to `run_finished`; verification |
| `src/core` `case.rs`, `intake.rs` | Case folder (`rakvor.case/1`), registry verification, `--intake hashinv` |
| `src/core` `trust.rs` | RFC 3161 timestamp of the chain head; CMS signing of PDF reports (parent only; the only network code) |
| `src/core` `report/` | View models → minijinja HTML, built-in PDF writer, schema-versioned JSON; `imgview.rs` turns image details into report sections |
| `src/core` `run.rs`, `input.rs`, `hash.rs`, `env.rs`, `events.rs` | Orchestration, inputs, hashing, `environment.json`, typed events and ETA |
| `src/cli` | Commands, long help (also the source of [usage-cli.md](usage-cli.md)), `fingerprints learn/list/export` |
| `src/gui` | egui window; `state.rs` holds every rule (options, viewer model, audit of views) and is unit-tested; `app.rs` draws |

## Data flow of one image

```mermaid
flowchart LR
    E[("evidence<br/>read-only")] -->|pass 1| H["hash.rs<br/>1–2 digests"]
    E -->|pass 2 in worker| B["bytes<br/>re-hash = pass 1?"]
    B --> P["parse()<br/>ParsedImage"]
    P --> S["structure + metadata<br/>signals"]
    P --> Q["qt::fingerprint<br/>+ fingerprint DBs"]
    B --> C["decode_coefficients<br/>(baseline JPEG)"] --> D["double compression"]
    B --> X["decode_pixels<br/>(JPEG/PNG/WebP ≤ caps)"]
    X --> M["original.png · ELA · noise<br/>ghosts · copy-move · thumbnail"]
    P --> R["reference comparison"]
    S & Q & D & M & R --> F["findings + details<br/>+ outputs list"]
    F --> W["parent: hash outputs,<br/>output_written, item_analyzed"]
    W --> REP["reports"]
```

## The frozen image model

`src/img/src/model.rs` is **frozen**: fields may be added with a default, never renamed or removed. The parsers
and the analyses were written in parallel against it, and the core depends only on it.

```text
bytes ──► parse()                ──► ParsedImage { format, width, height, structure: [Segment tree],
                                                   metadata { tags, xmp, xmp_history, makernote_present,
                                                   icc_sha256, problems }, jpeg | png | webp | heif info,
                                                   thumbnail, signals }
      ──► decode_coefficients()  ──► Vec<ComponentCoefficients>   (baseline JPEG only)
      ──► decode_pixels()        ──► Pixels (RGB8)                (JPEG/PNG/WebP; never HEIF)
analyses(ParsedImage, coefficients, pixels) ──► Vec<Signal { code, severity, params }> + Vec<MapImage { name, png, params }>
```

`Signal`s carry only a code, a severity and parameters; the core maps them to findings and the wording comes from
`i18n/*.json`. `MapImage`s carry their parameters, which go into the report next to the map.

## One run, step by step

```mermaid
sequenceDiagram
    autonumber
    participant UI as CLI / GUI
    participant R as run.rs
    participant A as audit.jsonl
    participant W as worker
    participant O as reports/
    UI->>R: run(config, sink, cancel)
    R->>A: run_started, UI actions, environment_recorded, inputs_resolved
    loop each item (sequential)
        R->>A: item_started, item_hashed{digests, mtime_unchanged}
        R->>A: case verification (with --case-dir)
        R->>A: analysis_started
        R->>W: image candidate → worker
        W-->>R: findings, details, outputs
        R->>A: output_written (each file), item_analyzed
    end
    R->>A: duplicate_group(s), hash_register_written
    R->>O: per-item technical, summary, custody
    R->>A: item_reports_written
    R->>A: timestamp_obtained / timestamp_pending
    R->>O: statistics
    R->>A: statistics_report_written, run_finished
    R-->>UI: RunFinished
    Note over UI,A: later, in the GUI viewer: review.jsonl ← ui_map_viewed (anchored to run_finished)
```

Items are processed sequentially so that the order of audit entries equals the order of work; inside one image the
heavy analyses (noise wavelet strips, residual rows, copy-move DCT planes) use `rayon`. Cancellation is checked
after every hashing chunk and between items; partial reports and `run_cancelled` are written.

## Worker process

```text
run.rs ──► analyzer::analyze_item ── not an image ──► inventory analysis (in process)
                    │ image candidate (signature or extension)
                    ▼
      worker::analyze_in_worker ── spawn `<exe> worker --in … --item-dir … --opts <json> --expect-digest <algo>:<hex>`
        limits first (Unix RLIMIT_AS, no core dump, empty network namespace when permitted;
        Windows Job Object), then "RV-WORKER-GO" on stdin
                    ▼
      child: imgscan::analyze_image ─► rv_imgauth_img ─► structure.json, metadata.json, maps/*.png
                    │ one JSON reply on stdout {"protocol":"rv-imgauth-worker/1","analysis":…}
                    ▼
      parent: validate output paths ─► hash + audit each file ─► item_analyzed
```

Failure (timeout, crash, memory limit, bad reply) → `PARSER_FAILED` (high) and `details.worker`; the run continues.
Files left by a failed worker are kept as `partial_output` and audited. Details: [theory.md B19](theory.md#b19-worker-isolation).

## The review log

The run's audit chain is closed at `run_finished`. What the examiner looks at **afterwards** (maps opened in the
GUI viewer) goes into `review.jsonl` in the same log folder: a second hash chain whose first entry quotes the
`run_finished` hash. `verify-log` and `verify-case` verify it when present. If it cannot be written, the GUI falls
back to recording the view in the next run's audit log.

```mermaid
flowchart LR
    subgraph audit.jsonl
        A1["#1 run_started"] --> A2["…"] --> AN["#N run_finished<br/>hash H"]
        AN -.-> AT["timestamp_obtained<br/>(only entry allowed after)"]
    end
    subgraph review.jsonl
        R1["#1 ui_map_viewed<br/>anchor_seq N, anchor_hash H<br/>prev = H"] --> R2["#2 ui_map_viewed<br/>prev = #1"]
    end
    AN --> R1
```

## Run states (as the GUI sees them)

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Running: Start (inputs ≥ 1, algorithms 1–2)
    Running --> Finished: RunFinished
    Running --> Cancelled: Cancel → partial reports
    Running --> Failed: error (e.g. output not writable)
    Finished --> Viewer: View image
    Viewer --> Finished: Back to results
    Finished --> Running: Start again (new run ID)
    Cancelled --> Running
    Failed --> Running
```

## Why Rust

See [ADR 0001](adr/0001-language.md).

## Further reading

| Page | Covers |
| --- | --- |
| [theory.md](theory.md) | Every analysis, with exact parameters, at two levels |
| [admissibility.md](admissibility.md) | Legal requirements mapped to features; gaps; checklist |
| [reports.md](reports.md) | Report sections, per-item outputs, finding codes |
| [forensic-audit.md](forensic-audit.md) | Audit log, review log, timestamp, signed reports |
| [case-folder.md](case-folder.md) | Shared case folder and intake |
