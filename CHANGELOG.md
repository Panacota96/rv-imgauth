# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

## [Unreleased]

First release line of the image analyses (0.x). No version has been tagged yet: `version.txt` and
`.release-please-manifest.json` say 0.1.0, which release-please treats as already released, so its next release PR
would propose 0.2.0 unless the manifest is adjusted (to decide before the first tag).

### Added

- **Image analysis** of JPEG, HEIC/HEIF, PNG and WebP (by signature or extension), each image in an isolated worker
  process (time and memory limits, no network code; empty network namespace on Linux) that re-hashes the bytes it
  analyses (`EVIDENCE_CHANGED_DURING_ANALYSIS`).
- **Structure**: in-house JPEG marker parser (segment tree without gaps, MPF and trailers), PNG chunk walker with
  the PNG 3rd edition ordering rules, WebP RIFF walker (RFC 9649), HEIF/ISOBMFF box walker with item graph, grid,
  extents and unreferenced bytes; `structure.json` per item.
- **Metadata**: EXIF, XMP with `xmpMM:History`, IPTC-IIM, PNG text and `tIME`, JPEG comments; editor software,
  inconsistent dates and sizes, stripped metadata, Facebook `FBMD` trace, missing MakerNote; `metadata.json` per item.
- **Compression fingerprints**: de-zigzagged quantization tables, exact IJG quality or nearest q with its distance,
  table SHA-256 looked up in the starter, local and extra fingerprint databases; `fingerprints learn | list | export`.
- **Double JPEG compression** from the periodicity of DCT-coefficient histograms (baseline JPEG, in-house Huffman
  decoder).
- **Visual maps** with every parameter recorded and no score: ELA (`--ela-quality`, fixed gain), median residual and
  Mahdian–Saic σ noise maps, JPEG ghosts (`--maps ghosts`, q 50–90, ≤ 24 MP); lossless preview `maps/original.png`.
- **Copy-move** detection with 16×16 block DCT features and shift-vector voting (`--copy-move`).
- **Embedded thumbnail** comparison with `thumbnail-diff.png`.
- **Reference comparison** with known originals (`--reference`, repeatable): "consistent / inconsistent with the
  reference set (n=…)" and a per-property table.
- Finding catalogue in English, Spanish and French (title, what it shows, what it does not prove) and six image
  limitations in every report, including `NO_VALIDATED_ERROR_RATES_YET`.
- Report sections for images (preview, metadata and XMP history, structure, quantization tables, fingerprint
  databases, every map with its parameters, copy-move shifts, reference table); `--item-reports`, `--max-pixels`,
  `--worker-timeout`, `--worker-memory`, `--fingerprints-db`.
- **GUI image viewer**: original and overlay side by side or blended (opacity), synced zoom and pan, map selector,
  pixel readout, digest check of each picture, findings, structure tree and filterable metadata panels.
- **Review log** `review.jsonl`: maps viewed in the GUI after a run (`ui_map_viewed`), hash-chained and anchored to
  the run's `run_finished`; checked by `verify-log` and `verify-case`.
- Case folder (`--case-dir`, `--evidence`, `verify-case`) and one-step intake with RV HashInv (`--intake hashinv`).
- 15 synthetic sample images with exact expected findings; CI oracles (Pillow quantization tables, ExifTool
  metadata, numpy/PyWavelets maps); cargo-fuzz targets for the parsers.
- Documentation rewritten for images: theory (plain language and expert), admissibility (draft pending legal
  review), usage, CLI, GUI, reports, architecture, installation, case folder, forensic audit.

### Known limitations

- Error rates not yet measured on a validation corpus.
- The bundled starter fingerprint database is empty (version 0).
- HEIC/HEIF pixels are not decoded (structure and metadata only).
- Copy-move finds translated copies only; JPEG ghosts use the (0,0) grid offset only.
