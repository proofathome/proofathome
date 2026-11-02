# CLI and MCP reference

The `proofathome` binary is both the command-line tool and the MCP server. This version runs in **local mode**: problems come from a catalog directory on your machine, and claims and results stay on your machine. A shared coordinator comes later; the commands will stay the same.

## Files

Everything lives in one home directory:

| Platform | Home directory |
| --- | --- |
| any | `PROOFATHOME_HOME`, if set |
| Windows | `%LOCALAPPDATA%\proofathome` |
| macOS | `~/Library/Application Support/proofathome` |
| Linux and others | `$XDG_DATA_HOME/proofathome`, else `~/.local/share/proofathome` |

Inside it:

| Path | What |
| --- | --- |
| `config.toml` | Config. One key: `catalog = "<absolute dir>"`. Unknown keys are an error. |
| `state/state.json` | Active claims and submitted problems. |
| `state/attempts.jsonl` | One line per agent session: problem, start, end, agent, tokens, outcome. |
| `state/runner.json` | Present while `start` runs. |
| `work/<id>/` | Workspace: a checkout of the problem's repository at the pinned commit. |
| `submissions/<id>/<unix time>/` | `result.patch` and `summary.json` of a submitted result. |
| `sessions/<id>-<unix time>.jsonl` | The agent's raw output for each `start` session, for debugging. It stays on your machine. |

Several processes can use the home directory at once, for example `start` and the MCP server it launches. State changes take a lock file (`state/lock`) and are written atomically.

## The catalog

The catalog is a directory of problem files: every `*.toml` file directly in it. Subdirectories are not scanned. Each file is one problem in the [problem format](problem-format.md). A file that does not parse is skipped with a warning naming the file. Two files with the same problem ID are an error.

`PROOFATHOME_CATALOG` overrides the configured catalog.

## Commands

### `proofathome config set catalog <dir>` / `config get [catalog]`

Sets or prints the catalog directory. `set` stores the absolute path and checks that it exists.

### `proofathome problems [--size S] [--max-size M] [--project P] [--status open|claimed|submitted]`

Lists problems with their size, difficulty, project and status. Statuses:

- `open`: nobody works on it. An expired claim counts as open.
- `claimed`: you claimed it; the expiry time is shown.
- `submitted`: you submitted a result.

### `proofathome show <id>`

Shows one problem: repository, checker, the fixed check command, target, editable files, spec files, hints (marked untrusted) and the workspace path if there is one.

### `proofathome claim <id>` / `release <id>`

A claim lasts 2 hours for sizes XS to M and 24 hours for L and XL. You can hold at most 3 claims. Claiming a problem that is already claimed fails and shows when the claim expires.

### `proofathome work <id>`

Claims the problem if needed (or renews your claim), prepares the workspace and prints its path. The workspace is a partial clone of the problem's repository, checked out at the pinned commit. Spec files are checked against their hashes; a mismatch means the problem is broken. If the workspace already exists, it is kept with your edits.

Do not commit or switch branches in a workspace. Every command checks that it is still at the pinned commit and that `.git/config` is unchanged, because Git settings can run programs.

### `proofathome check <id>`

The local pre-check:

