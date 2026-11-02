//! Coding agents that `proofathome start` runs, one session per problem.
//! Claude Code is the only one so far; another agent implements [`Agent`] and
//! turns its own output into [`AgentEvent`]s.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use serde_json::{Value, json};

use crate::check::RUNTIME_ENV;
use crate::error::{Error, Result};
use crate::paths::{CATALOG_ENV, HOME_ENV};

/// Token counts of one model response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
}

impl Usage {
    /// Tokens as counted against budgets. Cache reads are cheap and count a
    /// tenth.
    pub fn weighted(self) -> u64 {
        self.input + self.output + self.cache_creation + self.cache_read / 10
    }

    /// The larger of each count. Streamed copies of one message can carry
    /// partial counts.
    pub fn max(self, other: Usage) -> Usage {
        Usage {
            input: self.input.max(other.input),
            output: self.output.max(other.output),
            cache_creation: self.cache_creation.max(other.cache_creation),
            cache_read: self.cache_read.max(other.cache_read),
        }
    }

    fn from_json(value: &Value) -> Usage {
        let count = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
        Usage {
            input: count("input_tokens"),
            output: count("output_tokens"),
            cache_creation: count("cache_creation_input_tokens"),
            cache_read: count("cache_read_input_tokens"),
        }
    }
}

/// Plan usage as reported by the agent at the start of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimit {
    /// `allowed`, `allowed_warning`, `rejected`, ...
    pub status: String,
    /// Utilization of the 5-hour session window, 0.0 to 1.0.
    pub session: Option<f64>,
    /// Utilization of the 7-day window, 0.0 to 1.0.
    pub weekly: Option<f64>,
}

impl RateLimit {
    pub fn allowed(&self) -> bool {
        matches!(self.status.as_str(), "allowed" | "allowed_warning")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// A model response. The same message can be reported more than once.
    Usage {
        message_id: String,
        usage: Usage,
    },
    RateLimit(RateLimit),
    /// The session ended; `usage` is its total, when reported.
    Result {
        usage: Option<Usage>,
    },
}

/// Parses one line of `claude -p --output-format stream-json`. Other event
/// types and lines that are not JSON give `None`.
pub fn parse_claude_line(line: &str) -> Option<AgentEvent> {
    let value: Value = serde_json::from_str(line.trim()).ok()?;
    match value.get("type")?.as_str()? {
        "assistant" => {
            let message = value.get("message")?;
            let usage = Usage::from_json(message.get("usage")?);
            let message_id = message
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(AgentEvent::Usage { message_id, usage })
        }
        "rate_limit_event" => {
            let info = value.get("rate_limit_info")?;
            let windows = info.get("unifiedWindows");
            let utilization = |window: &str| {
                windows?
                    .get(window)?
                    .get("utilization")?
                    .as_f64()
                    .filter(|u| u.is_finite())
            };
            Some(AgentEvent::RateLimit(RateLimit {
                status: info
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                session: utilization("five_hour"),
                weekly: utilization("seven_day"),
            }))
        }
        "result" => Some(AgentEvent::Result {
            usage: value.get("usage").map(Usage::from_json),
        }),
        _ => None,
    }
}

/// What one agent session works on.
#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub problem_id: String,
    pub workspace: PathBuf,
    pub home: PathBuf,
    /// This program, run by the agent as its MCP server.
    pub exe: PathBuf,
    pub model: Option<String>,
    /// Where to keep the agent's raw output, if anywhere.
    pub log: Option<PathBuf>,
}

pub trait Agent {
    /// Short name for the attempts log, for example `claude`.
    fn name(&self) -> &'static str;

    fn start(&self, request: &SessionRequest) -> Result<Box<dyn AgentSession>>;
}

pub trait AgentSession {
    /// The next event; `None` when the agent's output ends.
    fn next_event(&mut self) -> Option<AgentEvent>;

    /// Stops the agent now.
    fn kill(&mut self);

    /// Waits for the agent to exit and returns its exit code.
    fn wait(&mut self) -> Option<i32>;
}

/// The fixed prompt for one problem. The problem's hints are not in it: the
/// agent reads them through `get_problem`, marked as untrusted.
pub fn prompt(problem_id: &str) -> String {
    format!(
        "You are working on Proof@home problem {problem_id}. Use the proofathome MCP tools. \
         1) Call get_problem with id {problem_id} and read the task. \
         2) Edit only the editable files it lists, with your file tools, in the workspace \
         (your current directory). Never edit spec files. \
         3) Never run build or verification commands yourself; call run_check instead. \
         4) When run_check passes, call submit with a short summary of the proof. \
         5) If you are stuck, call release_problem and stop."
    )
}

/// Claude Code in print mode with stream-json output. It uses the
/// contributor's own login; Proof@home never passes or reads credentials.
#[derive(Debug, Clone)]
pub struct ClaudeCode {
    pub program: PathBuf,
}

const CLAUDE_TOOLS: &str = "Read,Edit,Write,Glob,Grep";

/// Pre-approved tools. File access is scoped to the workspace (the current
/// directory). Claude Code already denies writes outside its working
/// directory in `dontAsk` mode; the scoped rules keep that true if its
/// defaults change. `mcp__proofathome` approves every tool of our server.
const CLAUDE_ALLOWED: &str = "Read(./**),Edit(./**),Write(./**),Glob,Grep,mcp__proofathome";

