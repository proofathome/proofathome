//! Running `git`, always with an argument list and never through a shell.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// A `git` command. With `dir`, it runs in that repository. Settings that
/// would let repository files run programs are turned off.
pub fn command(dir: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    command
        .args(["-c", "core.fsmonitor=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    command
}

/// Runs `git` with `args` and returns its standard output.
pub fn run<I, S>(dir: Option<&Path>, args: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(dir);
    command.args(args);
    output(command)
}

/// Runs a prepared `git` command and returns its standard output, or an
/// error with its standard error.
pub fn output(mut command: Command) -> Result<Vec<u8>> {
    let shown = describe(&command);
    let output = command.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::new("`git` was not found; install Git and make sure it is in PATH")
        } else {
            Error::new(format!("cannot run `{shown}`: {e}"))
        }
    })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(Error::new(format!(
            "`{shown}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn describe(command: &Command) -> String {
    let args: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    // Skip `-C <dir>` and the safety settings to keep messages short.
    let mut shown = vec!["git".to_owned()];
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "-C" || arg == "-c" {
            iter.next();
        } else {
            shown.push(arg.clone());
        }
    }
    shown.join(" ")
}

/// `git rev-parse HEAD` in `dir`.
pub fn head(dir: &Path) -> Result<String> {
    let out = run(Some(dir), ["rev-parse", "HEAD"])?;
    Ok(String::from_utf8_lossy(&out).trim().to_owned())
}
