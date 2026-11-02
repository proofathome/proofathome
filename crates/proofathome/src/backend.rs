//! Where problems, claims and results live. [`crate::local::LocalBackend`]
//! keeps them on this machine; a coordinator service can implement the same
//! trait later.

use std::fmt;

use proofathome_core::{Problem, Size};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// At most this many problems can be claimed at once.
pub const MAX_ACTIVE_CLAIMS: usize = 3;

/// How long a claim lasts before the problem is open again.
pub fn claim_ttl_secs(size: Size) -> u64 {
    match size {
        Size::XS | Size::S | Size::M => 2 * 3600,
        Size::L | Size::XL => 24 * 3600,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Open,
    /// Claimed by this contributor until `expires_at` (unix seconds).
    Claimed {
        expires_at: u64,
    },
    Submitted {
        at: u64,
    },
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Open => "open",
            Status::Claimed { .. } => "claimed",
            Status::Submitted { .. } => "submitted",
        }
    }
}

/// A status filter for listings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Open,
    Claimed,
    Submitted,
}

impl StatusKind {
    pub fn parse(value: &str) -> Result<StatusKind> {
        match value {
            "open" => Ok(StatusKind::Open),
            "claimed" => Ok(StatusKind::Claimed),
            "submitted" => Ok(StatusKind::Submitted),
            _ => Err(Error::new(format!(
                "unknown status `{value}`; use open, claimed or submitted"
            ))),
        }
    }

    pub fn matches(self, status: Status) -> bool {
        matches!(
            (self, status),
            (StatusKind::Open, Status::Open)
                | (StatusKind::Claimed, Status::Claimed { .. })
                | (StatusKind::Submitted, Status::Submitted { .. })
        )
    }
}

impl fmt::Display for StatusKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StatusKind::Open => "open",
            StatusKind::Claimed => "claimed",
            StatusKind::Submitted => "submitted",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProblemEntry {
    pub problem: Problem,
    pub status: Status,
}

#[derive(Debug, Clone, Default)]
pub struct Listing {
    /// Sorted by problem ID.
    pub problems: Vec<ProblemEntry>,
    /// Catalog files that were skipped, with the reason.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub id: String,
    pub claimed_at: u64,
    pub expires_at: u64,
}

/// One agent session on one problem, as recorded in the attempts log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub problem_id: String,
    pub started_at: u64,
    pub ended_at: u64,
    pub agent: String,
    /// Weighted tokens, see `crate::agent::Usage::weighted`.
    pub tokens: u64,
    pub outcome: String,
}

/// A result ready to hand in. The local check has passed.
#[derive(Debug, Clone)]
pub struct Submission {
    pub problem_id: String,
    /// Binary diff against the pinned commit.
    pub patch: Vec<u8>,
    /// `summary.json`.
    pub summary: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// Where the result went: a directory in local mode.
    pub location: String,
    pub submitted_at: u64,
}

pub trait Backend {
    /// Every problem with its status.
    fn list(&self) -> Result<Listing>;

    fn get(&self, id: &str) -> Result<ProblemEntry> {
        self.list()?
            .problems
            .into_iter()
            .find(|entry| entry.problem.id == id)
            .ok_or_else(|| Error::new(format!("there is no problem `{id}` in the catalog")))
    }

    /// Claims an open problem.
    fn claim(&self, id: &str) -> Result<Claim>;

    /// Extends an active claim by a full claim period.
    fn renew(&self, id: &str) -> Result<Claim>;

    /// Gives up a claim, making the problem open again.
    fn release(&self, id: &str) -> Result<()>;

    /// Hands in a result, marks the problem submitted and ends the claim.
    fn submit(&self, submission: &Submission) -> Result<Receipt>;

    fn record_attempt(&self, attempt: &Attempt) -> Result<()>;

    /// Recorded attempts, oldest first.
    fn attempts(&self) -> Result<Vec<Attempt>>;
}
