//! The problem file: one proof obligation, pinned and sized.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Deserialize;

/// Problem files larger than this are refused before parsing.
pub const MAX_PROBLEM_FILE_BYTES: usize = 64 * 1024;

const MAX_ID_LEN: usize = 64;
const MAX_URL_LEN: usize = 200;
const MAX_PATH_LEN: usize = 256;
const MAX_VERSION_LEN: usize = 64;
const MAX_EDITABLE: usize = 32;
const MAX_SPEC_FILES: usize = 32;
const MAX_HINTS_LEN: usize = 2000;
const MAX_TIME_BUDGET_S: u32 = 3600;
const MAX_IMAGE_NAME_LEN: usize = 200;

/// A validated problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Unique problem ID, for example `ev-vsock-credit-0007`.
    pub id: String,
    /// The project the problem belongs to.
    pub project: String,
    /// HTTPS URL of the repository holding the code and the spec.
    pub repo: String,
    /// The pinned commit, as a full lowercase hex object ID.
    pub commit: String,
    /// The pinned checker.
    pub checker: Checker,
    /// How the checker is pointed at the code.
    pub check: Check,
    /// What must be proved: `<file path>::<item path>`.
    pub target: Target,
    /// Spec files and their SHA-256 hashes. A result must leave them unchanged.
    pub spec_files: BTreeMap<String, String>,
    /// The only files a result may change or add.
    pub editable: Vec<String>,
    /// What kind of change a result may make to `editable` files.
    pub mode: EditMode,
    /// The checker must accept the result within this many seconds.
    pub time_budget_s: u32,
    /// Budget class.
    pub size: Size,
    /// 1 (easy) to 5 (hard). Scales points.
    pub difficulty: u8,
    /// Free-form hints for the agent. Untrusted text: show it, never run it.
    pub hints: Option<String>,
}

/// The pinned checker a result must pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checker {
    pub tool: CheckerTool,
    /// Tool version, for example `0.2026.10.26.d17fbd0`.
    pub version: String,
    /// Checker container image, pinned by digest.
    pub image: ImageRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckerTool {
    Verus,
    Kani,
}

impl fmt::Display for CheckerTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CheckerTool::Verus => "verus",
            CheckerTool::Kani => "kani",
        })
    }
}

/// A container image pinned by digest: `<name>@sha256:<64 hex>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    /// Image name, for example `ghcr.io/proofathome/verus`.
    pub name: String,
    /// Digest, `sha256:<64 hex>`.
    pub digest: String,
}

impl fmt::Display for ImageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.digest)
    }
}

/// How the checker is pointed at the code. Which keys apply depends on
/// `checker.tool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Verus checks one standalone file with no dependencies besides the
    /// `vstd` that ships with Verus.
    VerusFile {
        /// Repository-relative path of the crate root, for example `src/lib.rs`.
        crate_root: String,
    },
    /// Verus checks a Cargo crate with `cargo verus focus`: the crate itself
    /// is verified, and its locked dependencies such as `vstd` are trusted.
    VerusCargo {
        /// Repository-relative directory holding `Cargo.toml`. `None` is the
        /// repository root.
        manifest_dir: Option<String>,
    },
    /// Kani runs one harness in one Cargo package.
    Kani {
        /// Rust item path of the harness, for example `proofs::credit_wraps`.
        harness: String,
        /// Repository-relative directory holding `Cargo.toml`. `None` is the
        /// repository root.
        manifest_dir: Option<String>,
    },
}

/// The command the checker container runs. It is fixed by the tool and never
/// taken from the problem file. The repository is always mounted read-only;
/// build output goes to `/tmp/target` inside the container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckCommand {
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Working directory inside the container. The repository is at `/work`.
    pub workdir: String,
    /// Whether this is a Cargo build whose dependencies must be fetched
    /// first, with [`CheckCommand::fetch_argv`] in the same directory. The
    /// check itself runs offline.
    pub cargo: bool,
}

impl CheckCommand {
    /// The command that downloads a Cargo check's dependencies, exactly as
    /// pinned by its `Cargo.lock`. It runs no code from the crates. `None`
    /// when nothing needs fetching.
    pub fn fetch_argv(&self) -> Option<Vec<String>> {
        self.cargo
            .then(|| ["cargo", "fetch", "--locked"].map(str::to_owned).to_vec())
    }
}

