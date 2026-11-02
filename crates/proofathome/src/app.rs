//! The operations behind both the CLI and the MCP server.

use proofathome_core::{Problem, Size};
use serde_json::{Value, json};

use crate::backend::{Backend, Claim, Listing, ProblemEntry, Receipt, Status, StatusKind};
use crate::check::{self, LocalCheck};
use crate::error::{Error, Result};
use crate::local::LocalBackend;
use crate::patch;
use crate::paths::{self, Home};
use crate::workspace::{self, Workspace};

/// Longest summary text a submission may carry.
pub const MAX_SUMMARY_CHARS: usize = 2000;

pub struct App {
    home: Home,
    backend: Box<dyn Backend>,
}

/// Listing filters. Every set filter must match.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub size: Option<Size>,
    pub max_size: Option<Size>,
    pub project: Option<String>,
    pub status: Option<StatusKind>,
}

impl Filter {
    pub fn matches(&self, entry: &ProblemEntry) -> bool {
        self.size.is_none_or(|size| entry.problem.size == size)
            && self.max_size.is_none_or(|max| entry.problem.size <= max)
            && self
                .project
                .as_ref()
                .is_none_or(|project| entry.problem.project == *project)
            && self.status.is_none_or(|kind| kind.matches(entry.status))
    }
}

#[derive(Debug, Clone)]
pub struct WorkResult {
    pub problem: Problem,
    pub claim: Claim,
    pub workspace: Workspace,
}

#[derive(Debug, Clone)]
pub struct SubmitResult {
    pub check: LocalCheck,
    /// `None` when the check failed and nothing was submitted.
    pub receipt: Option<Receipt>,
}

impl App {
    /// Local mode, with the home directory and catalog from the environment
    /// and the config file.
    pub fn from_env() -> Result<App> {
        let home = Home::from_env()?;
        let catalog = paths::catalog_dir(&home)?;
        let backend = LocalBackend::new(home.clone(), catalog);
        Ok(App::new(home, Box::new(backend)))
    }

    pub fn new(home: Home, backend: Box<dyn Backend>) -> App {
        App { home, backend }
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    pub fn backend(&self) -> &dyn Backend {
        self.backend.as_ref()
    }

    pub fn list(&self, filter: &Filter) -> Result<Listing> {
        let mut listing = self.backend.list()?;
        listing.problems.retain(|entry| filter.matches(entry));
        Ok(listing)
    }

    pub fn get(&self, id: &str) -> Result<ProblemEntry> {
        self.backend.get(id)
    }

    pub fn claim(&self, id: &str) -> Result<Claim> {
        self.backend.claim(id)
    }

    pub fn release(&self, id: &str) -> Result<()> {
        self.backend.release(id)
    }

    /// Renews our claim on `id`, or claims it if it is open.
    pub fn ensure_claim(&self, id: &str) -> Result<Claim> {
        match self.backend.get(id)?.status {
            Status::Submitted { .. } => Err(Error::new(format!("`{id}` was already submitted"))),
            Status::Claimed { .. } => self.backend.renew(id),
            Status::Open => self.backend.claim(id),
        }
    }

    /// The easiest open problem that `fits`, skipping `skip`: lowest
    /// difficulty first, then by ID.
    pub fn pick_open(
        &self,
        fits: impl Fn(&Problem) -> bool,
        skip: &[String],
    ) -> Result<Option<Problem>> {
        let listing = self.backend.list()?;
        Ok(listing
            .problems
            .into_iter()
            .filter(|entry| entry.status == Status::Open)
            .map(|entry| entry.problem)
            .filter(|problem| fits(problem) && !skip.contains(&problem.id))
            .min_by(|a, b| (a.difficulty, &a.id).cmp(&(b.difficulty, &b.id))))
    }

    /// Claims (or renews) `id` and prepares its workspace.
    pub fn work(&self, id: &str) -> Result<WorkResult> {
        let claim = self.ensure_claim(id)?;
        let problem = self.backend.get(id)?.problem;
        let workspace = workspace::prepare(&self.home, &problem)?;
        Ok(WorkResult {
            problem,
            claim,
            workspace,
        })
    }

    /// The local pre-check on the workspace of `id`.
    pub fn check(&self, id: &str) -> Result<(Problem, LocalCheck)> {
        let entry = self.backend.get(id)?;
        if matches!(entry.status, Status::Claimed { .. }) {
            self.backend.renew(id)?;
        }
        let path = workspace::open(&self.home, &entry.problem)?;
        let result = check::run(&self.home, &entry.problem, &path)?;
        Ok((entry.problem, result))
    }

    /// Runs the local check and, if it passes, submits the result.
    pub fn submit(
        &self,
        id: &str,
        summary: Option<&str>,
        usage: Option<Value>,
    ) -> Result<SubmitResult> {
        if let Some(text) = summary {
            check_summary(text)?;
        }
        self.ensure_claim(id)?;
        let problem = self.backend.get(id)?.problem;
        let path = workspace::open(&self.home, &problem)?;
        let check = check::run(&self.home, &problem, &path)?;
        let Some(checker) = check.checker.as_ref().filter(|_| check.passed) else {
            return Ok(SubmitResult {
                check,
                receipt: None,
            });
        };
        let patch = patch::make(&path, &problem.commit, &self.home.tmp_dir())?;
        let summary = json!({
            "problem_id": problem.id,
            "commit": problem.commit,
            "checker": {
                "tool": problem.checker.tool.to_string(),
                "version": problem.checker.version,
                "image": problem.checker.image.to_string(),
            },
            "check_duration_ms": checker.duration_ms,
            "output_tail": checker.output_tail,
            "summary": summary,
            "usage": usage,
            "client": format!("proofathome {}", env!("CARGO_PKG_VERSION")),
        });
        let receipt = self.backend.submit(&crate::backend::Submission {
            problem_id: problem.id.clone(),
            patch,
            summary,
        })?;
        Ok(SubmitResult {
            check,
            receipt: Some(receipt),
        })
    }
}

fn check_summary(text: &str) -> Result<()> {
    if text.chars().count() > MAX_SUMMARY_CHARS {
        return Err(Error::new(format!(
            "the summary is longer than {MAX_SUMMARY_CHARS} characters"
        )));
    }
    if text
        .chars()
        .any(|c| (c.is_control() && c != '\n') || matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
    {
        return Err(Error::new(
            "the summary contains control or bidirectional formatting characters",
        ));
    }
    Ok(())
}

/// `XS`, `S`, `M`, `L` or `XL`, in any case.
pub fn parse_size(text: &str) -> Result<Size> {
    Size::ALL
        .into_iter()
        .find(|size| size.to_string().eq_ignore_ascii_case(text.trim()))
        .ok_or_else(|| Error::new(format!("unknown size `{text}`; use XS, S, M, L or XL")))
}
