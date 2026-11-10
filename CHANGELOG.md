# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org/) by release-please and reviewed by a human.

## [0.3.1](https://github.com/Panacota96/rv-imgauth/compare/v0.3.0...v0.3.1) (2026-11-10)


### Bug Fixes

* **security:** patch dependency advisories, blind RSA signing, check advisories in CI ([039aac8](https://github.com/Panacota96/rv-imgauth/commit/039aac87eaf1b49519dfa67f03c3208107739492))
* **security:** patch dependency advisories, blind RSA signing, check advisories in CI ([019a1f1](https://github.com/Panacota96/rv-imgauth/commit/019a1f107a26fc89519d008e536349210611975d))

## [0.3.0](https://github.com/Panacota96/rv-imgauth/compare/v0.2.0...v0.3.0) (2026-11-10)


### Features

* **cli:** describe --json prints the tool's self-description for RV Hub ([32c84f8](https://github.com/Panacota96/rv-imgauth/commit/32c84f87e0f1d22bf10689bbb5c7a1237927a307))
* **gui:** --case-dir DIR presets the case folder when RV Hub launches the app ([dd9f4ab](https://github.com/Panacota96/rv-imgauth/commit/dd9f4abeb863d9b737f620202758f0a1726891a0))


### Bug Fixes

* **packaging:** RV ImgAuth installer gets its own AppId ([658d355](https://github.com/Panacota96/rv-imgauth/commit/658d355f587f851f28dc354b5a3f61686f562cc9))
* **trust:** drive timestamp checks from the audit log; RFC 3161/5280 TSA path rules ([e54044b](https://github.com/Panacota96/rv-imgauth/commit/e54044bc0add7748daafad633525c01852a0aa00))
* **trust:** drive timestamp checks from the audit log; RFC 3161/5280 TSA path rules ([8f94795](https://github.com/Panacota96/rv-imgauth/commit/8f947957a865de6c1213cb3886c9c119e89e4d7a))

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

## [Unreleased]

### Fixed
- Timestamp verification (`verify-log`, `verify-case`) is driven by the audit log's `timestamp_obtained`
  entries: deleting a token's `timestamp-<SEQ>.json` no longer skips its signature and chain checks (with `--tsa-ca`
  an untrusted TSA passed that way). A recorded token whose `.tsr` or `.json` is missing, or whose metadata disagrees
  with the entry, is a problem (exit 3); a token file that nothing records is checked too, or is a problem when it
  cannot be.
- The TSA certificate's extended key usage must be critical and contain only `id-kp-timeStamping` (RFC 3161 §2.3),
  as in rv-certcheck.
- `--tsa-ca` path validation enforces `pathLenConstraint` (anchor included, self-issued certificates not counted),
  `keyCertSign` for issuers with keyUsage, directoryName name constraints, and rejects unsupported critical
  extensions on any certificate of the path (policy processing and other name forms are not implemented: such
  chains fail closed). Path building tries every candidate issuer and anchor (cross-signed intermediates, renewed
  anchors, self-issued key-rollover intermediates), at most 8 certificates deep, without loops. Tested against 34
  fixture chains cross-checked with `openssl verify -purpose timestampsign` (`tests/fixtures/tsa-chains`).
- Without `--tsa-ca` the token line says "chain not checked" (as the other Rakvor tools), instead of "chain not
  validated".
- Docs: the `openssl ts -verify` cross-check is labelled a signature/chain check; revocation of the TSA certificate
  is never checked by the tool and must be established from CRL/OCSP evidence covering genTime.

Release-please writes each release entry above from the Conventional Commits; changes not yet released are
listed in the open release PR.