/// Where the repository is mounted inside the checker container.
pub const CONTAINER_WORKDIR: &str = "/work";

/// The proof target: a file and an item inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Repository-relative file path.
    pub file: String,
    /// Rust item path inside the file, for example `Queue::pop`.
    pub item: String,
}

/// What a result may change in `editable` files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EditMode {
    /// Only ghost code (proof blocks, invariants, ghost variables, lemmas).
    /// Erasing it must give the same exec code as before. For Verus.
    GhostOnly,
    /// Whole files of proof code, such as Kani harnesses behind `cfg(kani)`.
    /// The target's own file may not be editable.
    ProofFiles,
}

/// Budget class: how much agent usage one attempt is expected to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Size {
    XS,
    S,
    M,
    L,
    XL,
}

impl Size {
    /// All sizes, smallest first.
    pub const ALL: [Size; 5] = [Size::XS, Size::S, Size::M, Size::L, Size::XL];

    /// Upper bound of expected tokens per attempt. `None` for `XL`, which
    /// has no bound and is offered only to contributors who opt in.
    ///
    /// Placeholder values until the pilot measures real attempts.
    pub fn token_ceiling(self) -> Option<u64> {
        match self {
            Size::XS => Some(100_000),
            Size::S => Some(400_000),
            Size::M => Some(1_500_000),
            Size::L => Some(5_000_000),
            Size::XL => None,
        }
    }

    /// Whether a problem of this size fits in `remaining_tokens`.
    /// `XL` never fits a token budget.
    pub fn fits(self, remaining_tokens: u64) -> bool {
        self.token_ceiling()
            .is_some_and(|ceiling| ceiling <= remaining_tokens)
    }

    /// The largest size that fits in `remaining_tokens`, if any.
    pub fn largest_fitting(remaining_tokens: u64) -> Option<Size> {
        Size::ALL
            .into_iter()
            .rev()
            .find(|size| size.fits(remaining_tokens))
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Size::XS => "XS",
            Size::S => "S",
            Size::M => "M",
            Size::L => "L",
            Size::XL => "XL",
        })
    }
}

/// Why a problem file was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The file is larger than [`MAX_PROBLEM_FILE_BYTES`].
    TooLarge { bytes: usize },
    /// Not valid TOML, a missing or unknown field, or a wrong type.
    Toml(String),
    /// A field has a value the format does not allow.
    Invalid { field: String, reason: String },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::TooLarge { bytes } => write!(
                f,
                "problem file is {bytes} bytes, the limit is {MAX_PROBLEM_FILE_BYTES}"
            ),
            ParseError::Toml(message) => write!(f, "problem file is not valid: {message}"),
            ParseError::Invalid { field, reason } => write!(f, "`{field}` {reason}"),
        }
    }
}

