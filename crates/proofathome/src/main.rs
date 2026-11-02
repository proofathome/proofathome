use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use proofathome::app::{App, Filter, parse_size};
use proofathome::backend::StatusKind;
use proofathome::error::{Error, Result};
use proofathome::paths::{self, CATALOG_ENV, Config, Home};
use proofathome::proofathome_core::Size;
use proofathome::runner::{self, Budgets, StartOptions};
use proofathome::{agent, mcp, report, setup, time};

/// Proof@home: point your coding agent at small, machine-checked proofs for
/// open-source infrastructure.
#[derive(Parser)]
#[command(name = "proofathome", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List problems in the catalog.
    Problems {
        /// Only this size.
        #[arg(long, value_parser = size_arg)]
        size: Option<Size>,
        /// Only this size or smaller.
        #[arg(long, value_parser = size_arg)]
        max_size: Option<Size>,
        /// Only this project.
        #[arg(long)]
        project: Option<String>,
        /// Only problems with this status: open, claimed or submitted.
        #[arg(long, value_parser = status_arg)]
        status: Option<StatusKind>,
    },
    /// Show one problem.
    Show { id: String },
    /// Claim a problem.
    Claim { id: String },
    /// Give up a claimed problem.
    Release { id: String },
    /// Claim a problem if needed, prepare its workspace and print the path.
    Work { id: String },
    /// Run the local pre-check on a problem's workspace.
    Check { id: String },
    /// Check a problem's workspace and, if it passes, submit the result.
    Submit {
        id: String,
        /// A short description of the proof, at most 2000 characters.
        #[arg(long)]
        summary: Option<String>,
    },
    /// Show claims, submissions and recent agent sessions.
    Status,
    /// Read or change the config.
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Run the MCP server on stdin and stdout.
    Mcp,
    /// Register the MCP server with Claude Code and Codex, and check for a
    /// container runtime.
    Setup,
    /// Run your agent on problems, unattended, within a budget.
    Start(StartArgs),
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Set a value. The only key is `catalog`.
    Set { key: String, value: String },
    /// Print the config, or one value.
    Get { key: Option<String> },
}

#[derive(Clone, Copy, ValueEnum)]
enum AgentKind {
    Claude,
}

#[derive(Args)]
struct StartArgs {
    /// The agent to run.
    #[arg(long, value_enum, default_value = "claude")]
    agent: AgentKind,
    /// Total token budget, for example 500k or 2M.
    #[arg(long)]
    tokens: Option<String>,
    /// Share of the 5-hour session limit to use, for example 10%.
    #[arg(long)]
    session: Option<String>,
    /// Share of the weekly limit to use, for example 5%.
    #[arg(long)]
    weekly: Option<String>,
    /// Stop when session or weekly usage reaches this level.
    #[arg(long, default_value = "90%")]
    ceiling: String,
    /// Largest problem size to take.
    #[arg(long, value_parser = size_arg, default_value = "M")]
    max_size: Size,
    /// Allow XL problems, which have no token bound.
    #[arg(long)]
    allow_xl: bool,
    /// Stop after this many problems.
    #[arg(long)]
    max_problems: Option<u32>,
    /// Work on exactly this problem.
    #[arg(long)]
    problem: Option<String>,
    /// Model name, passed to the agent.
    #[arg(long)]
    model: Option<String>,
}

fn size_arg(text: &str) -> std::result::Result<Size, String> {
    parse_size(text).map_err(|e| e.to_string())
}

