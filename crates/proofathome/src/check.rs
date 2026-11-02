//! The local pre-check: cheap policy checks, then the pinned checker in a
//! container. The program's own checker runs again on every submitted result
//! and adds checks this one does not do yet, such as ghost-only erasure and
//! a scan for escape hatches like `assume`.
//!
//! Cargo-based checks take two containers from the same image. The fetch
//! step has network access and runs only `cargo fetch --locked`, which
//! downloads the crates pinned by `Cargo.lock` into a cache and runs no crate
//! code. The check step has no network and sees the cache read-only.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use proofathome_core::{CONTAINER_WORKDIR, Problem};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::fsutil::{atomic_write, create_dir_all, is_older_than, random_hex};
use crate::git;
use crate::paths::Home;
use crate::setup::find_in_path;
use crate::workspace::sha256_hex;

pub const RUNTIME_ENV: &str = "PROOFATHOME_CONTAINER_RUNTIME";

/// Where the Cargo cache is mounted inside the container.
pub const CARGO_CACHE_TARGET: &str = "/cargo-cache";

/// The fetch step's time limit. It only downloads crates.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// A successful fetch for one `Cargo.lock` is reused for this long. After
/// that, the fetch runs again, in case Cargo cleaned unused files out of the
/// cache.
const FETCH_REUSE: Duration = Duration::from_secs(7 * 24 * 3600);

const TAIL_LINES: usize = 200;
const TAIL_BYTES: usize = 16 * 1024;

/// The result of `proofathome check`.
#[derive(Debug, Clone, Serialize)]
pub struct LocalCheck {
    pub passed: bool,
    /// Policy violations. When there are any, no container runs.
    pub violations: Vec<String>,
    /// The dependency fetch, for Cargo-based checks. When it fails, the
    /// checker does not run.
    pub fetch: Option<ContainerRun>,
    /// Whether the fetch was skipped because the same `Cargo.lock` was
    /// fetched recently.
    pub fetch_skipped: bool,
    pub checker: Option<ContainerRun>,
}

/// One container run.
#[derive(Debug, Clone, Serialize)]
pub struct ContainerRun {
    pub passed: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// The last lines of combined standard output and error.
    pub output_tail: String,
}

/// Policy checks and then, if they pass, the dependency fetch (for Cargo
/// checks) and the checker.
pub fn run(home: &Home, problem: &Problem, workspace: &Path) -> Result<LocalCheck> {
    let mut result = LocalCheck {
        passed: false,
        violations: policy_violations(problem, workspace)?,
        fetch: None,
        fetch_skipped: false,
        checker: None,
    };
    if !result.violations.is_empty() {
        return Ok(result);
    }
    let runtime = container_runtime()?;
    let user = UserMapping::for_runtime(&runtime, home.root());
    let command = problem.check_command();
    let cache = if command.cargo {
        let cache = home
            .cache_dir()
            .join(format!("cargo-{}", problem.checker.tool));
        create_dir_all(&cache)?;
        let lock = find_lock_file(problem, workspace)?;
        let marker = cache
            .join(".proofathome")
            .join(format!("fetched-{}", sha256_hex(&lock)));
        if marker.is_file() && !is_older_than(&marker, FETCH_REUSE) {
            result.fetch_skipped = true;
        } else {
            let spec = fetch_spec(problem, workspace, &cache, &user)?;
            let fetch = run_container(&runtime, &spec, FETCH_TIMEOUT)?;
            let fetched = fetch.passed;
            result.fetch = Some(fetch);
            if !fetched {
                return Ok(result);
            }
            atomic_write(&marker, b"")?;
        }
        Some(cache)
    } else {
        None
    };
    let spec = check_spec(problem, workspace, cache.as_deref(), &user)?;
    let checker = run_container(&runtime, &spec, local_timeout(problem))?;
    result.passed = checker.passed;
    result.checker = Some(checker);
    Ok(result)
}

/// Twice the problem's time budget plus a minute for image start-up.
pub fn local_timeout(problem: &Problem) -> Duration {
    Duration::from_secs(u64::from(problem.time_budget_s) * 2 + 60)
}

/// The content of the `Cargo.lock` that pins a Cargo check: in the manifest
/// directory or the nearest directory above it, inside the repository.
fn find_lock_file(problem: &Problem, workspace: &Path) -> Result<Vec<u8>> {
    let workdir = problem.check_command().workdir;
    let manifest_dir = workdir
        .strip_prefix(CONTAINER_WORKDIR)
        .unwrap_or_default()
        .trim_start_matches('/');
    let mut dir = manifest_dir;
    loop {
        let path = workspace.join(dir).join("Cargo.lock");
        if path.is_file() {
            return fs::read(&path).map_err(|e| Error::io("read", &path, e));
        }
        if dir.is_empty() {
            break;
        }
        dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    let place = if manifest_dir.is_empty() {
        "the repository root".to_owned()
    } else {
        format!("`{manifest_dir}` or above it")
    };
    Err(Error::new(format!(
        "problem `{}` is broken: there is no Cargo.lock in {place}. Cargo-based checks \
         only use dependencies pinned by a committed Cargo.lock",
        problem.id
    )))
}

/// How container processes map to the user on the host, so that files they
/// write into the cache belong to that user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserMapping {
    /// Leave it to the runtime: Docker Desktop on Windows, where file
    /// ownership in bind mounts does not apply.
    Default,
    /// `--user <uid>:<gid>`, the owner of the home directory.
    User(String),
    /// Podman: `--userns=keep-id` maps the calling user into the container,
    /// also when Podman is rootless.
    KeepId,
}

