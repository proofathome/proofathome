//! The `start` runner with a scripted agent. The agent runs in-process, so
//! these tests work on every platform; the Git redirect to the local fixture
//! repository is set once for this test binary.

mod common;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use common::Fixture;
use proofathome::agent::{Agent, AgentEvent, AgentSession, SessionRequest, parse_claude_line};
use proofathome::app::App;
use proofathome::backend::{Backend, Status, Submission};
use proofathome::error::Result;
use proofathome::local::LocalBackend;
use proofathome::paths::Home;
use proofathome::proofathome_core::Size;
use proofathome::runner::{Budgets, Outcome, StartOptions, run};

/// Each test gets its own fixture, but Git's URL rewrite comes from the
/// process environment, which can only be set once safely. All fixtures in
/// this binary share one upstream directory.
fn fixture(name: &str) -> Fixture {
    static REMOTES: OnceLock<PathBuf> = OnceLock::new();
    let remotes = REMOTES.get_or_init(|| {
        let shared = Fixture::new("start-shared");
        for (key, value) in shared.env() {
            if key.starts_with("GIT_CONFIG") {
                // SAFETY: runs once, before any test in this binary starts
                // Git; std's own environment lock covers concurrent readers.
                unsafe { std::env::set_var(key, value) };
            }
        }
        shared.remotes.clone()
    });
    let mut f = Fixture::new(name);
    f.commit = common::git(&remotes.join("example-virtio"), &["rev-parse", "HEAD"]);
    f.remotes = remotes.clone();
    f
}

fn app(f: &Fixture) -> App {
    let home = Home::new(&f.home).unwrap();
    App::new(
        home.clone(),
        Box::new(LocalBackend::new(home, f.catalog.clone())),
    )
}

fn backend(f: &Fixture) -> LocalBackend {
    LocalBackend::new(Home::new(&f.home).unwrap(), f.catalog.clone())
}

fn assistant(id: &str, input: u64) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"id":"{id}","usage":{{"input_tokens":{input},"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#
    )
}

fn rate_limit(status: &str, session: f64, weekly: f64) -> String {
    format!(
        r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"{status}","unifiedWindows":{{"five_hour":{{"utilization":{session}}},"seven_day":{{"utilization":{weekly}}}}}}}}}"#
    )
}

/// One scripted session: lines to emit and whether the agent submits.
struct Script {
    lines: Vec<String>,
    submit: bool,
}

fn script(lines: &[String], submit: bool) -> Script {
    Script {
        lines: lines.to_vec(),
        submit,
    }
}

#[derive(Default)]
struct Log {
    started: Vec<String>,
    /// Lines each session emitted before it ended or was killed.
    emitted: Vec<usize>,
    killed: Vec<bool>,
}

struct FakeAgent {
    scripts: RefCell<VecDeque<Script>>,
    backend: LocalBackend,
    log: Rc<RefCell<Log>>,
}

impl FakeAgent {
    fn new(f: &Fixture, scripts: Vec<Script>) -> FakeAgent {
        FakeAgent {
            scripts: RefCell::new(scripts.into()),
            backend: backend(f),
            log: Rc::default(),
        }
    }
}

impl Agent for FakeAgent {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn start(&self, request: &SessionRequest) -> Result<Box<dyn AgentSession>> {
        assert!(request.workspace.join(".git").is_dir());
        let script = self
            .scripts
            .borrow_mut()
            .pop_front()
            .expect("no script left");
        self.log
            .borrow_mut()
            .started
            .push(request.problem_id.clone());
        if script.submit {
            // What the MCP `submit` tool does once the check passes.
            self.backend
                .submit(&Submission {
                    problem_id: request.problem_id.clone(),
                    patch: Vec::new(),
                    summary: serde_json::json!({}),
                })
                .unwrap();
        }
        let index = self.log.borrow().emitted.len();
        self.log.borrow_mut().emitted.push(0);
        self.log.borrow_mut().killed.push(false);
        Ok(Box::new(FakeSession {
            lines: script.lines.into(),
            log: Rc::clone(&self.log),
            index,
        }))
    }
}

struct FakeSession {
    lines: VecDeque<String>,
    log: Rc<RefCell<Log>>,
    index: usize,
}

impl AgentSession for FakeSession {
    fn next_event(&mut self) -> Option<AgentEvent> {
        while let Some(line) = self.lines.pop_front() {
            self.log.borrow_mut().emitted[self.index] += 1;
            if let Some(event) = parse_claude_line(&line) {
                return Some(event);
            }
        }
        None
    }

    fn kill(&mut self) {
        self.log.borrow_mut().killed[self.index] = true;
    }