impl std::error::Error for ParseError {}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> ParseError {
    ParseError::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProblem {
    id: String,
    project: String,
    repo: String,
    commit: String,
    checker: RawChecker,
    check: RawCheck,
    target: String,
    spec_files: BTreeMap<String, String>,
    editable: Vec<String>,
    mode: EditMode,
    time_budget_s: u32,
    size: Size,
    difficulty: u8,
    hints: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChecker {
    tool: CheckerTool,
    version: String,
    image: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCheck {
    crate_root: Option<String>,
    cargo: Option<bool>,
    harness: Option<String>,
    manifest_dir: Option<String>,
}

fn manifest_workdir(manifest_dir: Option<&String>) -> String {
    match manifest_dir {
        Some(dir) => format!("{CONTAINER_WORKDIR}/{dir}"),
        None => CONTAINER_WORKDIR.to_owned(),
    }
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|&arg| arg.to_owned()).collect()
}

impl Problem {
    /// The command the checker container runs for this problem.
    pub fn check_command(&self) -> CheckCommand {
        match &self.check {
            Check::VerusFile { crate_root } => CheckCommand {
                argv: argv(&["verus", "--crate-type=lib", crate_root]),
                workdir: CONTAINER_WORKDIR.to_owned(),
                cargo: false,
            },
            Check::VerusCargo { manifest_dir } => CheckCommand {
                argv: argv(&["cargo", "verus", "focus", "--locked", "--offline"]),
                workdir: manifest_workdir(manifest_dir.as_ref()),
                cargo: true,
            },
            // `cargo kani` has no `--locked` or `--offline`. The check runs
            // with `CARGO_NET_OFFLINE` and a read-only repository, so Cargo
            // can neither download nor rewrite `Cargo.lock`. `--exact` stops
            // `--harness` from also matching longer names.
            Check::Kani {
                harness,
                manifest_dir,
            } => CheckCommand {
                argv: argv(&["cargo", "kani", "--harness", harness, "--exact"]),
                workdir: manifest_workdir(manifest_dir.as_ref()),
                cargo: true,
            },
        }
    }

    /// Parses and validates a problem file.
    pub fn from_toml_str(text: &str) -> Result<Problem, ParseError> {
        if text.len() > MAX_PROBLEM_FILE_BYTES {
            return Err(ParseError::TooLarge { bytes: text.len() });
        }
        let raw: RawProblem =
            toml::from_str(text).map_err(|error| ParseError::Toml(error.message().to_owned()))?;
        Problem::validate(raw)
    }

    fn validate(raw: RawProblem) -> Result<Problem, ParseError> {
        check_slug("id", &raw.id)?;
        check_slug("project", &raw.project)?;
        check_repo_url(&raw.repo)?;
        check_object_id("commit", &raw.commit)?;

        check_version(&raw.checker.version)?;
        let image = parse_image_ref(&raw.checker.image)?;
        let check = parse_check(raw.checker.tool, raw.check)?;

        let target = parse_target(&raw.target)?;

        if raw.spec_files.is_empty() {
            return Err(invalid("spec_files", "must name at least one spec file"));
        }
        if raw.spec_files.len() > MAX_SPEC_FILES {
            return Err(invalid(
                "spec_files",
                format!("has more than {MAX_SPEC_FILES} entries"),
            ));
        }
        for (path, hash) in &raw.spec_files {
            check_rel_path("spec_files", path)?;
            check_sha256(&format!("spec_files.\"{path}\""), hash)?;
        }

        if raw.editable.is_empty() {
            return Err(invalid("editable", "must name at least one file"));
        }
        if raw.editable.len() > MAX_EDITABLE {
            return Err(invalid(
                "editable",
                format!("has more than {MAX_EDITABLE} entries"),
            ));
        }
        let mut seen = BTreeSet::new();
        for path in &raw.editable {
            check_rel_path("editable", path)?;
            if !seen.insert(path.as_str()) {
                return Err(invalid("editable", format!("lists `{path}` twice")));
            }
            if raw.spec_files.contains_key(path) {
                return Err(invalid(
                    "editable",
                    format!("lists spec file `{path}`; spec files can never be edited"),
                ));
            }
        }
        if raw.mode == EditMode::ProofFiles && seen.contains(target.file.as_str()) {
            return Err(invalid(
                "editable",
                format!(
                    "lists the target's file `{}`, which `proof-files` mode does not allow",
                    target.file
                ),
            ));
        }

        if raw.time_budget_s == 0 || raw.time_budget_s > MAX_TIME_BUDGET_S {
            return Err(invalid(
                "time_budget_s",
                format!("must be between 1 and {MAX_TIME_BUDGET_S}"),
            ));
        }
        if !(1..=5).contains(&raw.difficulty) {
            return Err(invalid("difficulty", "must be between 1 and 5"));
        }
        if let Some(hints) = &raw.hints {
            check_hints(hints)?;
        }

        Ok(Problem {
            id: raw.id,
            project: raw.project,
            repo: raw.repo,
            commit: raw.commit,
            checker: Checker {
                tool: raw.checker.tool,
                version: raw.checker.version,
                image,
            },
            check,
            target,
            spec_files: raw.spec_files,
            editable: raw.editable,
            mode: raw.mode,
            time_budget_s: raw.time_budget_s,
            size: raw.size,
            difficulty: raw.difficulty,
            hints: raw.hints,
        })
    }
}

/// Lowercase ASCII letters, digits and single hyphens, not at either end.
fn check_slug(field: &str, value: &str) -> Result<(), ParseError> {
    let ok = (2..=MAX_ID_LEN).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--");
    if ok {
        Ok(())
    } else {
        Err(invalid(
            field,
            format!(
                "must be 2 to {MAX_ID_LEN} lowercase letters, digits and single hyphens, \
                 not starting or ending with a hyphen"
            ),
        ))
    }
}

fn check_repo_url(value: &str) -> Result<(), ParseError> {
    let reason = "must be an https URL without credentials, query or fragment";
    let Some(rest) = value.strip_prefix("https://") else {
        return Err(invalid("repo", reason));
    };
    let host = rest.split('/').next().unwrap_or("");
    let ok = value.len() <= MAX_URL_LEN
        && !host.is_empty()
        && value.bytes().all(|b| b.is_ascii_graphic())
        && !value.contains(['@', '?', '#', '\\']);
    if ok {
        Ok(())
    } else {
        Err(invalid("repo", reason))
    }
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A full Git object ID: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
fn check_object_id(field: &str, value: &str) -> Result<(), ParseError> {
    if matches!(value.len(), 40 | 64) && is_lower_hex(value) {
        Ok(())
    } else {
        Err(invalid(
            field,
            "must be a full commit ID: 40 or 64 lowercase hex digits",
        ))
    }
}

fn check_sha256(field: &str, value: &str) -> Result<(), ParseError> {
    if value.len() == 64 && is_lower_hex(value) {
        Ok(())
    } else {
        Err(invalid(field, "must be a SHA-256: 64 lowercase hex digits"))
    }
}

fn check_version(value: &str) -> Result<(), ParseError> {
    let ok = (1..=MAX_VERSION_LEN).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'_'));
    if ok {
        Ok(())
    } else {
        Err(invalid(
            "checker.version",
            format!("must be 1 to {MAX_VERSION_LEN} letters, digits or `.-+_`"),
        ))
    }
}

/// `<name>@sha256:<64 hex>`. The name is lowercase: an optional registry host
/// with an optional port, then path segments, joined by `/`. Tags are refused:
/// only the digest pins the image.
fn parse_image_ref(value: &str) -> Result<ImageRef, ParseError> {
    let reason = format!(
        "must be a pinned image reference `<name>@sha256:<64 lowercase hex>`, \
         with a lowercase name of at most {MAX_IMAGE_NAME_LEN} characters and no tag, \
         for example `ghcr.io/proofathome/verus@sha256:...`"
    );
    let Some((name, digest)) = value.split_once('@') else {
        return Err(invalid("checker.image", reason));
    };
    let digest_ok = digest
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && is_lower_hex(hex));
    if !digest_ok || !is_image_name(name) {
        return Err(invalid("checker.image", reason));
    }
    Ok(ImageRef {
        name: name.to_owned(),
        digest: digest.to_owned(),
    })
}

