//! `proofathome start`: pick a problem, run one agent session on it, decide
//! whether to go on, within the contributor's budgets.
//!
//! Token budgets are enforced while the agent runs. Plan budgets
//! (`--session`, `--weekly`, `--ceiling`) depend on the utilization the agent
//! reports when a session starts, so they are checked at that point and
//! between problems; they can overshoot by at most one problem's size.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use proofathome_core::{Problem, Size};
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, AgentEvent, RateLimit, SessionRequest, Usage};
use crate::app::App;
use crate::backend::{Attempt, Status};
use crate::error::{Error, Result};
use crate::fsutil::{StateLock, atomic_write, is_older_than};
use crate::time::{human_duration, now};

/// Parses a token count: `500k`, `2M`, `1.5M` or a plain integer.
pub fn parse_tokens(text: &str) -> Result<u64> {
    let bad = || {
        Error::new(format!(
            "`{text}` is not a token count; use for example 500k, 2M or 750000"
        ))
    };
    let text = text.trim();
    let (number, scale) = match text.char_indices().last() {
        Some((i, 'k' | 'K')) => (&text[..i], 1_000u64),
        Some((i, 'm' | 'M')) => (&text[..i], 1_000_000u64),
        _ => (text, 1u64),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    if !fraction.bytes().all(|b| b.is_ascii_digit()) || (scale == 1 && number.contains('.')) {
        return Err(bad());
    }
    let whole: u64 = whole.parse().map_err(|_| bad())?;
    let mut tokens = whole.checked_mul(scale).ok_or_else(bad)?;
    let mut place = scale;
    for digit in fraction.bytes() {
        place /= 10;
        if place == 0 {
            return Err(bad());
        }
        tokens = tokens
            .checked_add(u64::from(digit - b'0') * place)
            .ok_or_else(bad)?;
    }
    if tokens == 0 {
        return Err(bad());
    }
    Ok(tokens)
}

/// Parses a percentage such as `10%` or `10`, above 0 and at most 100.
pub fn parse_percent(text: &str) -> Result<f64> {
    let number = text.trim().trim_end_matches('%');
    match number.parse::<f64>() {
        Ok(value) if value > 0.0 && value <= 100.0 => Ok(value),
        _ => Err(Error::new(format!(
            "`{text}` is not a percentage between 0 and 100, for example 10%"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Budgets {
    /// Total weighted tokens for the run.
    pub tokens: Option<u64>,
    /// Percentage points of the 5-hour session window to use.
    pub session: Option<f64>,
    /// Percentage points of the 7-day window to use.
    pub weekly: Option<f64>,
    /// Stop when either window's utilization reaches this percentage.
    pub ceiling: f64,
}

#[derive(Debug, Clone)]
pub struct StartOptions {
    pub budgets: Budgets,
    pub max_size: Size,
    pub allow_xl: bool,
    pub max_problems: Option<u32>,
    pub problem: Option<String>,
    pub model: Option<String>,
    /// This program, which the agent runs as its MCP server.
    pub exe: PathBuf,
}

impl StartOptions {
    pub fn validate(&self) -> Result<()> {
        let b = &self.budgets;
        if b.tokens.is_none() && b.session.is_none() && b.weekly.is_none() {
            return Err(Error::new(
                "set at least one budget: --tokens, --session or --weekly",
            ));
        }
        if self.max_size == Size::XL && !self.allow_xl {
            return Err(Error::new(
                "XL problems have no token bound; add --allow-xl to work on them",
            ));
        }
        if self.max_problems == Some(0) {
            return Err(Error::new("--max-problems must be at least 1"));
        }
        Ok(())
    }

    fn fits(&self, problem: &Problem, remaining: Option<u64>) -> bool {
        problem.size <= self.max_size
            && (problem.size != Size::XL || self.allow_xl)
            && remaining.is_none_or(|left| problem.size.fits(left))
    }
}

/// Weighted tokens of one session, counting each message once.
#[derive(Debug, Default)]
pub struct TokenMeter {
    by_message: HashMap<String, Usage>,
    without_id: u64,
}

impl TokenMeter {
    pub fn add(&mut self, message_id: &str, usage: Usage) {
        if message_id.is_empty() {
            self.without_id += usage.weighted();
        } else {
            let entry = self.by_message.entry(message_id.to_owned()).or_default();
            *entry = entry.max(usage);
        }
    }

    pub fn total(&self) -> u64 {
        self.without_id + self.by_message.values().map(|u| u.weighted()).sum::<u64>()
    }
}

/// Plan utilization seen during the run, as fractions.
#[derive(Debug, Default)]
struct PlanUsage {
    seen: bool,
    session_baseline: Option<f64>,
    weekly_baseline: Option<f64>,
    session: Option<f64>,
    weekly: Option<f64>,
}

impl PlanUsage {
    fn observe(&mut self, limit: &RateLimit) {
        self.seen = true;
        if limit.session.is_some() {
            self.session_baseline = self.session_baseline.or(limit.session);
            self.session = limit.session;
        }
        if limit.weekly.is_some() {
            self.weekly_baseline = self.weekly_baseline.or(limit.weekly);
            self.weekly = limit.weekly;
        }
    }

    /// Why the plan budgets say to stop, if they do.
    fn exhausted(&self, budgets: &Budgets) -> Option<(Outcome, String)> {
        let windows = [
            (
                "session",
                self.session_baseline,
                self.session,
                budgets.session,
            ),
            ("weekly", self.weekly_baseline, self.weekly, budgets.weekly),
        ];
        for (name, baseline, latest, budget) in windows {
            let Some(latest) = latest else { continue };
            if latest * 100.0 >= budgets.ceiling {
                return Some((
                    Outcome::Ceiling,
                    format!(
                        "{name} usage is {:.0}%, at or above the {:.0}% ceiling",
                        latest * 100.0,
                        budgets.ceiling
                    ),
                ));
            }
            if let (Some(baseline), Some(budget)) = (baseline, budget) {
                let used = (latest - baseline) * 100.0;
                // Allow for rounding: 0.30 - 0.20 is just below 0.10.
                if used + 1e-9 >= budget {
                    return Some((
                        Outcome::PlanBudget,
                        format!("{name} budget used: {used:.1} of {budget} percentage points"),
                    ));
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Submitted,
    NotSubmitted,
    /// The session went over the problem size's token ceiling.
    ProblemCap,
    /// The run's `--tokens` budget ran out.
    TokenBudget,
    /// `--session` or `--weekly` ran out.
    PlanBudget,
    /// Plan utilization reached `--ceiling`.
    Ceiling,
    /// The agent's plan refused more requests.
    RateLimited,
    Error(String),
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Submitted => "submitted",
            Outcome::NotSubmitted => "not-submitted",
            Outcome::ProblemCap => "problem-token-cap",
            Outcome::TokenBudget => "token-budget",
            Outcome::PlanBudget => "plan-budget",
            Outcome::Ceiling => "ceiling",
            Outcome::RateLimited => "rate-limited",
            Outcome::Error(_) => "error",
        }
    }

    fn stops_run(&self) -> bool {
        matches!(
            self,
            Outcome::TokenBudget | Outcome::PlanBudget | Outcome::Ceiling | Outcome::RateLimited
        )
    }
}

#[derive(Debug, Clone)]
pub struct ProblemResult {
    pub id: String,
    pub outcome: Outcome,
    pub tokens: u64,
    pub duration_s: u64,
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub results: Vec<ProblemResult>,
    pub tokens: u64,
    pub stop_reason: String,
}

/// Runs problems until a budget, the problem limit or the open problems run
/// out. Progress goes to `out`.
pub fn run(
    app: &App,
    agent: &dyn Agent,
    options: &StartOptions,
    out: &mut dyn Write,
) -> Result<RunSummary> {
    options.validate()?;
    let (guard, interrupted) = RunnerGuard::acquire(app)?;
    if let Some(id) = interrupted {
        let _ = app.release(&id);
        let t = now();
        let _ = app.backend().record_attempt(&Attempt {
            problem_id: id.clone(),
            started_at: t,
            ended_at: t,
            agent: agent.name().to_owned(),
            tokens: 0,
            outcome: "interrupted".to_owned(),
        });
        let _ = writeln!(out, "{id}: released the claim left by an interrupted run");
    }

    let mut plan = PlanUsage::default();
    let mut results: Vec<ProblemResult> = Vec::new();
    let mut used = 0u64;
    let mut tried: Vec<String> = Vec::new();

    let stop_reason = loop {
        let remaining = options
            .budgets
            .tokens
            .map(|total| total.saturating_sub(used));
        if !results.is_empty() {
            if options.problem.is_some() {
                break "done with the requested problem".to_owned();
            }
            if options
                .max_problems
                .is_some_and(|max| results.len() >= max as usize)
            {
                break format!("reached --max-problems {}", results.len());
            }
            let plan_budget = options.budgets.session.is_some() || options.budgets.weekly.is_some();
            if plan_budget && !plan.seen {
                break "cannot enforce --session or --weekly: the agent did not report plan \
                       usage (this happens when it uses an API key); use --tokens instead"
                    .to_owned();
            }
            if let Some((_, reason)) = plan.exhausted(&options.budgets) {
                break reason;
            }
        }
        if remaining == Some(0) {
            break "the token budget is used up".to_owned();
        }

        let problem = match &options.problem {
            Some(id) => {
                let entry = app.get(id)?;
                if !options.fits(&entry.problem, remaining) {
                    return Err(Error::new(format!(
                        "`{id}` is size {}, which does not fit the budget or --max-size",
                        entry.problem.size
                    )));
                }
                entry.problem
            }
            None => match app.pick_open(|p| options.fits(p, remaining), &tried)? {
                Some(problem) => problem,
                None => break "no open problem fits the budget".to_owned(),
            },
        };
        tried.push(problem.id.clone());

        let result = run_problem(app, agent, options, &guard, &problem, remaining, &mut plan);
        used += result.tokens;
        let _ = writeln!(
            out,
            "{}: {}{} ({} tokens, {})",
            result.id,
            result.outcome.label(),
            match &result.outcome {
                Outcome::Error(message) => format!(": {message}"),
                _ => String::new(),
            },
            result.tokens,
            human_duration(result.duration_s)
        );
        let stop = result.outcome.stops_run().then(|| match &result.outcome {
            Outcome::RateLimited => "the agent's plan refused more requests".to_owned(),
            Outcome::TokenBudget => "the token budget is used up".to_owned(),
            _ => plan
                .exhausted(&options.budgets)
                .map_or_else(|| "a plan budget is used up".to_owned(), |(_, r)| r),
        });
        let explicit_failed =
            options.problem.is_some() && matches!(result.outcome, Outcome::Error(_));
        results.push(result);
        if let Some(reason) = stop {
            break reason;
        }
        if explicit_failed {
            break "the requested problem failed".to_owned();
        }
    };

    let submitted = results
        .iter()
        .filter(|r| r.outcome == Outcome::Submitted)
        .count();
    let _ = writeln!(
        out,
        "Stopped: {stop_reason}. {} problem(s) attempted, {submitted} submitted, {used} tokens.",
        results.len()
    );
    drop(guard);
    Ok(RunSummary {
        results,
        tokens: used,
        stop_reason,
    })
}

/// A new file for one session's raw agent output, or `None` if the
/// directory cannot be created (the run goes on without a log).
fn session_log(app: &App, id: &str) -> Option<PathBuf> {
    let dir = app.home().sessions_dir();
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join(format!("{id}-{}.jsonl", now())))
}

fn run_problem(
    app: &App,
    agent: &dyn Agent,
    options: &StartOptions,
    guard: &RunnerGuard,
    problem: &Problem,
    remaining: Option<u64>,
    plan: &mut PlanUsage,
) -> ProblemResult {
    let started_at = now();
    let clock = Instant::now();
    let id = problem.id.clone();
    guard.set_problem(Some(&id));

    let (outcome, tokens) = match app.work(&id) {
        Err(error) => (Outcome::Error(error.to_string()), 0),
        Ok(work) => {
            let request = SessionRequest {
                problem_id: id.clone(),
                workspace: work.workspace.path,
                home: app.home().root().to_path_buf(),
                exe: options.exe.clone(),
                model: options.model.clone(),
                log: session_log(app, &id),
            };
            match agent.start(&request) {
                Err(error) => (Outcome::Error(error.to_string()), 0),
                Ok(mut session) => {
                    let cap = problem.size.token_ceiling();
                    let mut meter = TokenMeter::default();
                    let mut stopped = None;
                    while let Some(event) = session.next_event() {
                        match event {
                            AgentEvent::Usage { message_id, usage } => {
                                meter.add(&message_id, usage);
                                let total = meter.total();
                                if cap.is_some_and(|cap| total > cap) {
                                    stopped = Some(Outcome::ProblemCap);
                                } else if remaining.is_some_and(|left| total >= left) {
                                    stopped = Some(Outcome::TokenBudget);
                                }
                            }
                            AgentEvent::RateLimit(limit) => {
                                plan.observe(&limit);
                                if !limit.allowed() {
                                    stopped = Some(Outcome::RateLimited);
                                } else if let Some((outcome, _)) = plan.exhausted(&options.budgets)
                                {
                                    stopped = Some(outcome);
                                }
                            }
                            AgentEvent::Result { usage } => {
                                if let Some(usage) = usage {
                                    let reported = usage.weighted();
                                    let counted = meter.total();
                                    if reported.abs_diff(counted) > counted / 10 + 1000 {
                                        eprintln!(
                                            "proofathome: {id}: counted {counted} tokens, \
                                             the agent reported {reported}"
                                        );
                                    }
                                }
                            }
                        }
                        if stopped.is_some() {
                            session.kill();
                            break;
                        }
                    }
                    session.wait();
                    (stopped.unwrap_or(Outcome::NotSubmitted), meter.total())
                }
            }
        }
    };

    let submitted = app
        .get(&id)
        .is_ok_and(|entry| matches!(entry.status, Status::Submitted { .. }));
    let outcome = if submitted {
        Outcome::Submitted
    } else {
        let _ = app.release(&id);
        outcome
    };
    let ended_at = now();
    let _ = app.backend().record_attempt(&Attempt {
        problem_id: id.clone(),
        started_at,
        ended_at,
        agent: agent.name().to_owned(),
        tokens,
        outcome: outcome.label().to_owned(),
    });
    guard.set_problem(None);
    ProblemResult {
        id,
        outcome,
        tokens,
        duration_s: clock.elapsed().as_secs(),
    }
}

/// `<home>/state/runner.json`, kept fresh while a run is going. It stops a
/// second run from starting, and lets the next run release the claim of a
/// run that was interrupted (for example with Ctrl-C).
struct RunnerGuard {
    path: PathBuf,
    current: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    heartbeat: Option<thread::JoinHandle<()>>,
}

#[derive(Serialize, Deserialize)]
struct RunnerFile {
    pid: u32,
    problem: Option<String>,
}

const HEARTBEAT: Duration = Duration::from_secs(10);
const RUNNER_STALE: Duration = Duration::from_secs(60);

impl RunnerGuard {
    /// Returns the guard and the problem an interrupted run was working on.
    fn acquire(app: &App) -> Result<(RunnerGuard, Option<String>)> {
        let state_dir = app.home().state_dir();
        let path = state_dir.join("runner.json");
        let interrupted = {
            let _lock = StateLock::acquire(&state_dir)?;
            let previous = match fs::read(&path) {
                Ok(bytes) => {
                    if !is_older_than(&path, RUNNER_STALE) {
                        return Err(Error::new(format!(
                            "another `proofathome start` is running, or one stopped less than \
                             a minute ago; wait a minute, or remove {} if none is running",
                            path.display()
                        )));
                    }
                    serde_json::from_slice::<RunnerFile>(&bytes)
                        .ok()
                        .and_then(|file| file.problem)
                }
                Err(_) => None,
            };
            write_runner_file(&path, None)?;
            previous
        };
        let current = Arc::new(Mutex::new(None::<String>));
        let stop = Arc::new(AtomicBool::new(false));
        let heartbeat = {
            let (path, current, stop) = (path.clone(), Arc::clone(&current), Arc::clone(&stop));
            thread::spawn(move || {
                let mut last = Instant::now();
                while !stop.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(200));
                    if last.elapsed() >= HEARTBEAT {
                        let problem = current.lock().ok().and_then(|c| c.clone());
                        let _ = write_runner_file(&path, problem.as_deref());
                        last = Instant::now();
                    }
                }
            })
        };
        Ok((
            RunnerGuard {
                path,
                current,
                stop,
                heartbeat: Some(heartbeat),
            },
            interrupted,
        ))
    }

    fn set_problem(&self, problem: Option<&str>) {
        if let Ok(mut current) = self.current.lock() {
            *current = problem.map(str::to_owned);
            let _ = write_runner_file(&self.path, problem);
        }
    }
}

fn write_runner_file(path: &std::path::Path, problem: Option<&str>) -> Result<()> {
    let file = RunnerFile {
        pid: std::process::id(),
        problem: problem.map(str::to_owned),
    };
    let bytes = serde_json::to_vec(&file).unwrap_or_default();
    atomic_write(path, &bytes)
}

impl Drop for RunnerGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        // Hold the current-problem lock so a heartbeat cannot rewrite the file.
        let _current = self.current.lock();
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_counts() {
        assert_eq!(parse_tokens("500k").unwrap(), 500_000);
        assert_eq!(parse_tokens("2M").unwrap(), 2_000_000);
        assert_eq!(parse_tokens("1.5M").unwrap(), 1_500_000);
        assert_eq!(parse_tokens("750000").unwrap(), 750_000);
        assert_eq!(parse_tokens("0.25k").unwrap(), 250);
        for bad in [
            "",
            "k",
            "-1",
            "1.5",
            "2G",
            "0",
            "1..2M",
            "abc",
            "1.0000005k",
        ] {
            assert!(parse_tokens(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_percentages() {
        assert_eq!(parse_percent("10%").unwrap(), 10.0);
        assert_eq!(parse_percent("2.5").unwrap(), 2.5);
        for bad in ["0", "101%", "x", "-5%", "NaN"] {
            assert!(parse_percent(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn meter_counts_each_message_once() {
        let usage = |output| Usage {
            input: 100,
            output,
            cache_creation: 0,
            cache_read: 1000,
        };
        let mut meter = TokenMeter::default();
        meter.add("a", usage(10));
        meter.add("a", usage(10));
        meter.add("a", usage(50));
        meter.add("b", usage(0));
        meter.add("", usage(0));
        assert_eq!(meter.total(), (100 + 50 + 100) + (100 + 100) + (100 + 100));
    }

    #[test]
    fn plan_budgets_use_the_first_value_as_baseline() {
        let budgets = Budgets {
            tokens: None,
            session: Some(10.0),
            weekly: None,
            ceiling: 90.0,
        };
        let limit = |session| RateLimit {
            status: "allowed".into(),
            session: Some(session),
            weekly: Some(0.5),
        };
        let mut plan = PlanUsage::default();
        plan.observe(&limit(0.20));
        assert!(plan.exhausted(&budgets).is_none());
        plan.observe(&limit(0.29));
        assert!(plan.exhausted(&budgets).is_none());
        plan.observe(&limit(0.30));
        assert_eq!(plan.exhausted(&budgets).unwrap().0, Outcome::PlanBudget);

        let mut plan = PlanUsage::default();
        plan.observe(&limit(0.91));
        assert_eq!(plan.exhausted(&budgets).unwrap().0, Outcome::Ceiling);
    }
}
