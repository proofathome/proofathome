//! Local mode: problems from a catalog directory, claims and results in the
//! home directory.
//!
//! State files, all under `<home>/state/`:
//! - `state.json`: active claims and submitted problems.
//! - `attempts.jsonl`: one JSON object per agent session.
//! - `lock`: held while reading and updating the files above.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::backend::{
    Attempt, Backend, Claim, Listing, MAX_ACTIVE_CLAIMS, ProblemEntry, Receipt, Status, Submission,
    claim_ttl_secs,
};
use crate::catalog;
use crate::error::{Error, Result};
use crate::fsutil::{StateLock, atomic_write, create_dir_all};
use crate::paths::Home;
use crate::time::{now, rfc3339};

#[derive(Debug, Clone)]
pub struct LocalBackend {
    home: Home,
    catalog: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    #[serde(default)]
    claims: Vec<Claim>,
    #[serde(default)]
    submitted: Vec<Submitted>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Submitted {
    id: String,
    submitted_at: u64,
    location: String,
}

impl State {
    fn active_claim(&self, id: &str, now: u64) -> Option<&Claim> {
        self.claims
            .iter()
            .find(|claim| claim.id == id && claim.expires_at > now)
    }

    fn submitted(&self, id: &str) -> Option<&Submitted> {
        self.submitted.iter().find(|s| s.id == id)
    }

    fn status(&self, id: &str, now: u64) -> Status {
        if let Some(submitted) = self.submitted(id) {
            Status::Submitted {
                at: submitted.submitted_at,
            }
        } else if let Some(claim) = self.active_claim(id, now) {
            Status::Claimed {
                expires_at: claim.expires_at,
            }
        } else {
            Status::Open
        }
    }
}

impl LocalBackend {
    pub fn new(home: Home, catalog: PathBuf) -> LocalBackend {
        LocalBackend { home, catalog }
    }

    pub fn catalog_dir(&self) -> &Path {
        &self.catalog
    }

    fn state_file(&self) -> PathBuf {
        self.home.state_dir().join("state.json")
    }

    fn attempts_file(&self) -> PathBuf {
        self.home.state_dir().join("attempts.jsonl")
    }

    fn lock(&self) -> Result<StateLock> {
        StateLock::acquire(&self.home.state_dir())
    }

    /// Reads the state. Call with the lock held.
    fn load_state(&self) -> Result<State> {
        let path = self.state_file();
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                Error::new(format!(
                    "the state file {} is damaged: {e}; fix or remove it",
                    path.display()
                ))
            }),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(Error::io("read", &path, e)),
        }
    }

    fn save_state(&self, state: &State) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|e| Error::new(format!("cannot encode the state: {e}")))?;
        atomic_write(&self.state_file(), &bytes)
    }
}

impl Backend for LocalBackend {
    fn list(&self) -> Result<Listing> {
        let catalog = catalog::load(&self.catalog)?;
        let state = {
            let _lock = self.lock()?;
            self.load_state()?
        };
        let now = now();
        Ok(Listing {
            problems: catalog
                .problems
                .into_iter()
                .map(|problem| {
                    let status = state.status(&problem.id, now);
                    ProblemEntry { problem, status }
                })
                .collect(),
            warnings: catalog.warnings,
        })
    }

    fn claim(&self, id: &str) -> Result<Claim> {
        let entry = self.get(id)?;
        let _lock = self.lock()?;
        let mut state = self.load_state()?;
        let now = now();
        if state.submitted(id).is_some() {
            return Err(Error::new(format!("`{id}` was already submitted")));
        }
        if let Some(claim) = state.active_claim(id, now) {
            return Err(Error::new(format!(
                "`{id}` is already claimed until {}",
                rfc3339(claim.expires_at)
            )));
        }
        state.claims.retain(|claim| claim.expires_at > now);
        if state.claims.len() >= MAX_ACTIVE_CLAIMS {
            let held: Vec<&str> = state.claims.iter().map(|c| c.id.as_str()).collect();
            return Err(Error::new(format!(
                "you already hold {MAX_ACTIVE_CLAIMS} claims ({}); \
                 release one with `proofathome release <id>` first",
                held.join(", ")
            )));
        }
        let claim = Claim {
            id: id.to_owned(),
            claimed_at: now,
            expires_at: now + claim_ttl_secs(entry.problem.size),
        };
        state.claims.push(claim.clone());
        self.save_state(&state)?;
        Ok(claim)
    }

    fn renew(&self, id: &str) -> Result<Claim> {
        let entry = self.get(id)?;
        let _lock = self.lock()?;
        let mut state = self.load_state()?;
        let now = now();
        let claim = state
            .claims
            .iter_mut()
            .find(|claim| claim.id == id && claim.expires_at > now)
            .ok_or_else(|| Error::new(format!("you have no active claim on `{id}`")))?;
        claim.expires_at = now + claim_ttl_secs(entry.problem.size);
        let claim = claim.clone();
        self.save_state(&state)?;
        Ok(claim)
    }

    fn release(&self, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        let mut state = self.load_state()?;
        let now = now();
        let had_active = state.active_claim(id, now).is_some();
        let before = state.claims.len();
        state.claims.retain(|claim| claim.id != id);
        if state.claims.len() != before {
            self.save_state(&state)?;
        }
        if had_active {
            Ok(())
        } else {
            Err(Error::new(format!("you have no active claim on `{id}`")))
        }
    }

    fn submit(&self, submission: &Submission) -> Result<Receipt> {
        let id = submission.problem_id.as_str();
        self.get(id)?;
        let _lock = self.lock()?;
        let mut state = self.load_state()?;
        if state.submitted(id).is_some() {
            return Err(Error::new(format!("`{id}` was already submitted")));
        }
        let now = now();
        let dir = self.home.submissions_dir().join(id).join(now.to_string());
        create_dir_all(&dir)?;
        atomic_write(&dir.join("result.patch"), &submission.patch)?;
        let summary = serde_json::to_vec_pretty(&submission.summary)
            .map_err(|e| Error::new(format!("cannot encode the summary: {e}")))?;
        atomic_write(&dir.join("summary.json"), &summary)?;
        let location = dir.display().to_string();
        state.claims.retain(|claim| claim.id != id);
        state.submitted.push(Submitted {
            id: id.to_owned(),
            submitted_at: now,
            location: location.clone(),
        });
        self.save_state(&state)?;
        Ok(Receipt {
            location,
            submitted_at: now,
        })
    }

    fn record_attempt(&self, attempt: &Attempt) -> Result<()> {
        let mut line = serde_json::to_string(attempt)
            .map_err(|e| Error::new(format!("cannot encode the attempt: {e}")))?;
        line.push('\n');
        let _lock = self.lock()?;
        let path = self.attempts_file();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| Error::io("open", &path, e))?;
        file.write_all(line.as_bytes())
            .map_err(|e| Error::io("write", &path, e))
    }

    fn attempts(&self) -> Result<Vec<Attempt>> {
        let path = self.attempts_file();
        let text = {
            let _lock = self.lock()?;
            match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(Error::io("read", &path, e)),
            }
        };
        // A line that does not parse, for example one cut short by a crash,
        // is skipped rather than hiding every other attempt.
        Ok(text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect())
    }
}