impl UserMapping {
    pub fn for_runtime(runtime: &Path, home: &Path) -> UserMapping {
        let is_podman = runtime
            .file_stem()
            .is_some_and(|name| name.to_string_lossy().contains("podman"));
        if is_podman {
            return UserMapping::KeepId;
        }
        owner_of(home).map_or(UserMapping::Default, UserMapping::User)
    }

    fn args(&self) -> Vec<String> {
        match self {
            UserMapping::Default => Vec::new(),
            UserMapping::User(user) => vec!["--user".to_owned(), user.clone()],
            UserMapping::KeepId => vec!["--userns=keep-id".to_owned()],
        }
    }
}

#[cfg(unix)]
fn owner_of(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).ok()?;
    Some(format!("{}:{}", meta.uid(), meta.gid()))
}

#[cfg(not(unix))]
fn owner_of(_path: &Path) -> Option<String> {
    None
}

/// Everything about one container run except the runtime program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSpec {
    pub name: String,
    pub image: String,
    /// The runtime's default network when true, `--network none` when false.
    pub network: bool,
    pub user: UserMapping,
    /// `--mount` values.
    pub mounts: Vec<String>,
    pub env: Vec<(String, String)>,
    pub workdir: String,
    pub argv: Vec<String>,
}

impl ContainerSpec {
    /// The arguments after the runtime program.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "run".to_owned(),
            "--rm".to_owned(),
            "--name".to_owned(),
            self.name.clone(),
        ];
        if !self.network {
            args.extend(["--network", "none"].map(str::to_owned));
        }
        args.extend(
            [
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges",
                "--pids-limit",
                "512",
                "--memory",
                "8g",
            ]
            .map(str::to_owned),
        );
        args.extend(self.user.args());
        for mount in &self.mounts {
            args.push("--mount".to_owned());
            args.push(mount.clone());
        }
        for (key, value) in &self.env {
            args.push("--env".to_owned());
            args.push(format!("{key}={value}"));
        }
        args.push("--workdir".to_owned());
        args.push(self.workdir.clone());
        args.push(self.image.clone());
        args.extend(self.argv.iter().cloned());
        args
    }
}

fn mount(source: &Path, target: &str, read_only: bool) -> Result<String> {
    let text = source
        .to_str()
        .ok_or_else(|| Error::new(format!("{} is not valid UTF-8", source.display())))?;
    if text.contains([',', '"']) {
        return Err(Error::new(format!(
            "the path {text} contains a comma or quote, which a container mount cannot \
             express; set PROOFATHOME_HOME to a simpler path"
        )));
    }
    let mut mount = format!("type=bind,source={text},target={target}");
    if read_only {
        mount.push_str(",readonly");
    }
    Ok(mount)
}

fn container_name(problem: &Problem, step: &str) -> String {
    format!("pah-{}-{step}-{}", problem.id, &random_hex()[..12])
}

/// The fetch step: network on, repository read-only, cache writable.
pub fn fetch_spec(
    problem: &Problem,
    workspace: &Path,
    cache: &Path,
    user: &UserMapping,
) -> Result<ContainerSpec> {
    let command = problem.check_command();
    let argv = command
        .fetch_argv()
        .ok_or_else(|| Error::new("this check has no dependencies to fetch"))?;
    Ok(ContainerSpec {
        name: container_name(problem, "fetch"),
        image: problem.checker.image.to_string(),
        network: true,
        user: user.clone(),
        mounts: vec![
            mount(workspace, CONTAINER_WORKDIR, true)?,
            mount(cache, CARGO_CACHE_TARGET, false)?,
        ],
        env: vec![("CARGO_HOME".to_owned(), CARGO_CACHE_TARGET.to_owned())],
        workdir: command.workdir,
        argv,
    })
}

/// The check step: no network, repository read-only, and for Cargo checks
/// the cache read-only with Cargo offline.
pub fn check_spec(
    problem: &Problem,
    workspace: &Path,
    cache: Option<&Path>,
    user: &UserMapping,
) -> Result<ContainerSpec> {
    let command = problem.check_command();
    let mut mounts = vec![mount(workspace, CONTAINER_WORKDIR, true)?];
    let mut env = Vec::new();
    if let Some(cache) = cache {
        mounts.push(mount(cache, CARGO_CACHE_TARGET, true)?);
        env.push(("CARGO_HOME".to_owned(), CARGO_CACHE_TARGET.to_owned()));
        env.push(("CARGO_NET_OFFLINE".to_owned(), "true".to_owned()));
    }
    Ok(ContainerSpec {
        name: container_name(problem, "check"),
        image: problem.checker.image.to_string(),
        network: false,
        user: user.clone(),
        mounts,
        env,
        workdir: command.workdir,
        argv: command.argv,
    })
}

/// Runs one container and waits at most `timeout`.
pub fn run_container(
    runtime: &Path,
    spec: &ContainerSpec,
    timeout: Duration,
) -> Result<ContainerRun> {
    let mut command = Command::new(runtime);
    command
        .args(spec.args())
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
            .args(["kill", &spec.name])
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
    Ok(ContainerRun {
        passed: !timed_out && exit_code == Some(0),
        timed_out,
        exit_code,
        duration_ms,
        output_tail,
    })
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
    fn podman_keeps_the_calling_user() {
        let home = std::env::temp_dir();
        assert_eq!(
            UserMapping::for_runtime(Path::new("/usr/bin/podman"), &home),
            UserMapping::KeepId
        );
        assert_eq!(UserMapping::KeepId.args(), ["--userns=keep-id"]);
        let docker = UserMapping::for_runtime(Path::new("/usr/bin/docker"), &home);
        if cfg!(unix) {
            assert!(matches!(docker, UserMapping::User(_)));
        } else {
            assert_eq!(docker, UserMapping::Default);
            assert!(docker.args().is_empty());
        }
    }

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
