//! rv-imgauth command-line interface. All work happens in `rv_imgauth_core`; this file only
//! parses arguments, renders events (progress bar or JSON Lines) and maps results to exit codes.

mod fingerprints;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rv_imgauth_core::trust::{self, DEFAULT_TSA};
use rv_imgauth_core::{
    CASE_ROLE, Event, HashAlgo, RunConfig, TOOL_NAME, TOOL_VERSION, audit, case, hash, i18n, run,
};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const EXIT_OK: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_FINDINGS: u8 = 3;
const EXIT_CANCELLED: u8 = 130;

const ABOUT: &str = "Look for signals that images (JPEG, HEIC, PNG, WebP) were modified, with court-ready reports and a tamper-evident audit log.";

const LONG_ABOUT: &str = "\
DESCRIPTION:
  rv-imgauth looks for signals that a photo or image was modified after it was captured. It
  reviews a single file, a whole folder (bulk) or a list of files. For every file it:
    1. opens the file read-only and hashes it in one pass with one or two algorithms;
    2. identifies the real file type from its signature; JPEG, HEIC, PNG and WebP images (by
       signature or extension) are examined in an isolated worker process, which re-hashes the
       bytes it reads and compares them with the first pass;
    3. turns what it saw into findings (signals with a severity, what they show and what they do
       not prove) and writes a technical review and a customer summary (HTML, PDF and JSON).
  IMAGE ANALYSES (each writes findings; files go to the item folder <ITEM>, see FILES WRITTEN):
    - Structure: JPEG marker sequence, duplicate tables, progressive frames, data after EOI; PNG
      chunk order, private chunks, data after IEND; WebP VP8X flags vs chunks; HEIF boxes, item
      graph and bytes no box or item covers. Full tree with offsets in structure.json.
    - Metadata: EXIF, XMP (CreatorTool, xmpMM:History), IPTC and PNG text; editor software,
      inconsistent dates or sizes, stripped metadata, messenger traces, missing MakerNote. Every
      tag in metadata.json.
    - JPEG quantization tables: exact IJG quality q=N, or the nearest q and its distance (never a
      quality percentage); the table hash is looked up in the fingerprint databases.
    - Double JPEG compression: periodicity of the DCT coefficient histograms (baseline JPEG).
    - Visual maps (--maps; no score, visual aids only): ELA (JPEG re-saved at --ela-quality,
      fixed gain), noise (median 3x3 residual and a db8 sigma map per 40x40 block), JPEG ghosts
      (re-saved at q 50-90; images up to 24 megapixels).
    - Copy-move (--copy-move): 16x16 block DCT features on a grey copy at most 1024 px long, sorted
      and voted by shift vector (shift of 32 px or more, 20 blocks or more).
    - Embedded EXIF thumbnail compared with the main image (thumbnail-diff.png).
    - Reference comparison (--reference): tables, markers, EXIF tag set, thumbnail, resolution,
      ICC profile, subsampling, PNG chunks and HEIF items against known originals.
    HEIC/HEIF pixels are not decoded (structure and metadata only: HEIC_PIXELS_NOT_DECODED).
    Images above --max-pixels are not decoded (ANALYSIS_SKIPPED_SIZE_CAP).
  Each run also writes a statistics report and an audit trail under <OUT>/logs/<RUN-ID>/. Every
  action is appended to a hash-chained log that `rv-imgauth verify-log` can check later.
  The result is a set of signals for an examiner, never a verdict that an image is authentic or
  manipulated.

  CASE FOLDER (--case-dir): Rakvor tools share one folder per case. rv-hashinv, the intake tool,
  registers each evidence file once (evidence ids E0001, E0002, ...). rv-imgauth is an analysis tool:
  before analysing an image it re-hashes it and verifies it against that registry, so the reports
  prove the examined file is bit-identical to the registered one. `rv-imgauth verify-case <CASE>`
  checks the whole case.";

const ANALYZE_AFTER_HELP: &str = "\
EXAMPLES:
  Review one photo (SHA-256, English reports):
    rv-imgauth analyze --input evidence/IMG_0042.jpg
  Review a folder and its subfolders with SHA-256 + MD5, Spanish reports, for case CASE-2026-014:
    rv-imgauth analyze -i D:\\evidence -r --hash sha256,md5 --lang es --case CASE-2026-014
  Review the files listed in a text file, reports in the three languages, machine-readable progress:
    rv-imgauth analyze --input-list files.txt --lang all --progress jsonl > events.jsonl
  Every map, including JPEG ghosts:
    rv-imgauth analyze -i evidence -r --maps ela,noise,ghosts
  Large batch: noise map only, no copy-move, reports only for flagged items:
    rv-imgauth analyze -i E:\\phone-export -r --maps noise --copy-move off --item-reports flagged
  Compare with known originals from the same camera:
    rv-imgauth analyze -i IMG_0042.jpg --reference ref/IMG_0040.jpg --reference ref/IMG_0041.jpg
  Verify the audit log afterwards:
    rv-imgauth verify-log rakvor-out/logs/<RUN-ID>/audit.jsonl
  Analyse every item registered in a case folder (after rv-hashinv intake), then verify the case:
    rv-imgauth analyze --case-dir E:\\CASE-014
    rv-imgauth verify-case E:\\CASE-014
  Analyse two registered items of the case only:
    rv-imgauth analyze --case-dir E:\\CASE-014 --evidence E0001,E0002

EXIT CODES:
  0    Completed; no high-severity finding.
  1    Runtime error (input not found, output not writable, case log locked, ...).
       See the audit log.
  2    Usage error (unknown option, more than two hash algorithms, unsupported language).
       With --case-dir: folder of another case or schema, --case missing when creating the
       case, unknown evidence id.
  3    Completed with at least one high-severity finding (e.g. an image that could not be
       analysed, evidence that changed during the run or differs from the case registry), or
       the case folder fails verification (nothing is written to it).
  130  Cancelled with Ctrl+C. Partial reports and the cancellation are written to the audit log.

