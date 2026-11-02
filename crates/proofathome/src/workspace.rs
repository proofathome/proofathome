//! Workspaces: `<home>/work/<problem id>/`, a checkout of the problem's
//! repository at the pinned commit.
//!
//! Next to each workspace, `<id>.git-config.sha256` records the hash of its
//! `.git/config` when it was prepared. Git settings can run programs, and an
//! agent can write files, so a changed `.git/config` stops every later `git`
//! command in that workspace.

use std::fs;
use std::path::{Path, PathBuf};

use proofathome_core::Problem;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::fsutil::{atomic_write, create_dir_all, random_hex};
use crate::git;
use crate::paths::Home;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub path: PathBuf,
    /// Whether an existing workspace was kept, with any edits in it.
    pub resumed: bool,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn config_hash_file(home: &Home, id: &str) -> PathBuf {
    home.work_dir().join(format!("{id}.git-config.sha256"))
}

fn git_config_hash(workspace: &Path) -> Result<String> {
    let path = workspace.join(".git").join("config");
    let bytes = fs::read(&path).map_err(|e| Error::io("read", &path, e))?;
    Ok(sha256_hex(&bytes))
}

/// Prepares the workspace for `problem`, or keeps the existing one.
pub fn prepare(home: &Home, problem: &Problem) -> Result<Workspace> {
    let path = home.workspace(&problem.id);
    if path.exists() {
        open(home, problem)?;
        return Ok(Workspace {
            path,
            resumed: true,
        });
    }
    let work_dir = home.work_dir();
    create_dir_all(&work_dir)?;
    let temp = work_dir.join(format!("{}.tmp-{}", problem.id, random_hex()));
    match clone_into(problem, &temp) {
        Ok(()) => {}
        Err(error) => {
            let _ = fs::remove_dir_all(&temp);
            return Err(error);
        }
    }
    let hash = git_config_hash(&temp)?;
    atomic_write(&config_hash_file(home, &problem.id), hash.as_bytes())?;
    fs::rename(&temp, &path).map_err(|e| {
        let _ = fs::remove_dir_all(&temp);
        Error::io("move the new workspace to", &path, e)
    })?;
    Ok(Workspace {
        path,
        resumed: false,
    })
}

fn clone_into(problem: &Problem, dir: &Path) -> Result<()> {
    let mut clone = git::command(None);
    clone
        .args([
            "clone",
            "--quiet",
            "--filter=blob:none",
            "--no-checkout",
            "--config",
            "core.autocrlf=false",
            "--",
            &problem.repo,
        ])
        .arg(dir);
    git::output(clone)?;
    git::run(
        Some(dir),
        [
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "--quiet",
            &problem.commit,
        ],
    )?;
    let head = git::head(dir)?;
    if head != problem.commit {
        return Err(Error::new(format!(
            "the checkout of {} is at {head}, not at the pinned commit {}",
            problem.repo, problem.commit
        )));
    }
    verify_spec_files(problem, dir)
}

/// Checks the spec files in the pinned commit against their hashes. A
/// mismatch means the problem itself is broken.
fn verify_spec_files(problem: &Problem, dir: &Path) -> Result<()> {
    let mut broken = Vec::new();
    for (path, expected) in &problem.spec_files {
        let object = format!("{}:{path}", problem.commit);
        match git::run(Some(dir), ["cat-file", "blob", &object]) {
            Ok(bytes) if sha256_hex(&bytes) == *expected => {}
            Ok(bytes) => broken.push(format!(
                "`{path}` has SHA-256 {}, the problem expects {expected}",
                sha256_hex(&bytes)
            )),
            Err(_) => broken.push(format!("`{path}` is not in commit {}", problem.commit)),
        }
    }
    if broken.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!(
            "problem `{}` is broken, its spec files do not match the pinned commit: {}",
            problem.id,
            broken.join("; ")
        )))
    }
}

/// Opens an existing workspace: it must be at the pinned commit and its
/// `.git/config` must be unchanged.
pub fn open(home: &Home, problem: &Problem) -> Result<PathBuf> {
    let path = home.workspace(&problem.id);
    if !path.join(".git").is_dir() {
        return Err(Error::new(format!(
            "there is no workspace for `{}`; run `proofathome work {}` first",
            problem.id, problem.id
        )));
    }
    let hash_file = config_hash_file(home, &problem.id);
    let expected = fs::read_to_string(&hash_file).unwrap_or_default();
    if git_config_hash(&path)? != expected.trim() {
        return Err(Error::new(format!(
            "{} changed since the workspace was prepared, and git settings can run programs; \
             remove {} to start over",
            path.join(".git").join("config").display(),
            path.display()
        )));
    }
    let head = git::head(&path)?;
    if head != problem.commit {
        return Err(Error::new(format!(
            "the workspace {} is at commit {head}, not at the pinned commit {}; \
             do not commit or switch branches there. Remove it to start over",
            path.display(),
            problem.commit
        )));
    }
    Ok(path)
}
