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
    /// Checker container image digest, `sha256:<64 hex>`.
    pub image: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckerTool {
    Verus,
    Kani,
}

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

impl Problem {
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
        check_image_digest(&raw.checker.image)?;

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
                image: raw.checker.image,
            },
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

fn check_image_digest(value: &str) -> Result<(), ParseError> {
    match value.strip_prefix("sha256:") {
        Some(hex) if hex.len() == 64 && is_lower_hex(hex) => Ok(()),
        _ => Err(invalid(
            "checker.image",
            "must be an image digest: `sha256:` and 64 lowercase hex digits",
        )),
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