FILES WRITTEN:
  <OUT>/logs/<RUN-ID>/audit.jsonl          Hash-chained log of every action (one JSON object/line).
  <OUT>/logs/<RUN-ID>/audit.log            The same actions, human-readable.
  <OUT>/logs/<RUN-ID>/hashes.csv           Hash register: index, path, size, digests, status.
  <OUT>/logs/<RUN-ID>/environment.json     Tool and dependency versions, OS, host, user, arguments.
  <OUT>/logs/<RUN-ID>/timestamp-<SEQ>.tsr  RFC 3161 token for the audit chain head (+ .json).
  <OUT>/reports/<RUN-ID>/statistics.json   Run statistics (+ statistics.<LANG>.html and .pdf).
  <ITEM> = <OUT>/reports/<RUN-ID>/items/<NNNNN>-<NAME>:
  <ITEM>/technical.json|.<LANG>.html|.pdf  Technical review (image analysis, fingerprint databases,
                                           maps, every file written for the item).
  <ITEM>/summary.json|.<LANG>.html|.pdf    Customer summary.
  <ITEM>/custody.json|.<es|fr|en>.html|.pdf  Chain-of-custody form.
  <ITEM>/structure.json                    Marker, chunk or box tree with byte offsets.
  <ITEM>/metadata.json                     Every metadata tag read (EXIF, XMP, IPTC, PNG text).
  <ITEM>/maps/original.png                 Lossless preview of the decoded pixels (not for HEIC).
  <ITEM>/maps/ela.png, noise.png, noise-sigma.png, ghost-qNN.png, copymove.png,
              thumbnail-diff.png           Visual maps, each with its parameters in the report.
  Every file under <ITEM> is hashed with the selected algorithms and recorded in the audit log.
  With --case-dir <CASE>, logs/ and reports/ go to <CASE>/runs/rv-imgauth/<RUN-ID>/, and:
  <CASE>/case-log.jsonl                    Hash-chained case log of every tool: run_started and
                                           run_finished of this run are appended to it.
  <CASE>/case.json                         Case id and jurisdiction (only if the folder is new).
  The evidence registry (<CASE>/evidence/registry.json) is read, never written, by rv-imgauth.

FORENSIC NOTES:
  - Evidence is opened read-only and never modified; modification times are compared before and
    after.
  - Work on a verified copy (write-blocked image or export), not on original media.
  - Findings are signals, not a verdict: an image without findings is not proven authentic, and a
    finding does not prove manipulation. Read \"What it does not prove\" and the Limitations
    section.
  - Images are analysed in a separate worker process with a time and memory limit, and images
    larger than --max-pixels are not decoded (decompression bombs).
  - Metadata (EXIF, XMP) and software strings are written by the device or program that saved the
    file and can be forged; re-saving, resizing or sending through a messenger destroys most
    traces.
  - Each audit entry stores the SHA-256 of the previous one; editing, deleting or re-ordering any
    line is detected by `verify-log`. Keep the log folder with the reports.
  - MD5 and SHA-1 are accepted for compatibility with older case files; pair them with SHA-256 or
    BLAKE3.
  - The chain head is timestamped by an RFC 3161 authority (proves when the log existed). Offline?
    The run finishes with `timestamp_pending`; run `rv-imgauth timestamp <LOG>` later.
  - With --sign-cert every PDF report is signed with the examiner's certificate; the password is
    never logged.
  - Case folder: every item is compared with the registry (by --evidence id, else by path, else
    by content) before it is analysed; a different hash is the high finding
    EVIDENCE_HASH_MISMATCH_WITH_CASE (the analysis still runs), a file not in the registry is
    EVIDENCE_NOT_IN_CASE_REGISTRY (medium). If no selected algorithm is in the registry, SHA-256
    (or another registry algorithm) is added and logged as case_algorithm_added.
  - The case log and registry digest are re-verified before every write; a broken case is
    refused (exit 3). Appends happen under case-log.lock; a lock older than 10 minutes is
    reported as stale and never deleted automatically.
  - Each run's audit.jsonl records case_linked (the case-log entry it started from); the case
    log records the run's final audit head, so the two chains vouch for each other.";

const FINGERPRINTS_AFTER_HELP: &str = "\
DESCRIPTION:
  Fingerprints recognise the camera, phone, app or editor that wrote an image from its JPEG
  quantization tables (SHA-256 of the tables), chroma subsampling and software or camera strings.
  Three databases are used by `analyze`:
  the bundled starter DB, the local DB learned on this workstation and, optionally,
  --fingerprints-db FILE. The local DB lives in
  %LOCALAPPDATA%\\Rakvor\\rv-imgauth\\fingerprints.local.json (Windows) or
  $XDG_DATA_HOME/rakvor/rv-imgauth/ (default ~/.local/share/rakvor/rv-imgauth/) on Linux/macOS.
  Nothing is ever sent anywhere: learning only reads the files you name.

