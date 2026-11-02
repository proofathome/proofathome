//! `proofathome setup`: register the MCP server with the agents found on
//! this machine and check for a container runtime.

use std::env;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::check;
use crate::error::{Error, Result};

pub const CLAUDE_BIN_ENV: &str = "PROOFATHOME_CLAUDE_BIN";

/// Finds `name` in PATH. On Windows, also tries the extensions in PATHEXT.
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let extensions: Vec<OsString> = if cfg!(windows) {
        let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        std::iter::once(OsString::new())
            .chain(
                pathext
                    .split(';')
                    .filter(|e| !e.is_empty())
                    .map(OsString::from),
            )
            .collect()
    } else {
        vec![OsString::new()]
    };
    for dir in env::split_paths(&path) {
        for ext in &extensions {
            let mut file = OsString::from(name);
            file.push(ext);
            let candidate = dir.join(file);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// The `claude` program: `PROOFATHOME_CLAUDE_BIN`, else `claude` in PATH.
pub fn claude_program() -> Option<PathBuf> {
    match env::var_os(CLAUDE_BIN_ENV).filter(|v| !v.is_empty()) {
        Some(path) => Some(PathBuf::from(path)),
        None => find_in_path("claude"),
    }
}

pub fn current_exe() -> Result<PathBuf> {
    env::current_exe().map_err(|e| Error::new(format!("cannot find this program's path: {e}")))
}

/// The MCP client config for other clients.
pub fn mcp_json(exe: &Path) -> String {
    let value = serde_json::json!({
        "mcpServers": {
            "proofathome": { "command": exe.to_string_lossy(), "args": ["mcp"] }
        }
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

fn quiet_status(program: &Path, args: &[&str]) -> std::io::Result<bool> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
}

fn run_shown(out: &mut dyn Write, program: &Path, args: &[&str]) -> Result<bool> {
    let _ = writeln!(out, "  $ {} {}", program.display(), args.join(" "));
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::new(format!("cannot run {}: {e}", program.display())))?;
    for line in String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
    {
        let _ = writeln!(out, "    {line}");
    }
    Ok(output.status.success())
}

/// Runs setup and reports to `out`. Returns an error only when something
/// that was found could not be set up.
pub fn run(out: &mut dyn Write) -> Result<()> {
    let exe = current_exe()?;
    let exe_text = exe.to_string_lossy().into_owned();
    let mut failed = Vec::new();

    match claude_program() {
        Some(claude) => {
            let _ = writeln!(out, "Claude Code: found {}", claude.display());
            if quiet_status(&claude, &["mcp", "get", "proofathome"]).unwrap_or(false) {
                run_shown(
                    out,
                    &claude,
                    &["mcp", "remove", "-s", "user", "proofathome"],
                )?;
            }
            let added = run_shown(
                out,
                &claude,
                &[
                    "mcp",
                    "add",
                    "-s",
                    "user",
                    "proofathome",
                    "--",
                    &exe_text,
                    "mcp",
                ],
            )?;
            if added {
                let _ = writeln!(out, "  registered the proofathome MCP server for your user");
            } else {
                failed.push("Claude Code");
            }
        }
        None => {
            let _ = writeln!(out, "Claude Code: not found (looked for `claude` in PATH)");
        }
    }

    match find_in_path("codex") {
        Some(codex) => {
            let _ = writeln!(
                out,
                "Codex: found {} (experimental, not verified with Proof@home yet)",
                codex.display()
            );
            let added = run_shown(
                out,
                &codex,
                &["mcp", "add", "proofathome", "--", &exe_text, "mcp"],
            )?;
            if !added {
                failed.push("Codex");
            }
        }
        None => {
            let _ = writeln!(out, "Codex: not found");
        }
    }

    match check::container_runtime() {
        Ok(runtime) => {
            let _ = writeln!(out, "Container runtime: {}", runtime.display());
        }
        Err(error) => {
            let _ = writeln!(
                out,
                "Container runtime: missing. `check` and `submit` need one: {error}"
            );
        }
    }

    let _ = writeln!(
        out,
        "\nFor other MCP clients, add this server to their config:\n{}",
        mcp_json(&exe)
    );

    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!(
            "could not register the MCP server with {}",
            failed.join(" and ")
        )))
    }
}