fn is_image_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_IMAGE_NAME_LEN {
        return false;
    }
    let segments: Vec<&str> = name.split('/').collect();
    segments.iter().enumerate().all(|(index, segment)| {
        if index == 0 && segments.len() > 1 && segment.contains(':') {
            is_registry_host(segment)
        } else {
            is_image_path_segment(segment)
        }
    })
}

/// `host:port`. A host without a port is checked as a path segment, which
/// allows the same characters.
fn is_registry_host(segment: &str) -> bool {
    let Some((host, port)) = segment.split_once(':') else {
        return false;
    };
    is_image_path_segment(host)
        && (1..=5).contains(&port.len())
        && port.bytes().all(|b| b.is_ascii_digit())
}

/// `[a-z0-9._-]+`, starting and ending with a letter or digit.
fn is_image_path_segment(segment: &str) -> bool {
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let bytes = segment.as_bytes();
    match (bytes.first(), bytes.last()) {
        (Some(&first), Some(&last)) => {
            alnum(first)
                && alnum(last)
                && bytes
                    .iter()
                    .all(|&b| alnum(b) || matches!(b, b'.' | b'_' | b'-'))
        }
        _ => false,
    }
}

fn parse_check(tool: CheckerTool, raw: RawCheck) -> Result<Check, ParseError> {
    match tool {
        CheckerTool::Verus => {
            if raw.harness.is_some() {
                return Err(invalid("check", "`harness` is for kani, not verus"));
            }
            match (raw.crate_root, raw.cargo) {
                (Some(crate_root), None) => {
                    if raw.manifest_dir.is_some() {
                        return Err(invalid(
                            "check",
                            "has `crate_root` and `manifest_dir`; use `crate_root` for one \
                             standalone file, or `cargo = true` with an optional \
                             `manifest_dir` for a Cargo crate",
                        ));
                    }
                    check_rel_path("check", &crate_root)?;
                    if !crate_root.ends_with(".rs") {
                        return Err(invalid("check", "`crate_root` must be a `.rs` file"));
                    }
                    Ok(Check::VerusFile { crate_root })
                }
                (None, Some(true)) => {
                    if let Some(dir) = &raw.manifest_dir {
                        check_rel_path("check", dir)?;
                    }
                    Ok(Check::VerusCargo {
                        manifest_dir: raw.manifest_dir,
                    })
                }
                (Some(_), Some(_)) => Err(invalid(
                    "check",
                    "has `crate_root` and `cargo`; use only one",
                )),
                (None, Some(false)) => Err(invalid(
                    "check",
                    "has `cargo = false`; leave `cargo` out and set `crate_root` instead",
                )),
                (None, None) => Err(invalid(
                    "check",
                    "must have `crate_root` (one standalone file) or `cargo = true` \
                     (a Cargo crate) for verus",
                )),
            }
        }
        CheckerTool::Kani => {
            if raw.crate_root.is_some() || raw.cargo.is_some() {
                return Err(invalid(
                    "check",
                    "may only have `harness` and `manifest_dir` for kani; \
                     `crate_root` and `cargo` are for verus",
                ));
            }
            let Some(harness) = raw.harness else {
                return Err(invalid("check", "must have `harness` for kani"));
            };
            if !harness.split("::").all(is_ident) {
                return Err(invalid(
                    "check",
                    "`harness` must be one or more Rust identifiers joined by `::`",
                ));
            }
            if let Some(dir) = &raw.manifest_dir {
                check_rel_path("check", dir)?;
            }
            Ok(Check::Kani {
                harness,
                manifest_dir: raw.manifest_dir,
            })
        }
    }
}