    fn wait(&mut self) -> Option<i32> {
        Some(0)
    }
}

fn options(tokens: Option<u64>, session: Option<f64>) -> StartOptions {
    StartOptions {
        budgets: Budgets {
            tokens,
            session,
            weekly: None,
            ceiling: 90.0,
        },
        max_size: Size::M,
        allow_xl: false,
        max_problems: None,
        problem: None,
        model: None,
        exe: PathBuf::from("proofathome"),
    }
}

fn status(f: &Fixture, id: &str) -> Status {
    backend(f).get(id).unwrap().status
}

#[test]
fn counts_each_message_once_and_records_success() {
    let f = fixture("count");
    f.add_problem("ev-a-0001", "S", 1);
    let lines = [
        rate_limit("allowed", 0.1, 0.2),
        "not json".to_owned(),
        assistant("m1", 1000),
        assistant("m1", 1000),
        assistant("m1", 1000),
        assistant("m2", 500),
        r#"{"type":"result","usage":{"input_tokens":1500}}"#.to_owned(),
    ];
    let agent = FakeAgent::new(&f, vec![script(&lines, true)]);
    let mut out = Vec::new();
    let summary = run(&app(&f), &agent, &options(Some(1_000_000), None), &mut out).unwrap();
    assert_eq!(summary.results.len(), 1);
    assert_eq!(summary.results[0].outcome, Outcome::Submitted);
    assert_eq!(summary.results[0].tokens, 1500);
    assert_eq!(summary.tokens, 1500);
    assert!(matches!(status(&f, "ev-a-0001"), Status::Submitted { .. }));
    let attempts = backend(&f).attempts().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].outcome, "submitted");
    assert_eq!(attempts[0].tokens, 1500);
    assert_eq!(attempts[0].agent, "fake");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("ev-a-0001: submitted (1500 tokens"), "{out}");
    assert!(out.contains("no open problem fits"), "{out}");
    assert!(!f.home.join("state/runner.json").exists());
}

#[test]
fn kills_a_session_over_the_problem_cap_and_releases() {
    let f = fixture("cap");
    f.add_problem("ev-a-0001", "XS", 1);
    let lines = [
        assistant("m1", 60_000),
        assistant("m2", 60_000),
        assistant("m3", 60_000),
    ];
    let agent = FakeAgent::new(&f, vec![script(&lines, false)]);
    let summary = run(
        &app(&f),
        &agent,
        &options(Some(10_000_000), None),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(summary.results[0].outcome, Outcome::ProblemCap);
    assert_eq!(summary.results[0].tokens, 120_000);
    let log = agent.log.borrow();
    assert_eq!(log.emitted, [2]);
    assert_eq!(log.killed, [true]);
    assert_eq!(status(&f, "ev-a-0001"), Status::Open);
    assert_eq!(
        backend(&f).attempts().unwrap()[0].outcome,
        "problem-token-cap"
    );
}

#[test]
fn stops_when_the_token_budget_runs_out() {
    let f = fixture("total");
    f.add_problem("ev-a-0001", "XS", 1);
    f.add_problem("ev-b-0002", "XS", 2);
    f.add_problem("ev-c-0003", "XS", 3);
    let first = [assistant("m1", 40_000)];
    let second = [
        assistant("m1", 50_000),
        assistant("m2", 50_000),
        assistant("m3", 50_000),
    ];
    let agent = FakeAgent::new(&f, vec![script(&first, false), script(&second, false)]);
    let mut out = Vec::new();
    let summary = run(&app(&f), &agent, &options(Some(140_000), None), &mut out).unwrap();
    assert_eq!(agent.log.borrow().started, ["ev-a-0001", "ev-b-0002"]);
    assert_eq!(summary.results[1].outcome, Outcome::TokenBudget);
    assert_eq!(summary.tokens, 140_000);
    assert_eq!(agent.log.borrow().killed, [false, true]);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("token budget is used up")
    );
}

