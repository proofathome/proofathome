//! The local pre-check: cheap policy checks, then the pinned checker in a
//! container. The program's own checker runs again on every submitted result
//! and adds checks this one does not do yet, such as ghost-only erasure and
//! a scan for escape hatches like `assume`.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use proofathome_core::Problem;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::fsutil::random_hex;
use crate::git;
use crate::setup::find_in_path;
use crate::workspace::sha256_hex;

pub const RUNTIME_ENV: &str = "PROOFATHOME_CONTAINER_RUNTIME";

const TAIL_LINES: usize = 200;
const TAIL_BYTES: usize = 16 * 1024;

/// The result of `proofathome check`.
#[derive(Debug, Clone, Serialize)]
pub struct LocalCheck {
    pub passed: bool,
    /// Policy violations. When there are any, the checker does not run.
    pub violations: Vec<String>,
    pub checker: Option<CheckerRun>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckerRun {
    pub passed: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// The last lines of combined standard output and error.
    pub output_tail: String,
}

/// Policy checks and then, if they pass, the checker.
pub fn run(problem: &Problem, workspace: &Path) -> Result<LocalCheck> {
    let violations = policy_violations(problem, workspace)?;
    if !violations.is_empty() {
        return Ok(LocalCheck {
            passed: false,
            violations,
            checker: None,
        });
    }
    let runtime = container_runtime()?;
    let checker = run_checker(problem, workspace, &runtime, local_timeout(problem))?;
    Ok(LocalCheck {
        passed: checker.passed,
        violations,
        checker: Some(checker),
    })
}

/// Twice the problem's time budget plus a minute for image start-up.
pub fn local_timeout(problem: &Problem) -> Duration {
    Duration::from_secs(u64::from(problem.time_budget_s) * 2 + 60)
}

/// A changed path from `git status --porcelain=v1 -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// The two status letters, for example ` M`, `??` or `R `.
    pub code: String,
    pub path: String,
    /// The source of a rename or copy.
    pub from: Option<String>,
}

/// Parses NUL-separated `git status --porcelain=v1 -z` output. A rename or
/// copy is followed by its source path as a separate field.
pub fn parse_porcelain(bytes: &[u8]) -> Result<Vec<StatusEntry>> {
    let bad = || Error::new("cannot read the output of `git status`");
    let mut fields = bytes
        .split(|&b| b == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8(field.to_vec()).map_err(|_| bad()));
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        let field = field?;
        if field.len() < 4 || field.as_bytes()[2] != b' ' || !field.is_char_boundary(2) {
            return Err(bad());
        }
        let code = field[..2].to_owned();
        let path = field[3..].to_owned();
        let from = if code.contains(['R', 'C']) {
            Some(fields.next().ok_or_else(bad)??)
        } else {
            None
        };
        entries.push(StatusEntry { code, path, from });
    }
    Ok(entries)
}