/// A repository-relative file path: `/`-separated, no `.` or `..` segments,
/// no empty segments, no backslashes or drive letters, printable ASCII.
fn check_rel_path(field: &str, value: &str) -> Result<(), ParseError> {
    let ok = (1..=MAX_PATH_LEN).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
        && !value.contains(['\\', ':'])
        && value
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    if ok {
        Ok(())
    } else {
        Err(invalid(
            field,
            format!(
                "has `{value}`, which is not a repository-relative path \
                 (`/`-separated, no `.` or `..`, no leading `/`, at most {MAX_PATH_LEN} bytes)"
            ),
        ))
    }
}

fn is_ident(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && value != "_"
}

fn parse_target(value: &str) -> Result<Target, ParseError> {
    let reason = "must be `<file path>::<item path>`, for example `src/queue.rs::Queue::pop`";
    let Some((file, item)) = value.split_once("::") else {
        return Err(invalid("target", reason));
    };
    check_rel_path("target", file)?;
    if !file.ends_with(".rs") {
        return Err(invalid("target", "must point into a `.rs` file"));
    }
    if !item.split("::").all(is_ident) {
        return Err(invalid("target", reason));
    }
    Ok(Target {
        file: file.to_owned(),
        item: item.to_owned(),
    })
}

fn check_hints(value: &str) -> Result<(), ParseError> {
    if value.chars().count() > MAX_HINTS_LEN {
        return Err(invalid(
            "hints",
            format!("is longer than {MAX_HINTS_LEN} characters"),
        ));
    }
    if value
        .chars()
        .any(|c| (c.is_control() && c != '\n') || is_bidi_control(c))
    {
        return Err(invalid(
            "hints",
            "contains control or bidirectional formatting characters",
        ));
    }
    Ok(())
}

/// Unicode bidirectional formatting characters, which can make text read
/// differently from how it is stored.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}