#[test]
fn only_problems_that_fit_the_remaining_tokens_are_picked() {
    let f = fixture("fit");
    f.add_problem("ev-a-0001", "S", 1);
    f.add_problem("ev-b-0002", "XS", 5);
    let agent = FakeAgent::new(&f, vec![script(&[assistant("m1", 10)], false)]);
    // 300k fits XS (100k) but not S (400k), even though S is easier.
    let summary = run(
        &app(&f),
        &agent,
        &options(Some(300_000), None),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(agent.log.borrow().started, ["ev-b-0002"]);
    assert_eq!(summary.results[0].outcome, Outcome::NotSubmitted);
    assert_eq!(status(&f, "ev-b-0002"), Status::Open);
}

#[test]
fn continues_within_the_session_budget_and_stops_when_it_is_used() {
    let f = fixture("session");
    f.add_problem("ev-a-0001", "XS", 1);
    f.add_problem("ev-b-0002", "XS", 2);
    f.add_problem("ev-c-0003", "XS", 3);
    f.add_problem("ev-d-0004", "XS", 4);
    let agent = FakeAgent::new(
        &f,
        vec![
            script(
                &[rate_limit("allowed", 0.20, 0.5), assistant("m", 10)],
                true,
            ),
            script(
                &[rate_limit("allowed", 0.25, 0.5), assistant("m", 10)],
                false,
            ),
            script(
                &[rate_limit("allowed_warning", 0.30, 0.5), assistant("m", 10)],
                false,
            ),
        ],
    );
    let mut out = Vec::new();
    let summary = run(&app(&f), &agent, &options(None, Some(10.0)), &mut out).unwrap();
    assert_eq!(
        agent.log.borrow().started,
        ["ev-a-0001", "ev-b-0002", "ev-c-0003"]
    );
    assert_eq!(summary.results[2].outcome, Outcome::PlanBudget);
    assert_eq!(agent.log.borrow().emitted[2], 1, "killed at session start");
    assert_eq!(status(&f, "ev-c-0003"), Status::Open);
    assert!(
        summary.stop_reason.contains("session budget used"),
        "{}",
        summary.stop_reason
    );
}

#[test]
fn stops_at_the_ceiling() {
    let f = fixture("ceiling");
    f.add_problem("ev-a-0001", "XS", 1);
    f.add_problem("ev-b-0002", "XS", 2);
    let agent = FakeAgent::new(
        &f,
        vec![script(
            &[rate_limit("allowed", 0.1, 0.95), assistant("m", 10)],
            false,
        )],
    );
    let summary = run(
        &app(&f),
        &agent,
        &options(Some(1_000_000), None),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(summary.results.len(), 1);
    assert_eq!(summary.results[0].outcome, Outcome::Ceiling);
    assert!(summary.stop_reason.contains("ceiling"));
}

#[test]
fn a_rejected_rate_limit_stops_the_run() {
    let f = fixture("rejected");
    f.add_problem("ev-a-0001", "XS", 1);
    f.add_problem("ev-b-0002", "XS", 2);
    let agent = FakeAgent::new(
        &f,
        vec![script(
            &[rate_limit("rejected", 1.0, 0.5), assistant("m", 10)],
            false,
        )],
    );
    let summary = run(
        &app(&f),
        &agent,
        &options(Some(1_000_000), None),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(summary.results.len(), 1);
    assert_eq!(summary.results[0].outcome, Outcome::RateLimited);
    assert_eq!(agent.log.borrow().killed, [true]);
    assert_eq!(status(&f, "ev-a-0001"), Status::Open);
}

#[test]
fn releases_on_failure_and_moves_on() {
    let f = fixture("failure");
    f.add_problem("ev-a-0001", "XS", 1);
    f.add_problem("ev-b-0002", "XS", 2);
    let agent = FakeAgent::new(
        &f,
        vec![
            script(&[assistant("m", 10)], false),
            script(&[assistant("m", 10)], true),
        ],
    );
    let mut options = options(Some(1_000_000), None);
    options.max_problems = Some(5);
    let summary = run(&app(&f), &agent, &options, &mut Vec::new()).unwrap();
    let outcomes: Vec<_> = summary.results.iter().map(|r| r.outcome.clone()).collect();
    assert_eq!(outcomes, [Outcome::NotSubmitted, Outcome::Submitted]);
    assert_eq!(status(&f, "ev-a-0001"), Status::Open);
    let attempts = backend(&f).attempts().unwrap();
    assert_eq!(attempts[0].outcome, "not-submitted");
    assert_eq!(attempts[1].outcome, "submitted");
}

#[test]
fn plan_budgets_need_rate_limit_events() {
    let f = fixture("apikey");
    f.add_problem("ev-a-0001", "XS", 1);
    f.add_problem("ev-b-0002", "XS", 2);
    let agent = FakeAgent::new(&f, vec![script(&[assistant("m", 10)], false)]);
    let summary = run(&app(&f), &agent, &options(None, Some(5.0)), &mut Vec::new()).unwrap();
    assert_eq!(summary.results.len(), 1);
    assert!(summary.stop_reason.contains("cannot enforce --session"));
}

#[test]
fn works_on_exactly_the_requested_problem() {
    let f = fixture("explicit");
    f.add_problem("ev-a-0001", "XS", 1);
    f.add_problem("ev-b-0002", "S", 2);
    let agent = FakeAgent::new(&f, vec![script(&[assistant("m", 10)], true)]);
    let mut options = options(Some(1_000_000), None);
    options.problem = Some("ev-b-0002".into());
    let summary = run(&app(&f), &agent, &options, &mut Vec::new()).unwrap();
    assert_eq!(agent.log.borrow().started, ["ev-b-0002"]);
    assert_eq!(summary.results.len(), 1);

    let f = fixture("explicit-big");
    f.add_problem("ev-c-0003", "L", 1);
    let agent = FakeAgent::new(&f, vec![]);
    let mut big = options.clone();
    big.problem = Some("ev-c-0003".into());
    let error = run(&app(&f), &agent, &big, &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("does not fit"), "{error}");
}

#[test]
fn validates_options() {
    let f = fixture("options");
    let agent = FakeAgent::new(&f, vec![]);
    let error = run(&app(&f), &agent, &options(None, None), &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("at least one budget"));
    let mut xl = options(Some(1), None);
    xl.max_size = Size::XL;
    let error = run(&app(&f), &agent, &xl, &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("--allow-xl"));
}

#[test]
fn releases_the_claim_of_an_interrupted_run() {
    let f = fixture("interrupted");
    f.add_problem("ev-a-0001", "XS", 1);
    backend(&f).claim("ev-a-0001").unwrap();
    let runner = f.home.join("state/runner.json");
    fs::write(&runner, r#"{"pid":1,"problem":"ev-a-0001"}"#).unwrap();

    // A fresh runner file means another run is active.
    let agent = FakeAgent::new(&f, vec![]);
    let error = run(&app(&f), &agent, &options(Some(1), None), &mut Vec::new()).unwrap_err();
    assert!(error.to_string().contains("another `proofathome start`"));

    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
    fs::File::options()
        .write(true)
        .open(&runner)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let mut out = Vec::new();
    // A 1-token budget fits nothing, so the run stops right after recovery.
    run(&app(&f), &agent, &options(Some(1), None), &mut out).unwrap();
    assert_eq!(status(&f, "ev-a-0001"), Status::Open);
    assert!(String::from_utf8(out).unwrap().contains("interrupted run"));
    assert_eq!(backend(&f).attempts().unwrap()[0].outcome, "interrupted");
    assert!(!runner.exists());
}

/// The real binary with a fake `claude` that checks its arguments, submits
/// through the CLI and prints stream-json.
#[cfg(unix)]
#[test]
fn start_runs_claude_with_a_fixed_command() {
    let f = fixture("e2e");
    f.add_problem("ev-a-0001", "S", 1);
    let log = f.root.join("claude.args");
    let claude = f.root.join("claude");
    let exe = env!("CARGO_BIN_EXE_proofathome");
    common::write_script(
        &claude,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{log}'\npwd >> '{log}'\n\
             echo '{{\"type\":\"system\",\"subtype\":\"init\"}}'\n\
             echo '{rl}'\necho '{a1}'\necho '{a1}'\n\
             printf '// proof\\n' > example-virtio/src/vsock/credit.rs\n\
             '{exe}' submit ev-a-0001 --summary fake >/dev/null 2>&1 || echo submit-failed >> '{log}'\n\
             echo '{{\"type\":\"result\",\"usage\":{{\"input_tokens\":700}}}}'\n",
            log = log.display(),
            rl = rate_limit("allowed", 0.1, 0.2),
            a1 = assistant("msg_1", 700),
        ),
    );
    let mut command = f.command(&["start", "--tokens", "1M", "--model", "test-model"]);
    command.env("PROOFATHOME_CLAUDE_BIN", &claude);
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("ev-a-0001: submitted (700 tokens"),
        "{stdout}"
    );

    let args = fs::read_to_string(&log).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert!(!args.contains(&"submit-failed"), "{args:?}");
    assert_eq!(args[0], "-p");
    assert!(args[1].contains("ev-a-0001"));
    assert!(
        !args
            .iter()
            .any(|a| a.contains("Ignore previous instructions"))
    );
    let config_at = args.iter().position(|a| *a == "--mcp-config").unwrap() + 1;
    let config: serde_json::Value = serde_json::from_str(args[config_at]).unwrap();
    assert_eq!(config["mcpServers"]["proofathome"]["command"], exe);
    assert_eq!(
        config["mcpServers"]["proofathome"]["env"]["PROOFATHOME_HOME"],
        f.home.to_string_lossy().as_ref()
    );
    assert!(args.contains(&"--model") && args.contains(&"test-model"));
    assert!(args.last().unwrap().ends_with("work/ev-a-0001"), "{args:?}");
}