EXAMPLES:
  rv-imgauth fingerprints learn originals/*.jpg --label \"Phone X camera\" --category camera
  rv-imgauth fingerprints list --db local
  rv-imgauth fingerprints export --out lab-fingerprints-2026-10.json";

const FP_LEARN_AFTER_HELP: &str = "\
DESCRIPTION:
  Reads each image read-only, takes what identifies the software or device that wrote it and adds
  one entry `local-<label>` to the local database. Several files: only what all files share is
  kept. Learning the same label again merges into the existing entry. Nothing is written if any
  file fails. Learned: the JPEG quantization-table SHA-256 and chroma subsampling, EXIF Software
  or XMP CreatorTool, and EXIF Make/Model. A JPEG without quantization tables, or a file with
  none of these, cannot be learned from.

FILES WRITTEN:
  <LOCAL-DB-FOLDER>/fingerprints.local.json    The local database (version incremented).
  <LOCAL-DB-FOLDER>/fingerprints.audit.jsonl   Hash-chained log of every change: files and their
                                               SHA-256, DB SHA-256 before and after, examiner, case.

FORENSIC NOTES:
  - Learn only from files whose origin you can prove (taken by you with the device, or vendor
    samples).
  - Software strings can be forged; a match is a signal about the software, not about a person.
  - The DB SHA-256 printed here appears in every report that used the database.

EXAMPLES:
  rv-imgauth fingerprints learn IMG_0001.jpg --label \"Phone X camera\" --category camera
  rv-imgauth fingerprints learn a.jpg b.jpg --label \"Messenger Y\" --category messenger --case LAB-01

EXIT CODES:
  0  Learned (or merged); DB SHA-256 before/after printed.
  1  A file is unreadable or cannot be learned from; or the DB cannot be written.
  2  Usage error (unknown category, missing --label).";

#[derive(Parser)]
#[command(name = TOOL_NAME, version = TOOL_VERSION, about = ABOUT, long_about = LONG_ABOUT, max_term_width = 100,
          after_long_help = "Run `rv-imgauth <COMMAND> --help` for the options, examples and exit codes of each command.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Look for signals of modification in images (a file, a folder or a list) and write the reports.
    ///
    /// `scan` is accepted as a hidden alias for one release so existing scripts keep working.
    #[command(long_about = LONG_ABOUT, after_long_help = ANALYZE_AFTER_HELP, alias = "scan")]
    Analyze(Box<AnalyzeArgs>),
    /// Manage the local fingerprint database (learn, list, export). Offline only.
    #[command(subcommand, after_long_help = FINGERPRINTS_AFTER_HELP)]
    Fingerprints(FingerprintsCmd),
    /// Verify that an audit log (audit.jsonl) has not been edited, truncated or re-ordered, and check
    /// every RFC 3161 timestamp token of the run (imprint and the TSA's CMS signature).
    #[command(after_long_help = "\
EXAMPLES:
  rv-imgauth verify-log rakvor-out/logs/20261019T101500Z-a1b2c3/audit.jsonl
  rv-imgauth verify-log rakvor-out/logs/20261019T101500Z-a1b2c3
  rv-imgauth verify-log rakvor-out/logs/20261019T101500Z-a1b2c3 --tsa-ca digicert-tsa-roots.pem

TIMESTAMP CHECKS:
  For each timestamp-<N>.tsr: the token covers the chain head of entry N; the TSA's CMS signature is
  valid (signer certificate in the token, ESS signing-certificate binding, message digest over the
  TSTInfo, RSA signature, timeStamping key usage). Without --tsa-ca the output says \"chain not
  validated\"; with it the TSA certificate must chain to a certificate of the bundle (signatures,
  validity at the token's time, CA flag). Revocation is not checked.

REVIEW LOG:
  review.jsonl in the same folder (viewing after the run, e.g. maps opened in the GUI viewer) is
  checked when present: an intact hash chain whose first entry links to the run's run_finished
  entry, every entry naming this run. The output adds \"review entries: N\".

EXIT CODES:
  0  The chain is intact and every timestamp token verifies.
  1  The log or the --tsa-ca file cannot be read.
  3  The chain is broken (an entry was edited, deleted or re-ordered), a timestamp token does not
     verify (altered token, invalid TSA signature, chain not trusted by --tsa-ca), or review.jsonl
     is broken or anchored to another run.")]
    VerifyLog {
        /// Path to audit.jsonl, or to the run's log folder that contains it.
        ///
        /// Example: rakvor-out/logs/20261019T101500Z-a1b2c3
        log: PathBuf,
        /// CA bundle (PEM, one or more certificates, or one DER certificate) that the TSA certificate
        /// must chain to. Obtain it from the TSA's own repository, not from the token.
        /// Default: none (the TSA signature is checked, the chain is not validated)
        /// Example: --tsa-ca C:\certs\tsa-roots.pem
        #[arg(long, value_name = "FILE", verbatim_doc_comment)]
        tsa_ca: Option<PathBuf>,
    },
    /// Check every hash algorithm against known-answer test vectors.
    #[command(after_long_help = "\
EXAMPLES:
  rv-imgauth self-test

EXIT CODES:
  0  All algorithms give the published answer for the input \"abc\".
  1  At least one algorithm failed: do not use this build for evidence.")]
    SelfTest,
    /// Add an RFC 3161 timestamp to a finished run (e.g. after working offline).
    #[command(after_long_help = "\
EXAMPLES:
  rv-imgauth timestamp rakvor-out/logs/20261019T101500Z-a1b2c3
  rv-imgauth timestamp rakvor-out/logs/20261019T101500Z-a1b2c3 --tsa-url https://freetsa.org/tsr

EXIT CODES:
  0  Timestamp obtained and logged.
  1  The TSA could not be reached (nothing changed; try again).
  3  The log is broken: no timestamp is added to a broken chain.")]
    Timestamp {
        /// The run's log folder (or its audit.jsonl).
        ///
        /// Example: rakvor-out/logs/20261019T101500Z-a1b2c3
        log: PathBuf,
        /// RFC 3161 time-stamping authority.
        /// Default: http://timestamp.digicert.com
        /// Example: --tsa-url https://freetsa.org/tsr
        #[arg(long, value_name = "URL", default_value = DEFAULT_TSA, verbatim_doc_comment, hide_default_value = true)]
        tsa_url: String,
        /// Name written in the new audit entry.
        /// Default: the operating-system user name
        /// Example: --actor "Ana Ruiz"
        #[arg(short, long, value_name = "NAME", verbatim_doc_comment)]
        actor: Option<String>,
    },
    /// Verify a whole case folder: case log, evidence registry and every run's audit log.
    #[command(after_long_help = "\
CHECKS:
  - case-log.jsonl is an intact hash chain starting with case_created; case.json is unchanged;
  - evidence/registry.json has the SHA-256 recorded at the last registry change;
  - every run announced in the case log (by any Rakvor tool) has an intact audit.jsonl (and
    valid timestamp tokens), its case_linked entry points to its run_started entry, and the
    audit head recorded at run_finished is its final entry (only timestamp_obtained may follow);
  - a run's review.jsonl (viewing after the run), when present, is an intact chain anchored to
    that run's run_finished entry (\"review entries: N\").

EXAMPLES:
  rv-imgauth verify-case E:\\CASE-014

EXIT CODES:
  0  The case folder is intact (warnings, e.g. a run still in progress, are printed).
  1  The folder is not a case folder (no case.json).
  3  At least one check failed: the case was edited, truncated or re-ordered.")]
    VerifyCase {
        /// The case folder (the one that holds case.json).
        ///
        /// Example: E:\CASE-014
        case: PathBuf,
    },
    /// Print the full help of every command as Markdown (used to generate docs/usage-cli.md).
    #[command(hide = true)]
    DocsCli,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProgressMode {
    /// Progress bar with elapsed time, throughput and ETA on stderr.
    Bar,
    /// One JSON event per line on stdout (for scripts and other tools).
    Jsonl,
    /// No progress output; only the final summary.
    None,
}

#[derive(Args)]
struct AnalyzeArgs {
    /// File or folder to review. Repeat for several.
    ///
    /// Folders are reviewed one level deep unless --recursive is given. Symbolic links are not
    /// followed.
    /// Default: none (required unless --input-list, --case-dir or --evidence is given).
    /// Example: --input D:\evidence --input E:\phone-export\chat.txt
    #[arg(
        short,
        long = "input",
        value_name = "PATH",
        required_unless_present_any = ["input_list", "case_dir", "evidence"],
        verbatim_doc_comment
    )]
    inputs: Vec<PathBuf>,

    /// Text file with one path per line (bulk review). Lines starting with # are ignored.
    ///
    /// Relative paths are resolved from the list file's folder.
    /// Default: none. Example: --input-list case-014-files.txt
    #[arg(short = 'L', long, value_name = "FILE", verbatim_doc_comment)]
    input_list: Option<PathBuf>,

    /// Also review files in subfolders.
    ///
    /// Default: off (only the first level of each folder). Example: -i D:\evidence --recursive
    #[arg(short, long, verbatim_doc_comment)]
    recursive: bool,

    /// Hash algorithm(s): one or two of md5, sha1, sha256, sha512, sha3-256, blake3.
    ///
    /// Both are computed in the same read of each file. More than two is refused (exit code 2).
    /// Default: sha256. Example: --hash sha256,md5   or   --hash sha256 --hash blake3
    #[arg(
        short = 'H',
        long = "hash",
        value_name = "ALGO",
        value_delimiter = ',',
        default_value = "sha256",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    hashes: Vec<String>,

    /// Folder where logs/ and reports/ are created. Created if missing. Ignored with --case-dir.
    ///
    /// Default: ./rakvor-out. Example: --out E:\case-014\rv-imgauth
    #[arg(
        short,
        long,
        value_name = "DIR",
        default_value = "rakvor-out",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    out: PathBuf,

    /// Report language: en, es, fr, or all (one HTML and PDF per language).
    ///
    /// JSON reports are language-neutral.
    /// Default: en. Example: --lang es   or   --lang all
    #[arg(
        short,
        long,
        value_name = "LANG",
        default_value = "en",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    lang: String,

    /// Name of the examiner, written in every audit entry and report.
    ///
    /// Default: the operating-system user name. Example: --actor "Ana Ruiz"
    #[arg(short, long, value_name = "NAME", verbatim_doc_comment)]
    actor: Option<String>,

    /// Case or engagement identifier printed on the reports. With --case-dir it names the case
    /// when the folder is created, and must match case.json afterwards (it can be left out).
    /// Default: none. Example: --case CASE-2026-014
    #[arg(short, long = "case", value_name = "ID", verbatim_doc_comment)]
    case_id: Option<String>,

    /// How progress is shown: bar, jsonl or none.
    ///
    /// Default: bar (hidden automatically when stderr is not a terminal).
    /// Example: --progress jsonl
    #[arg(
        short,
        long,
        value_enum,
        value_name = "MODE",
        default_value = "bar",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    progress: ProgressMode,

    /// Chain-of-custody form per item: co (Colombia, Fiscalía style, Spanish) or fr (France,
    /// scellés, French). An English copy is always added. With --case-dir the case's
    /// jurisdiction applies.
    /// Default: co
    /// Example: --jurisdiction fr
    #[arg(short, long, value_name = "CC", default_value = "co", value_parser = ["co", "fr"], verbatim_doc_comment, hide_default_value = true, hide_possible_values = true)]
    jurisdiction: String,

    /// RFC 3161 time-stamping authority for the audit chain head.
    /// Default: http://timestamp.digicert.com
    /// Example: --tsa-url https://freetsa.org/tsr
    #[arg(long, value_name = "URL", default_value = DEFAULT_TSA, verbatim_doc_comment, hide_default_value = true)]
    tsa_url: String,

    /// Do not request a trusted timestamp (offline labs). Also set by RV_NO_TIMESTAMP=1.
    /// Default: off (a timestamp is requested)
    /// Example: --no-timestamp
    #[arg(long, env = "RV_NO_TIMESTAMP", value_parser = clap::builder::FalseyValueParser::new(), verbatim_doc_comment, hide_env = true)]
    no_timestamp: bool,

    /// Examiner's certificate (.p12/.pfx, RSA) used to sign every PDF report. The password is
    /// read from the environment variable named by --sign-pass-env, or asked for interactively
    /// on the terminal without echo.
    /// Default: none (PDFs are not signed)
    /// Example: --sign-cert C:\certs\ana-ruiz.p12
    #[arg(long, value_name = "P12", verbatim_doc_comment)]
    sign_cert: Option<PathBuf>,

    /// Environment variable holding the certificate password.
    /// Default: RV_SIGN_PASSWORD
    /// Example: --sign-pass-env CERT_PASS
    #[arg(
        long,
        value_name = "VAR",
        default_value = "RV_SIGN_PASSWORD",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    sign_pass_env: String,

    /// Analysis maps written per image under <ITEM>/maps/ (comma-separated or repeated), or none:
    /// ela (error level, JPEG only: ela.png), noise (noise.png and noise-sigma.png) and ghosts
    /// (JPEG ghosts at q 50, 60, 70, 80, 90: ghost-qNN.png; JPEG up to 24 megapixels). The
    /// preview maps/original.png and the thumbnail difference are written whatever is selected.
    /// Default: ela,noise
    /// Example: --maps ela,noise,ghosts   or   --maps none
    #[arg(
        long,
        value_name = "LIST",
        value_delimiter = ',',
        default_value = "ela,noise",
        value_parser = ["ela", "noise", "ghosts", "none"],
        verbatim_doc_comment,
        hide_default_value = true,
        hide_possible_values = true
    )]
    maps: Vec<String>,

    /// Look for regions copied and pasted within the same image (copy-move): on or off. Writes
    /// maps/copymove.png and the shift vectors when a candidate is found. Scaled or rotated copies
    /// are not detected.
    /// Default: on
    /// Example: --copy-move off
    #[arg(
        long,
        value_enum,
        value_name = "ON|OFF",
        default_value = "on",
        verbatim_doc_comment,
        hide_default_value = true,
        hide_possible_values = true
    )]
    copy_move: OnOff,

    /// JPEG quality used to re-compress each image for error level analysis, 1 to 100
    /// (jpeg-encoder, IJG tables, 4:4:4, fixed gain 15).
    /// Default: 95
    /// Example: --ela-quality 90
    #[arg(long, value_name = "Q", default_value_t = 95, value_parser = clap::value_parser!(u8).range(1..=100), verbatim_doc_comment, hide_default_value = true)]
    ela_quality: u8,

    /// Images with more pixels than this are not decoded (protection against decompression
    /// bombs), 1000000 to 2000000000: no preview, map, copy-move or thumbnail comparison
    /// (ANALYSIS_SKIPPED_SIZE_CAP); structure, metadata and tables are still examined.
    /// Default: 100000000 (100 megapixels)
    /// Example: --max-pixels 250000000
    #[arg(long, value_name = "N", default_value_t = 100_000_000, value_parser = clap::value_parser!(u64).range(1_000_000..=2_000_000_000), verbatim_doc_comment, hide_default_value = true)]
    max_pixels: u64,

    /// Reference image (a known original from the claimed device) to compare every item with.
    /// Repeat for several. Read-only; its SHA-256 is recorded. Each property is compared only when
    /// all references agree on it (REFERENCE_CONSISTENT / REFERENCE_INCONSISTENT).
    /// Default: none
    /// Example: --reference ref/IMG_0040.jpg --reference ref/IMG_0041.jpg
    #[arg(long = "reference", value_name = "FILE", verbatim_doc_comment)]
    references: Vec<PathBuf>,

    /// Seconds a worker process may spend on one image before it is stopped (PARSER_FAILED).
    /// Default: 120
    /// Example: --worker-timeout 600
    #[arg(long, value_name = "SECS", default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=86400), verbatim_doc_comment, hide_default_value = true)]
    worker_timeout: u64,

    /// Memory limit of one worker process in MiB (address space on Linux/macOS, job on Windows).
    /// Default: 2048
    /// Example: --worker-memory 4096
    #[arg(long, value_name = "MB", default_value_t = 2048, value_parser = clap::value_parser!(u64).range(128..=1048576), verbatim_doc_comment, hide_default_value = true)]
    worker_memory: u64,

    /// Extra fingerprint database (JSON, as written by `fingerprints export`),
    /// used with the bundled and local databases. Its name, version and SHA-256 are reported.
    /// Default: none (bundled starter DB + local learned DB)
    /// Example: --fingerprints-db case-014-fingerprints.json
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    fingerprints_db: Option<PathBuf>,

    /// Per-item report files for every item (all) or only for items with findings above info
    /// (flagged). Skipped items are still listed in the statistics report; each skip is audited.
    /// Default: all
    /// Example: --item-reports flagged
    #[arg(
        long,
        value_enum,
        value_name = "WHICH",
        default_value = "all",
        verbatim_doc_comment,
        hide_default_value = true,
        hide_possible_values = true
    )]
    item_reports: ItemReports,

    /// Shared case folder that links the Rakvor tools of one case (see docs/case-folder.md).
    /// Every item is verified against <DIR>/evidence/registry.json (written by the intake tool,
    /// rv-hashinv) before it is analysed; outputs go to <DIR>/runs/rv-imgauth/<RUN-ID>/ and the
    /// run is recorded in <DIR>/case-log.jsonl. With no --input, every registered item is analysed.
    /// Default: none (standalone run in --out)
    /// Example: --case-dir E:\CASE-014
    #[arg(long, value_name = "DIR", verbatim_doc_comment)]
    case_dir: Option<PathBuf>,

    /// Registered evidence ids to analyse (needs --case-dir), comma-separated or repeated.
    /// Default: none (the --input files, or every registered item)
    /// Example: --evidence E0001,E0007
    #[arg(
        short = 'E',
        long = "evidence",
        value_name = "IDS",
        requires = "case_dir",
        verbatim_doc_comment
    )]
    evidence: Vec<String>,

    /// Register the evidence with the intake tool first: hashinv runs RV HashInv
    /// (`rv-hashinv scan --case-dir`) on the same --input/--input-list, so the case gets its hash
    /// inventory, hash reports and evidence registry from rv-hashinv itself; then every item is
    /// analysed and verified against that registry. Needs --case-dir; not with --evidence.
    /// rv-hashinv is searched in: --hashinv-exe, RV_HASHINV, next to rv-imgauth, the RV HashInv
    /// install folder, PATH. Its path, version and SHA-256 go into this run's audit log.
    /// Default: none. Example: --case-dir E:\CASE-014 --case CASE-014 -i D:\evidence -r --intake hashinv
    #[arg(
        long,
        value_enum,
        default_value_t = Intake::None,
        requires = "case_dir",
        hide_default_value = true,
        hide_possible_values = true,
        verbatim_doc_comment
    )]
    intake: Intake,

    /// rv-hashinv executable for --intake hashinv (needs version 0.3.0 or later).
    /// Default: searched (see --intake). Example: --hashinv-exe "C:\Tools\RV HashInv\rv-hashinv.exe"
    #[arg(long, value_name = "EXE", verbatim_doc_comment)]
    hashinv_exe: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Intake {
    /// The case registry already exists (rv-hashinv was run before), or no case folder.
    None,
    /// Run rv-hashinv on the same evidence first.
    Hashinv,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ItemReports {
    All,
    Flagged,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum DbChoice {
    Local,
    Starter,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExportChoice {
    Local,
    Starter,
}

#[derive(Subcommand)]
enum FingerprintsCmd {
    /// Learn a device, app or editor from known-original images and add it to the local database.
    #[command(after_long_help = FP_LEARN_AFTER_HELP)]
    Learn {
        /// Known-original images made with the device or tool to learn (read-only; never modified).
        /// Default: none (at least one is required)
        /// Example: originals/IMG_0001.jpg originals/IMG_0002.jpg
        #[arg(value_name = "IMAGE", required = true, verbatim_doc_comment)]
        files: Vec<PathBuf>,
        /// Name of the tool as it should appear in reports.
        /// Default: none (required)
        /// Example: --label "Phone X camera"
        #[arg(long, value_name = "TEXT", verbatim_doc_comment)]
        label: String,
        /// Category: camera, os, app, editor, messenger or library.
        /// Default: none (required)
        /// Example: --category camera
        #[arg(long, value_name = "CAT", value_parser = rv_imgauth_core::fingerprints::CATEGORIES, verbatim_doc_comment, hide_possible_values = true)]
        category: String,
        /// Case the originals come from, written in the entry's source and the audit log.
        /// Default: none
        /// Example: --case CASE-2026-014
        #[arg(short, long = "case", value_name = "ID", verbatim_doc_comment)]
        case_id: Option<String>,
        /// Examiner written in the audit log.
        /// Default: the operating-system user name
        /// Example: --actor "Ana Ruiz"
        #[arg(short, long, value_name = "NAME", verbatim_doc_comment)]
        actor: Option<String>,
    },
    /// List the entries of the fingerprint databases with their version and SHA-256.
    #[command(after_long_help = "\
EXAMPLES:
  rv-imgauth fingerprints list
  rv-imgauth fingerprints list --db local

EXIT CODES:
  0  Listed.
  1  The local database cannot be read.")]
    List {
        /// Which database: local (learned on this workstation), starter (bundled) or all.
        /// Default: all
        /// Example: --db local
        #[arg(
            long,
            value_enum,
            value_name = "DB",
            default_value = "all",
            verbatim_doc_comment,
            hide_default_value = true,
            hide_possible_values = true
        )]
        db: DbChoice,
    },
    /// Write a database to a JSON file (for --fingerprints-db elsewhere, or for disclosure).
    #[command(after_long_help = "\
EXAMPLES:
  rv-imgauth fingerprints export --out lab-fingerprints-2026-10.json
  rv-imgauth fingerprints export --db starter --out starter.json

EXIT CODES:
  0  Written; its SHA-256 is printed.
  1  The output already exists (never overwritten) or the local database does not exist.")]
    Export {
        /// File to create. An existing file is never overwritten.
        /// Default: none (required)
        /// Example: --out lab-fingerprints-2026-10.json
        #[arg(long, value_name = "FILE", verbatim_doc_comment)]
        out: PathBuf,
        /// Which database: local or starter.
        /// Default: local
        /// Example: --db starter
        #[arg(
            long,
            value_enum,
            value_name = "DB",
            default_value = "local",
            verbatim_doc_comment,
            hide_default_value = true,
            hide_possible_values = true
        )]
        db: ExportChoice,
    },
}

fn main() -> ExitCode {
    // Worker child (image analysis in an isolated process): dispatched before clap sees the arguments.
    if std::env::args_os().nth(1).is_some_and(|a| a == "worker") {
        std::process::exit(rv_imgauth_core::worker::worker_main());
    }
    let cli = Cli::parse();
    ExitCode::from(match cli.command {
        Command::Analyze(a) => analyze(*a),
        Command::Fingerprints(f) => fingerprints_cmd(f),
        Command::VerifyLog { log, tsa_ca } => verify_log(log, tsa_ca),
        Command::VerifyCase { case } => verify_case(case),
        Command::SelfTest => self_test(),
        Command::Timestamp {
            log,
            tsa_url,
            actor,
        } => timestamp(log, tsa_url, actor),
        Command::DocsCli => {
            print!("{}", docs_cli());
            EXIT_OK
        }
    })
}

fn usage_error(msg: &str) -> u8 {
    eprintln!(
        "error: {}\n\nFor more information, try 'rv-imgauth analyze --help'.",
        msg
    );
    EXIT_USAGE
}

fn fingerprints_cmd(f: FingerprintsCmd) -> u8 {
    let r = match f {
        FingerprintsCmd::Learn {
            files,
            label,
            category,
            case_id,
            actor,
        } => fingerprints::learn(&fingerprints::LearnRequest {
            files: &files,
            label: &label,
            category: &category,
            case_id: case_id.as_deref(),
            actor: &actor.unwrap_or_else(rv_imgauth_core::env::os_user),
        }),
        FingerprintsCmd::List { db } => fingerprints::list(match db {
            DbChoice::Local => "local",
            DbChoice::Starter => "starter",
            DbChoice::All => "all",
        }),
        FingerprintsCmd::Export { out, db } => fingerprints::export(
            match db {
                ExportChoice::Local => "local",
                ExportChoice::Starter => "starter",
            },
            &out,
        ),
    };
    match r {
        Ok(text) => {
            let _ = writeln!(std::io::stdout(), "{}", text.trim_end());
            EXIT_OK
        }
        Err(e) => {
            eprintln!("error: {}", e);
            EXIT_ERROR
        }
    }
}

fn analyze(a: AnalyzeArgs) -> u8 {
    let mut algos = Vec::new();
    for h in &a.hashes {
        match HashAlgo::parse(h) {
            Some(x) => algos.push(x),
            None => {
                return usage_error(&format!(
                    "unknown hash algorithm '{}': use md5, sha1, sha256, sha512, sha3-256 or blake3",
                    h
                ));
            }
        }
    }
    if let Err(e) = hash::validate_selection(&algos) {
        return usage_error(&e);
    }
    let langs = match i18n::parse_langs(&a.lang) {
        Ok(l) => l,
        Err(e) => return usage_error(&e),
    };

    if let Some(db) = &a.fingerprints_db
        && !db.is_file()
    {
        eprintln!("error: fingerprint database not found: {}", db.display());
        return EXIT_ERROR;
    }
    if let Some(r) = a.references.iter().find(|r| !r.is_file()) {
        eprintln!("error: reference image not found: {}", r.display());
        return EXIT_ERROR;
    }
    let maps: Vec<String> = if a.maps.iter().any(|m| m == "none") {
        if a.maps.len() > 1 {
            return usage_error("--maps none cannot be combined with other maps");
        }
        vec![]
    } else {
        let mut m: Vec<String> = Vec::new();
        for x in &a.maps {
            if !m.contains(x) {
                m.push(x.clone());
            }
        }
        m
    };
    let evidence_ids = match case::parse_evidence_ids(&a.evidence) {
        Ok(ids) => ids,
        Err(e) => return usage_error(&e.message),
    };
    let intake = a.intake == Intake::Hashinv;
    if intake && !a.evidence.is_empty() {
        return usage_error(
            "--intake hashinv registers new evidence: it cannot be combined with --evidence",
        );
    }
    if intake && a.inputs.is_empty() && a.input_list.is_none() {
        return usage_error(
            "--intake hashinv needs the evidence to register (--input or --input-list)",
        );
    }
    // With --intake the case may not exist yet: rv-hashinv creates it, and the core checks it afterwards.
    if let Some(dir) = a.case_dir.as_ref().filter(|_| !intake) {
        // Case problems found before the run map to their own exit codes (2 usage, 3 broken case).
        if let Err(e) = case::preflight(dir, a.case_id.as_deref(), &evidence_ids) {
            return match e.exit_code() {
                EXIT_USAGE => usage_error(&e.message),
                code => {
                    eprintln!("error: {}", e.message);
                    code
                }
            };
        }
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let _ = ctrlc::set_handler(move || c.store(true, Ordering::SeqCst));

    let cfg = RunConfig {
        inputs: a.inputs,
        input_list: a.input_list,
        recursive: a.recursive,
        algorithms: algos,
        out_dir: a.out,
        langs,
        actor: a.actor.unwrap_or_else(rv_imgauth_core::env::os_user),
        case_id: a.case_id,
        frontend: "cli".into(),
        preamble: vec![],
        command_line: std::env::args().collect(),
        tool_name: TOOL_NAME.into(),
        tool_version: TOOL_VERSION.into(),
        jurisdiction: a.jurisdiction.clone(),
        tsa_url: if a.no_timestamp {
            None
        } else {
            Some(a.tsa_url.clone())
        },
        sign_password: a
            .sign_cert
            .as_ref()
            .and_then(|_| password(&a.sign_pass_env)),
        sign_cert: a.sign_cert.clone(),
        domain: rv_imgauth_core::analyzer::DomainOptions {
            maps,
            copy_move: a.copy_move == OnOff::On,
            ela_quality: a.ela_quality,
            max_pixels: a.max_pixels,
            references: a.references.clone(),
            worker_timeout_s: a.worker_timeout,
            worker_memory_mb: a.worker_memory,
            fingerprints_db: a.fingerprints_db.clone(),
            item_reports_flagged_only: a.item_reports == ItemReports::Flagged,
            isolation: std::env::current_exe()
                .map(|exe| rv_imgauth_core::analyzer::Isolation::Subprocess { exe })
                .unwrap_or_default(),
        },
        case_dir: a.case_dir.clone(),
        intake_hashinv: intake,
        hashinv_exe: a.hashinv_exe.clone(),
        evidence_ids,
        case_role: CASE_ROLE,
    };

    let bar = match a.progress {
        ProgressMode::Bar => {
            let b = ProgressBar::with_draw_target(Some(0), ProgressDrawTarget::stderr_with_hz(8));
            b.set_style(
                ProgressStyle::with_template("{spinner} [{elapsed_precise}] {wide_bar} {binary_bytes}/{binary_total_bytes} {binary_bytes_per_sec} ETA {eta} {msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            Some(b)
        }
        _ => None,
    };
    let jsonl = matches!(a.progress, ProgressMode::Jsonl);
    // A closed stdout (e.g. `| head`) must never abort a forensic run half-way: stop printing, keep working.
    let mut stdout_open = true;
    let mut sink = |e: &Event| {
        if jsonl && stdout_open {
            let mut out = std::io::stdout().lock();
            stdout_open = writeln!(out, "{}", serde_json::to_string(e).unwrap_or_default())
                .and_then(|_| out.flush())
                .is_ok();
        }
        if let Some(b) = &bar {
            match e {
                Event::RunStarted {
                    total_bytes,
                    total_items,
                    ..
                } => {
                    b.set_length(*total_bytes);
                    b.set_message(format!("0/{} files", total_items));
                }
                Event::Progress {
                    done,
                    total,
                    bytes_done,
                    ..
                } => {
                    b.set_position(*bytes_done);
                    b.set_message(format!("{}/{} files", done, total));
                }
                Event::Error { path, message } => b.println(format!(
                    "warning: {}: {}",
                    path.as_deref().unwrap_or("-"),
                    message
                )),
                Event::IntakeStarted { version, path, .. } => {
                    b.println(format!("intake: rv-hashinv {version} ({path}) registers the evidence"))
                }
                Event::IntakeProgress { done, total, .. } => {
                    b.set_message(format!("intake {}/{} files", done, total))
                }
                Event::IntakeFinished {
                    run_id,
                    exit_code,
                    items,
                    report_dir,
                    ..
                } => b.println(format!(
                    "intake: rv-hashinv run {run_id} finished (exit {exit_code}, {items} files); hash reports: {}",
                    report_dir.as_deref().unwrap_or("-")
                )),
                Event::Cancelled { .. } => b.println("cancelled: writing partial reports..."),
                Event::Timestamp { status, reason, .. } if status != "ok" => b.println(format!(
                    "warning: timestamp pending ({}); run `rv-imgauth timestamp` later",
                    reason.as_deref().unwrap_or("-")
                )),
                _ => {}
            }
        }
    };
    let result = run(&cfg, &mut sink, &cancel);
    if let Some(b) = &bar {
        b.finish_and_clear();
    }
    match result {
        Err(e) => {
            eprintln!("error: {}", e);
            EXIT_ERROR
        }
        Ok(s) => {
            if !jsonl {
                let out = format!(
                    "{status}  run {id}\n  items: {items}  bytes: {bytes}  errors: {errors}  elapsed: {elapsed}\n  findings: high {h}, medium {m}, low {l}, info {i}\n  timestamp: {ts}\n  report: {report}\n  logs:   {logs}",
                    status = if s.cancelled {
                        "CANCELLED"
                    } else if s.high > 0 {
                        "DONE (high-severity findings)"
                    } else {
                        "DONE"
                    },
                    id = s.run_id,
                    items = s.items,
                    bytes = s.bytes,
                    errors = s.errors,
                    elapsed = rv_imgauth_core::time::human_duration(s.elapsed_ms),
                    h = s.high,
                    m = s.medium,
                    l = s.low,
                    i = s.info,
                    ts = format!(
                        "{} {}",
                        s.timestamp["status"].as_str().unwrap_or("off"),
                        s.timestamp["gen_time"].as_str().unwrap_or("")
                    )
                    .trim(),
                    report = s
                        .report_dir
                        .join(format!("statistics.{}.html", cfg.langs[0]))
                        .display(),
                    logs = s.log_dir.display()
                );
                let _ = writeln!(std::io::stdout(), "{}", out);
                if let Some(c) = &s.case {
                    let t = &c.tally;
                    let _ = writeln!(
                        std::io::stdout(),
                        "  case:   {} ({}) verified {}, mismatched {}, not in registry {}",
                        c.case_id,
                        c.role,
                        t.verified,
                        t.mismatched,
                        t.unregistered
                    );
                }
            }
            if s.cancelled {
                EXIT_CANCELLED
            } else if s.high > 0 {
                EXIT_FINDINGS
            } else {
                EXIT_OK
            }
        }
    }
}

fn password(env: &str) -> Option<String> {
    if let Ok(p) = std::env::var(env) {
        return Some(p);
    }
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        // Read from the controlling terminal with echo switched off (/dev/tty on Unix, CONIN$ on Windows):
        // the password never appears on screen, in a terminal scroll-back or in a screen recording.
        return rpassword::prompt_password("Certificate password (input hidden): ").ok();
    }
    None
}

fn timestamp(log: PathBuf, tsa_url: String, actor: Option<String>) -> u8 {
    let dir = if log.is_file() {
        log.parent().map(|p| p.to_path_buf()).unwrap_or_default()
    } else {
        log
    };
    let mut audit_log =
        match audit::AuditLog::resume(&dir, &actor.unwrap_or_else(rv_imgauth_core::env::os_user)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: {}", e);
                return EXIT_FINDINGS;
            }
        };
    match trust::timestamp_head(&dir, audit_log.entries(), audit_log.head(), &tsa_url) {
        Ok(ts) => {
            if let Err(e) = audit_log.record("timestamp_obtained", Some(&ts.file), ts.to_json()) {
                eprintln!("error: {}", e);
                return EXIT_ERROR;
            }
            println!(
                "OK  timestamp {} by {} covers entry #{}",
                ts.gen_time, ts.tsa, ts.covers_seq
            );
            EXIT_OK
        }
        Err(e) => {
            eprintln!("error: {}", e);
            EXIT_ERROR
        }
    }
}

fn verify_log(log: PathBuf, tsa_ca: Option<PathBuf>) -> u8 {
    let path = if log.is_dir() {
        log.join("audit.jsonl")
    } else {
        log
    };
    let anchors = match tsa_ca.as_deref().map(trust::load_ca_bundle).transpose() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {}", e);
            return EXIT_ERROR;
        }
    };
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let mut tokens = Vec::new();
    let mut review = None;
    let checked = audit::verify(&path).and_then(|n| {
        let v = trust::verify_timestamps_with(&dir, &audit::heads(&path), anchors.as_deref());
        tokens = v.tokens;
        let mut problems = v.problems;
        // The review log (viewing after the run) is checked when present: chain and anchor to this run.
        match rv_imgauth_core::review::verify(&dir) {
            Ok(r) => review = r,
            Err(p) => problems.extend(p.into_iter().map(|x| format!("review.jsonl: {x}"))),
        }
        if problems.is_empty() {
            Ok(n)
        } else {
            Err(problems)
        }
    });
    match checked {
        Ok(n) => {
            println!("OK  {} entries, chain intact: {}", n, path.display());
            for t in tokens {
                println!("    {}", t);
            }
            if let Some(r) = review {
                println!("    review entries: {r} (review.jsonl, anchored to run_finished)");
            }
            EXIT_OK
        }
        Err(problems) if problems.len() == 1 && problems[0].starts_with("cannot open") => {
            eprintln!("error: {}", problems[0]);
            EXIT_ERROR
        }
        Err(problems) => {
            println!("BROKEN  {}", path.display());
            for p in problems {
                println!("  - {}", p);
            }
            EXIT_FINDINGS
        }
    }
}

fn verify_case(dir: PathBuf) -> u8 {
    if !dir.join(case::CASE_FILE).is_file() {
        eprintln!(
            "error: {} is not a case folder (no {})",
            dir.display(),
            case::CASE_FILE
        );
        return EXIT_ERROR;
    }
    let v = case::verify_case(&dir);
    if v.problems.is_empty() {
        println!(
            "OK  case {}: {} case-log entries, {} registered items, {} runs verified: {}",
            v.case_id,
            v.entries,
            v.registry_items,
            v.runs,
            dir.display()
        );
        if v.review_entries > 0 {
            println!("    review entries: {}", v.review_entries);
        }
        for w in &v.warnings {
            println!("  warning: {}", w);
        }
        EXIT_OK
    } else {
        println!("BROKEN  case {}: {}", v.case_id, dir.display());
        for p in &v.problems {
            println!("  - {}", p);
        }
        for w in &v.warnings {
            println!("  warning: {}", w);
        }
        EXIT_FINDINGS
    }
}

fn self_test() -> u8 {
    let failed = hash::self_test();
    for (algo, _) in hash::KNOWN_ANSWERS {
        println!(
            "{:<9} {}",
            algo.name(),
            if failed.contains(&algo) {
                "FAIL"
            } else {
                "PASS"
            }
        );
    }
    if failed.is_empty() {
        EXIT_OK
    } else {
        EXIT_ERROR
    }
}

/// Markdown of every command's long help, so docs/usage-cli.md can never drift from the binary.
fn docs_cli() -> String {
    let mut root = Cli::command().term_width(100);
    root.build();
    let mut out = String::from(
        "# Command-line reference\n\n<!-- Generated by `rv-imgauth docs-cli`. Do not edit by hand: CI fails if this file differs. -->\n\n",
    );
    out.push_str("```mermaid\nflowchart LR\n  A[rv-imgauth] --> B[analyze]\n  A --> C[verify-log]\n  A --> D[self-test]\n  A --> T[timestamp]\n  A --> V[verify-case]\n  A --> G[fingerprints]\n  G --> H[(local fingerprint DB)]\n  H --> B\n  T --> E\n  B --> E[(logs/RUN-ID)]\n  B --> F[(reports/RUN-ID)]\n  B -. --case-dir .-> K[(CASE folder)]\n  V --> K\n  C --> E\n```\n\n");
    out.push_str(&format!(
        "## rv-imgauth\n\n```text\n{}\n```\n\n",
        root.render_long_help().to_string().trim_end()
    ));
    let names: Vec<String> = root
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        .map(|c| c.get_name().to_string())
        .collect();
    for name in names {
        let sub = root.find_subcommand_mut(&name).expect("subcommand");
        let mut sub = sub.clone().bin_name(format!("rv-imgauth {}", name));
        out.push_str(&format!(
            "## rv-imgauth {}\n\n```text\n{}\n```\n\n",
            name,
            sub.render_long_help().to_string().trim_end()
        ));
        let nested: Vec<String> = sub
            .get_subcommands()
            .filter(|c| !c.is_hide_set() && c.get_name() != "help")
            .map(|c| c.get_name().to_string())
            .collect();
        for child in nested {
            let mut c = sub
                .find_subcommand_mut(&child)
                .expect("subcommand")
                .clone()
                .bin_name(format!("rv-imgauth {} {}", name, child));
            out.push_str(&format!(
                "### rv-imgauth {} {}\n\n```text\n{}\n```\n\n",
                name,
                child,
                c.render_long_help().to_string().trim_end()
            ));
        }
    }
    out
}