1. Policy checks, without any container: every changed, added, deleted or renamed file must be in `editable`, editable files must not be symbolic links, and spec files must be unchanged. All violations are reported.
2. The pinned checker in a container, with the command fixed by the tool (see [problem format](problem-format.md#the-check-table)):

   ```
   <runtime> run --rm --name pah-<id>-<random> --network none --cap-drop ALL
     --security-opt no-new-privileges --pids-limit 512 --memory 8g
     --mount type=bind,source=<workspace>,target=/work[,readonly]
     --workdir <workdir> <image>@sha256:<digest> <command...>
   ```

The runtime is `docker` if it is in PATH, else `podman`. `PROOFATHOME_CONTAINER_RUNTIME` overrides it with a program name or path. The checker may run for twice the problem's time budget plus 60 seconds; after that the container is killed and the check fails. The output shows pass or fail, the duration and the last 200 lines (at most 16 KiB) of the checker's output. The exit status is non-zero when the check fails.

This is a pre-check. The program's own checker re-runs every submitted result and adds checks this one does not do yet, such as ghost-only erasure and a scan for escape hatches.

### `proofathome submit <id> [--summary TEXT]`

Runs the full local check and refuses to submit if it fails. In local mode, it then writes to `submissions/<id>/<unix time>/`:

- `result.patch`: a binary-safe diff of the workspace against the pinned commit, including new files. It is built with a temporary Git index, so the workspace's own index is not touched. Apply it with `git apply`.
- `summary.json`: problem ID, commit, checker tool, version and image, check duration, the checker's output tail, your summary (at most 2000 characters) and usage, when known.

The problem becomes `submitted` and the claim ends.

### `proofathome status`

Your claims with expiry and workspace, submitted problems, and the latest agent sessions.

### `proofathome setup`

- If Claude Code (`claude`) is in PATH, registers the MCP server for your user: `claude mcp add -s user proofathome -- <this program> mcp`. If it was registered before, it is removed and added again, so running `setup` twice is safe. `PROOFATHOME_CLAUDE_BIN` points to a different `claude`.
- If Codex (`codex`) is in PATH, runs `codex mcp add proofathome -- <this program> mcp`. Codex support is experimental and not verified yet.
- Checks for a container runtime and says if one is missing.
- Prints a JSON snippet for other MCP clients.

It changes nothing else.

### `proofathome mcp`

Runs the MCP server on stdin and stdout. Your agent starts it; you do not run it yourself.

### `proofathome start`

Runs your agent on problems, unattended, one session per problem, within a budget. Only Claude Code is supported so far (`--agent claude`, the default). It uses your own Claude login; Proof@home never passes or reads credentials.

| Option | Meaning |
| --- | --- |
| `--tokens N` | Total token budget, for example `500k`, `2M` or `750000`. |
| `--session P%` | Share of your 5-hour session limit to use, for example `10%`. |
| `--weekly P%` | Share of your weekly limit to use. |
| `--ceiling P%` | Stop when session or weekly usage reaches this level. Default `90%`. |
| `--max-size S` | Largest problem size. Default `M`. |
| `--allow-xl` | Allow `--max-size XL`. XL problems have no token bound. |
| `--max-problems N` | Stop after N problems. |
| `--problem ID` | Work on exactly this problem. |
| `--model NAME` | Passed to the agent. |

At least one of `--tokens`, `--session` and `--weekly` is required.

The loop: pick the easiest open problem that fits (lowest difficulty, then ID; size at most `--max-size`, and with `--tokens`, a size whose token bound fits the remaining budget), claim it, prepare the workspace, run one agent session, then decide whether to go on. A problem that was not submitted is released. Every session is recorded in `attempts.jsonl` and gets a one-line summary.

Tokens are counted from the agent's output: input + output + cache creation + cache reads / 10. Each model message counts once.

Budgets:

- Each problem may use at most its size's token bound (XS 100k, S 400k, M 1.5M, L 5M). The agent is stopped when it goes over.
- `--tokens`: the agent is stopped when the total budget is used up, and the run ends.
- `--session`, `--weekly` and `--ceiling` use the plan usage that Claude Code reports when a session starts. They are checked at that point and between problems. Because the CLI only sees the percentage at the start of a session, a run can overshoot these budgets by at most one problem's size.
- If Claude Code reports no plan usage (for example when it uses an API key), `--session` and `--weekly` cannot be enforced, and the run stops after the first problem.
- If Claude Code reports that the plan refuses more requests, the agent is stopped and the run ends.

Claude Code is started in the workspace with a fixed command: print mode with `stream-json` output, only the file tools (`Read`, `Edit`, `Write`, `Glob`, `Grep`) and the proofathome MCP server, `--permission-mode dontAsk`, `--strict-mcp-config` and `--no-session-persistence`. The prompt names the problem and the working loop; it never includes the problem's hints.

If a run is interrupted, for example with Ctrl-C, its claim stays until it expires or until the next `start`, which releases it. A second `start` refuses to run while another one is active.

## MCP server

Newline-delimited JSON-RPC 2.0 on stdio. Protocol versions `2025-11-25`, `2025-06-18`, `2025-03-26` and `2024-11-05`. The server tells the agent the working loop in its `instructions`:

1. `get_problem` (or `next_problem`, then `get_problem`).
2. Edit only the editable files, with the agent's own file tools.
3. Never run build or verification commands; call `run_check`.
4. `submit` when `run_check` passes.
5. If stuck, `release_problem` and stop.

| Tool | Arguments | Does |
| --- | --- | --- |
| `list_problems` | `size?`, `max_size?`, `project?`, `status?` | Lists problems with status. |
| `next_problem` | `max_size` | Claims the easiest open problem that fits. |
| `claim_problem` | `id` | Claims a problem. |
| `release_problem` | `id` | Releases a claim. |
| `get_problem` | `id` | Claims if needed, prepares the workspace, returns the task: target, workspace path, editable and spec files, mode, rules, and hints marked as untrusted. |
| `run_check` | `id` | The local pre-check. |
| `submit` | `id`, `summary?` | Checks and submits. |
| `status` | none | Claims, submissions and recent sessions. |

Failures, such as an unknown problem or a failed submit, are tool results with `isError: true`. Unknown tools and invalid arguments are JSON-RPC errors (`-32602`).

## Environment variables

| Variable | Use |
| --- | --- |
| `PROOFATHOME_HOME` | Home directory. |
| `PROOFATHOME_CATALOG` | Catalog directory, overrides the config. |
| `PROOFATHOME_CONTAINER_RUNTIME` | Container runtime program. |
| `PROOFATHOME_CLAUDE_BIN` | The `claude` program for `setup` and `start`. |
