# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

## [0.2.0](https://github.com/Panacota96/rv-imgauth/compare/v0.1.0...v0.2.0) (2026-10-28)


### Features

* **cli:** fingerprints learn records software, make/model and the quantization-table digest ([09e95fa](https://github.com/Panacota96/rv-imgauth/commit/09e95fa3447e02e5acb23bc4dff30093c624c55d))
* **cli:** real image analysis end to end: report sections, original preview, review log, samples and oracles ([47dccc1](https://github.com/Panacota96/rv-imgauth/commit/47dccc1c92654ffb3c056d2c00b8d832af199f69))
* **core:** image analysis glue against the frozen image-layer API ([26163fc](https://github.com/Panacota96/rv-imgauth/commit/26163fc0beb14bf54605e82c5b8fb1d617f66cbf))
* **gui:** image viewer (side by side / blended, scaled maps, structure and metadata) and full analysis options ([64fdd0f](https://github.com/Panacota96/rv-imgauth/commit/64fdd0f8600ebf664952e3084d8693cd83138ac6))
* **gui:** map views go to the reviewed run's review log; pin oracle Python packages ([87289fd](https://github.com/Panacota96/rv-imgauth/commit/87289fd5c6909b69b2800c374827dd44fc2c0539))
* **img:** FILE_STRUCTURE_DAMAGED; one expected file per sample shared by the parser and analysis tests ([902feb2](https://github.com/Panacota96/rv-imgauth/commit/902feb253d6531ec6a4b5eaabed1e7de6557761d))
* **img:** freeze the image model, the image-layer API and the finding catalogue ([3c3acca](https://github.com/Panacota96/rv-imgauth/commit/3c3accaf5b192dc2312536c5390fb6cc7ebe812a))
* **img:** JPEG/PNG/WebP/HEIF parsers, metadata readers, baseline coefficient decoder and signals ([6a96345](https://github.com/Panacota96/rv-imgauth/commit/6a9634572d713d9fa0c84a93ef0144ee5d13386b))
* **img:** pixel decoding and the image analyses ([abccc1d](https://github.com/Panacota96/rv-imgauth/commit/abccc1d63bd990fe2b2d6d448b5e54c81a5f1c21))


### Bug Fixes

* PIXELS_NOT_DECODED finding; --max-pixels bounded by the 120 MP hard cap; MPF-only trailers are info; cargo-deny ([74a1410](https://github.com/Panacota96/rv-imgauth/commit/74a14109c218346004cb165f27fceabba87d11b0))

### Highlights

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
- Run findings when an analysis could not run: `FILE_STRUCTURE_DAMAGED`, `PIXELS_NOT_DECODED`,
  `HEIC_PIXELS_NOT_DECODED`, `ANALYSIS_SKIPPED_SIZE_CAP` (`--max-pixels` up to the 120 MP hard cap).
- Dependency licences checked in CI (cargo-deny; no GPL, AGPL or LGPL).

### Known limitations

- Error rates not yet measured on a validation corpus.
- The bundled starter fingerprint database is empty (version 0).
- HEIC/HEIF pixels are not decoded (structure and metadata only).
- Copy-move finds translated copies only; JPEG ghosts use the (0,0) grid offset only.