/// Every way the workspace breaks the problem's rules, before running any
/// container: changed paths outside `editable`, symlinks, changed spec files.
pub fn policy_violations(problem: &Problem, workspace: &Path) -> Result<Vec<String>> {
    let status = git::run(
        Some(workspace),
        [
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    let entries = parse_porcelain(&status)?;
    let mut violations = Vec::new();
    for entry in &entries {
        let paths = std::iter::once(&entry.path).chain(entry.from.as_ref());
        for path in paths {
            if problem.spec_files.contains_key(path) {
                continue; // Reported by the hash check below.
            }
            if !problem.editable.iter().any(|e| e == path) {
                violations.push(format!(
                    "`{path}` changed ({}) but is not in the editable list",
                    entry.code.trim()
                ));
            } else if is_symlink(&workspace.join(path)) {
                violations.push(format!("`{path}` is a symbolic link; edit the file itself"));
            }
        }
    }
    for (path, expected) in &problem.spec_files {
        match fs::read(workspace.join(path)) {
            Ok(bytes) if sha256_hex(&bytes) == *expected => {}
            Ok(_) => violations.push(format!("spec file `{path}` was changed")),
            Err(_) => violations.push(format!("spec file `{path}` was deleted")),
        }
    }
    Ok(violations)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// `PROOFATHOME_CONTAINER_RUNTIME`, else `docker`, else `podman` from PATH.
pub fn container_runtime() -> Result<PathBuf> {
    if let Some(value) = env::var_os(RUNTIME_ENV).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(&value);
        if path.components().count() > 1 {
            return Ok(path);
        }
        return find_in_path(&value.to_string_lossy()).ok_or_else(|| {
            Error::new(format!(
                "{RUNTIME_ENV} is `{}`, which is not in PATH",
                path.display()
            ))
        });
    }
    find_in_path("docker")
        .or_else(|| find_in_path("podman"))
        .ok_or_else(|| {
            Error::new(
                "no container runtime found: install Docker or Podman, \
                 or set PROOFATHOME_CONTAINER_RUNTIME",
            )
        })
}

/// The arguments after the runtime program.
pub fn container_args(problem: &Problem, workspace: &Path, name: &str) -> Result<Vec<String>> {
    let source = workspace
        .to_str()
        .ok_or_else(|| Error::new(format!("{} is not valid UTF-8", workspace.display())))?;
    if source.contains([',', '"']) {
        return Err(Error::new(format!(
            "the workspace path {source} contains a comma or quote, which a container \
             mount cannot express; set PROOFATHOME_HOME to a simpler path"
        )));
    }
    let command = problem.check_command();
    let mut mount = format!("type=bind,source={source},target=/work");
    if !command.writable {
        mount.push_str(",readonly");
    }
    let mut args: Vec<String> = [
        "run",
        "--rm",
        "--name",
        name,
        "--network",
        "none",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--pids-limit",
        "512",
        "--memory",
        "8g",
        "--mount",
        &mount,
        "--workdir",
        &command.workdir,
        &problem.checker.image.to_string(),
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.extend(command.argv);
    Ok(args)
}

/// Runs the checker container and waits at most `timeout`.
pub fn run_checker(
    problem: &Problem,
    workspace: &Path,
    runtime: &Path,
    timeout: Duration,
) -> Result<CheckerRun> {
    let name = format!("pah-{}-{}", problem.id, &random_hex()[..12]);
    let args = container_args(problem, workspace, &name)?;
    let mut command = Command::new(runtime);
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let start = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|e| Error::new(format!("cannot run {}: {e}", runtime.display())))?;
    let tail = Arc::new(Mutex::new(Tail::default()));
    let readers = [
        child
            .stdout
            .take()
            .map(|out| collect(out, Arc::clone(&tail))),
        child
            .stderr
            .take()
            .map(|err| collect(err, Arc::clone(&tail))),
    ];

    let (exit_code, timed_out) = wait_with_timeout(&mut child, timeout, || {
        let _ = Command::new(runtime)
            .args(["kill", &name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    })?;
    if !timed_out {
        for reader in readers.into_iter().flatten() {
            let _ = reader.join();
        }
    }
    let duration_ms = start.elapsed().as_millis() as u64;
    let mut output_tail = tail.lock().map(|t| t.render()).unwrap_or_default();
    if timed_out {
        output_tail.push_str(&format!(
            "\n[proofathome: stopped after {} s, the local time limit]\n",
            timeout.as_secs()
        ));
    }
    Ok(CheckerRun {
        passed: !timed_out && exit_code == Some(0),
        timed_out,
        exit_code,
        duration_ms,
        output_tail,
    })
}

/// Waits for `child`. On timeout, calls `stop` and kills the child.
/// Returns the exit code and whether the timeout hit.
fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
    stop: impl FnOnce(),
) -> Result<(Option<i32>, bool)> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok((status.code(), false)),
            Ok(None) => {}
            Err(e) => return Err(Error::new(format!("cannot wait for the checker: {e}"))),
        }
        if start.elapsed() >= timeout {
            stop();
            let _ = child.kill();
            let _ = child.wait();
            return Ok((None, true));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn collect(stream: impl Read + Send + 'static, tail: Arc<Mutex<Tail>>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line);
                    if let Ok(mut tail) = tail.lock() {
                        tail.push(text.trim_end_matches(['\n', '\r']));
                    }
                }
            }
        }
    })
}

/// The last `TAIL_LINES` lines, at most `TAIL_BYTES` bytes.
#[derive(Debug, Default)]
pub struct Tail {
    lines: VecDeque<String>,
}

impl Tail {
    pub fn push(&mut self, line: &str) {
        let mut line = line.to_owned();
        if line.len() >= TAIL_BYTES {
            let mut cut = line.len() - (TAIL_BYTES - 1);
            while !line.is_char_boundary(cut) {
                cut += 1;
            }
            line.drain(..cut);
        }
        self.lines.push_back(line);
        if self.lines.len() > TAIL_LINES {
            self.lines.pop_front();
        }
    }

    pub fn render(&self) -> String {
        let mut kept = Vec::new();
        let mut bytes = 0;
        for line in self.lines.iter().rev() {
            if bytes + line.len() + 1 > TAIL_BYTES {
                break;
            }
            bytes += line.len() + 1;
            kept.push(line.as_str());
        }
        kept.reverse();
        let mut out = kept.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_with_renames() {
        let raw = b" M src/a.rs\0?? new file.rs\0R  src/new.rs\0src/old.rs\0D  gone.rs\0";
        let entries = parse_porcelain(raw).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].code, " M");
        assert_eq!(entries[1].path, "new file.rs");
        assert_eq!(entries[2].path, "src/new.rs");
        assert_eq!(entries[2].from.as_deref(), Some("src/old.rs"));
        assert_eq!(entries[3].code, "D ");
        assert!(parse_porcelain(b"").unwrap().is_empty());
        assert!(parse_porcelain(b"R  only-one\0").is_err());
        assert!(parse_porcelain(b"bad\0").is_err());
    }

    #[test]
    fn tail_keeps_the_last_lines_and_bytes() {
        let mut tail = Tail::default();
        for i in 0..300 {
            tail.push(&format!("line {i}"));
        }
        let text = tail.render();
        assert_eq!(text.lines().count(), 200);
        assert!(text.starts_with("line 100\n"));
        assert!(text.ends_with("line 299\n"));

        let mut tail = Tail::default();
        for _ in 0..10 {
            tail.push(&"x".repeat(5000));
        }
        assert!(tail.render().len() <= TAIL_BYTES);
        let mut tail = Tail::default();
        tail.push(&"é".repeat(20_000));
        let text = tail.render();
        assert!(!text.is_empty() && text.len() <= TAIL_BYTES);
    }

    #[cfg(unix)]
    #[test]
    fn a_checker_that_hangs_is_stopped() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let mut stopped = false;
        let start = Instant::now();
        let (code, timed_out) =
            wait_with_timeout(&mut child, Duration::from_millis(200), || stopped = true).unwrap();
        assert!(timed_out && stopped && code.is_none());
        assert!(start.elapsed() < Duration::from_secs(10));
    }
}