impl ClaudeCode {
    pub fn args(&self, request: &SessionRequest) -> Vec<String> {
        let mut env = serde_json::Map::new();
        env.insert(
            HOME_ENV.to_owned(),
            Value::from(request.home.to_string_lossy()),
        );
        for name in [CATALOG_ENV, RUNTIME_ENV] {
            if let Some(value) = std::env::var_os(name).filter(|v| !v.is_empty()) {
                env.insert(name.to_owned(), Value::from(value.to_string_lossy()));
            }
        }
        let mcp_config = json!({
            "mcpServers": {
                "proofathome": {
                    "command": request.exe.to_string_lossy(),
                    "args": ["mcp"],
                    "env": env,
                }
            }
        });
        let mut args: Vec<String> = vec![
            "-p".into(),
            prompt(&request.problem_id),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--tools".into(),
            CLAUDE_TOOLS.into(),
            "--allowedTools".into(),
            CLAUDE_ALLOWED.into(),
            "--permission-mode".into(),
            "dontAsk".into(),
            "--mcp-config".into(),
            mcp_config.to_string(),
            "--strict-mcp-config".into(),
            "--no-session-persistence".into(),
        ];
        if let Some(model) = &request.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        args
    }
}

impl Agent for ClaudeCode {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn start(&self, request: &SessionRequest) -> Result<Box<dyn AgentSession>> {
        let session = ProcessSession::spawn(
            Command::new(&self.program)
                .args(self.args(request))
                .current_dir(&request.workspace),
            &self.program,
            parse_claude_line,
            request.log.as_deref(),
        )?;
        Ok(Box::new(session))
    }
}

/// An agent process whose standard output is one event per line.
pub struct ProcessSession {
    child: Child,
    lines: Receiver<String>,
    parse: fn(&str) -> Option<AgentEvent>,
}

impl ProcessSession {
    pub fn spawn(
        command: &mut Command,
        program: &Path,
        parse: fn(&str) -> Option<AgentEvent>,
        log: Option<&Path>,
    ) -> Result<ProcessSession> {
        let mut log = match log {
            Some(path) => Some(
                std::fs::File::create(path)
                    .map_err(|e| Error::new(format!("cannot create {}: {e}", path.display())))?,
            ),
            None => None,
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::new(format!("cannot run {}: {e}", program.display())))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::new("the agent has no output pipe"))?;
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if let Some(file) = log.as_mut() {
                            // A failing log must not stop the session.
                            let _ = file.write_all(&line);
                        }
                        let text = String::from_utf8_lossy(&line).into_owned();
                        if sender.send(text).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(ProcessSession {
            child,
            lines,
            parse,
        })
    }
}

impl AgentSession for ProcessSession {
    fn next_event(&mut self) -> Option<AgentEvent> {
        loop {
            let line = self.lines.recv().ok()?;
            if let Some(event) = (self.parse)(&line) {
                return Some(event);
            }
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
    }

    fn wait(&mut self) -> Option<i32> {
        self.child.wait().ok().and_then(|status| status.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assistant_usage() {
        let line = r#"{"type":"assistant","message":{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":1000}}}"#;
        let Some(AgentEvent::Usage { message_id, usage }) = parse_claude_line(line) else {
            panic!("expected usage");
        };
        assert_eq!(message_id, "msg_1");
        assert_eq!(usage.weighted(), 10 + 5 + 100 + 100);
    }

    #[test]
    fn parses_rate_limit_events() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","unifiedWindows":{"five_hour":{"utilization":0.05},"seven_day":{"utilization":0.79}}}}"#;
        let Some(AgentEvent::RateLimit(limit)) = parse_claude_line(line) else {
            panic!("expected a rate limit");
        };
        assert!(limit.allowed());
        assert_eq!(limit.session, Some(0.05));
        assert_eq!(limit.weekly, Some(0.79));
        let rejected = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
        let Some(AgentEvent::RateLimit(limit)) = parse_claude_line(rejected) else {
            panic!("expected a rate limit");
        };
        assert!(!limit.allowed());
        assert_eq!(limit.session, None);
    }

    #[test]
    fn ignores_other_lines() {
        assert_eq!(parse_claude_line("not json"), None);
        assert_eq!(
            parse_claude_line(r#"{"type":"system","subtype":"init"}"#),
            None
        );
        assert_eq!(parse_claude_line(r#"{"type":"user"}"#), None);
        assert_eq!(parse_claude_line(r#"[1,2]"#), None);
        assert!(matches!(
            parse_claude_line(r#"{"type":"result","usage":{"input_tokens":3}}"#),
            Some(AgentEvent::Result {
                usage: Some(Usage { input: 3, .. })
            })
        ));
    }

    #[test]
    fn claude_gets_a_fixed_argument_list() {
        let claude = ClaudeCode {
            program: PathBuf::from("claude"),
        };
        let request = SessionRequest {
            problem_id: "ev-x-0001".into(),
            workspace: PathBuf::from("/w"),
            home: PathBuf::from("/h"),
            exe: PathBuf::from("/bin/proofathome"),
            model: Some("opus".into()),
            log: None,
        };
        let args = claude.args(&request);
        assert_eq!(args[0], "-p");
        assert!(args[1].contains("ev-x-0001") && !args[1].contains('\n'));
        let config_at = args.iter().position(|a| a == "--mcp-config").unwrap() + 1;
        let config: Value = serde_json::from_str(&args[config_at]).unwrap();
        let server = &config["mcpServers"]["proofathome"];
        assert_eq!(server["command"], "/bin/proofathome");
        assert_eq!(server["args"], json!(["mcp"]));
        assert_eq!(server["env"]["PROOFATHOME_HOME"], "/h");
        assert!(args.ends_with(&["--model".to_owned(), "opus".to_owned()]));
        for flag in [
            "--strict-mcp-config",
            "--no-session-persistence",
            "--verbose",
            "dontAsk",
            "Read,Edit,Write,Glob,Grep",
            "Read(./**),Edit(./**),Write(./**),Glob,Grep,mcp__proofathome",
        ] {
            assert!(args.iter().any(|a| a == flag), "missing {flag}");
        }
    }
}