fn status_arg(text: &str) -> std::result::Result<StatusKind, String> {
    StatusKind::parse(text).map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs a command. `Ok(false)` is a failure that was already reported.
fn run(command: Command) -> Result<bool> {
    let mut out = io::stdout().lock();
    match command {
        Command::Problems {
            size,
            max_size,
            project,
            status,
        } => {
            let app = App::from_env()?;
            let listing = app.list(&Filter {
                size,
                max_size,
                project,
                status,
            })?;
            for warning in &listing.warnings {
                eprintln!("warning: {warning}");
            }
            if listing.problems.is_empty() {
                let _ = writeln!(out, "No problems match.");
            } else {
                let _ = writeln!(out, "{}", report::problem_header());
                for entry in &listing.problems {
                    let _ = writeln!(out, "{}", report::problem_row(entry));
                }
            }
        }
        Command::Show { id } => {
            let app = App::from_env()?;
            let entry = app.get(&id)?;
            let path = app.home().workspace(&id);
            let path = path.exists().then_some(path);
            let _ = write!(out, "{}", report::problem_details(&entry, path.as_deref()));
        }
        Command::Claim { id } => {
            let claim = App::from_env()?.claim(&id)?;
            let _ = writeln!(
                out,
                "Claimed {id} until {}.",
                time::rfc3339(claim.expires_at)
            );
        }
        Command::Release { id } => {
            App::from_env()?.release(&id)?;
            let _ = writeln!(out, "Released {id}.");
        }
        Command::Work { id } => {
            let work = App::from_env()?.work(&id)?;
            if work.workspace.resumed {
                eprintln!("Kept the existing workspace with your edits.");
            }
            eprintln!(
                "Claimed {id} until {}.",
                time::rfc3339(work.claim.expires_at)
            );
            let _ = writeln!(out, "{}", work.workspace.path.display());
        }
        Command::Check { id } => {
            let (_, check) = App::from_env()?.check(&id)?;
            let _ = write!(out, "{}", report::check_text(&check));
            return Ok(check.passed);
        }
        Command::Submit { id, summary } => {
            let result = App::from_env()?.submit(&id, summary.as_deref(), None)?;
            let _ = write!(out, "{}", report::check_text(&result.check));
            match result.receipt {
                Some(receipt) => {
                    let _ = writeln!(out, "\nSubmitted {id}: {}", receipt.location);
                }
                None => {
                    let _ = writeln!(out, "\nNot submitted: the check failed.");
                    return Ok(false);
                }
            }
        }
        Command::Status => {
            let app = App::from_env()?;
            let (text, _) = report::status_report(&app)?;
            let _ = write!(out, "{text}");
        }
        Command::Config(command) => config(command, &mut out)?,
        Command::Mcp => {
            drop(out);
            eprintln!(
                "proofathome {} MCP server on stdio",
                env!("CARGO_PKG_VERSION")
            );
            mcp::serve(&App::from_env, io::stdin().lock(), io::stdout().lock())
                .map_err(|e| Error::new(format!("MCP server stopped: {e}")))?;
        }
        Command::Setup => setup::run(&mut out)?,
        Command::Start(args) => return start(args, &mut out),
    }
    Ok(true)
}

fn config(command: ConfigCommand, out: &mut dyn Write) -> Result<()> {
    let home = Home::from_env()?;
    let check_key = |key: &str| {
        if key == "catalog" {
            Ok(())
        } else {
            Err(Error::new(format!(
                "unknown config key `{key}`; the only key is `catalog`"
            )))
        }
    };
    match command {
        ConfigCommand::Set { key, value } => {
            check_key(&key)?;
            let dir = paths::set_catalog(&home, &PathBuf::from(value))?;
            let _ = writeln!(out, "catalog = {}", dir.display());
        }
        ConfigCommand::Get { key } => {
            if let Some(key) = &key {
                check_key(key)?;
            }
            let config = Config::load(&home)?;
            match config.catalog {
                Some(catalog) => {
                    let _ = writeln!(out, "catalog = {catalog}");
                }
                None if key.is_some() => {
                    return Err(Error::new("`catalog` is not set"));
                }
                None => {}
            }
            if key.is_none() {
                let _ = writeln!(out, "# home: {}", home.root().display());
            }
            if let Some(value) = std::env::var_os(CATALOG_ENV).filter(|v| !v.is_empty()) {
                let _ = writeln!(
                    out,
                    "# {CATALOG_ENV} is set and overrides it: {}",
                    value.to_string_lossy()
                );
            }
        }
    }
    Ok(())
}

fn start(args: StartArgs, out: &mut dyn Write) -> Result<bool> {
    let budgets = Budgets {
        tokens: args
            .tokens
            .as_deref()
            .map(runner::parse_tokens)
            .transpose()?,
        session: args
            .session
            .as_deref()
            .map(runner::parse_percent)
            .transpose()?,
        weekly: args
            .weekly
            .as_deref()
            .map(runner::parse_percent)
            .transpose()?,
        ceiling: runner::parse_percent(&args.ceiling)?,
    };
    let options = StartOptions {
        budgets,
        max_size: args.max_size,
        allow_xl: args.allow_xl,
        max_problems: args.max_problems,
        problem: args.problem,
        model: args.model,
        exe: setup::current_exe()?,
    };
    options.validate()?;
    let agent = match args.agent {
        AgentKind::Claude => agent::ClaudeCode {
            program: setup::claude_program().ok_or_else(|| {
                Error::new(
                    "Claude Code was not found: install it and make sure `claude` is in PATH",
                )
            })?,
        },
    };
    let app = App::from_env()?;
    // Fail early, before any claim, if the container runtime is missing.
    proofathome::check::container_runtime()?;
    let summary = runner::run(&app, &agent, &options, out)?;
    Ok(summary
        .results
        .iter()
        .all(|r| !matches!(r.outcome, runner::Outcome::Error(_))))
}
