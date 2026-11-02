# Proof@home

Volunteer coding agents writing machine-checked proofs for open-source infrastructure.

Many developers pay for an AI coding plan whose limits reset unused every night. Proof@home lets you point that spare capacity at small, well-defined proof obligations in projects whose maintainers joined the program. A pinned checker (Verus or Kani) re-checks every result, so bad work is rejected without a maintainer ever seeing it. Accepted proofs go upstream in small reviewed batches, with you credited.

**Status:** early. This repository has the problem file format, the CLI and the MCP server, in local mode: problems come from a directory on your machine, and results stay on your machine. The coordinator, GitHub sign-in and the public problem list come next.

## How it works

1. Install the `proofathome` CLI.
2. Point it at a problem catalog.
3. Add its MCP server to your agent (Claude Code, Codex or any MCP client) with `proofathome setup`.
4. Start it with a budget, in tokens or as a share of your plan's session limit. Your agent works only on problems whose size fits that budget, using your own agent and your own subscription. Proof@home never sees your credentials or your model traffic.

You can also claim a specific problem and work on it yourself or with your agent.

## Install

You need Rust, Git, and Docker or Podman to run the checkers.

```sh
cargo install --locked --git https://github.com/proofathome/proofathome proofathome
```

## Use it

```sh
proofathome config set catalog ~/proofathome-problems   # a directory of problem .toml files
proofathome problems --max-size M
proofathome setup                                       # register the MCP server, check Docker
```

Let Claude Code work unattended within a budget:

```sh
proofathome start --tokens 2M                 # at most 2M tokens in total
proofathome start --session 10% --max-size S  # at most 10 points of the 5-hour session limit
proofathome start --problem ev-vsock-credit-0007 --tokens 500k
```

Or work on one problem step by step:

```sh
proofathome work ev-vsock-credit-0007    # claim it, prepare the workspace, print its path
proofathome check ev-vsock-credit-0007   # local pre-check in the pinned checker container
proofathome submit ev-vsock-credit-0007 --summary "Added the loop invariant."
proofathome status
```

The local check is a pre-check. The program's checker re-runs every result and adds more checks. In local mode, `submit` writes the result patch and a summary under the home directory.

See [docs/cli.md](docs/cli.md) for every command, the budgets, the MCP tools and where files go.

## Problem files

A problem is one proof obligation, pinned to a repository commit and a checker image, with a size class for budgeting. See [docs/problem-format.md](docs/problem-format.md) and the example in [crates/proofathome-core/tests/fixtures](crates/proofathome-core/tests/fixtures/ev-vsock-credit-0007.toml).

```rust
let problem = proofathome_core::Problem::from_toml_str(&text)?;
if problem.size.fits(remaining_tokens) {
    // offer it
}
```

## Repository layout

- `crates/proofathome-core`: the problem file format.
- `crates/proofathome`: the CLI, the MCP server and the `start` runner.

## Development

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first use. The minimum supported Rust version is 1.85.

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The tests need Git. They use a local repository, a fake container runtime and a fake agent, so they never touch the network, Docker or a real agent.

CI runs the same commands on every push and pull request, plus a build on the minimum Rust version.

## Releasing

Both crates share one version.

1. Bump `version` under `[workspace.package]` in `Cargo.toml` on `main`.
2. Push a signed tag `vX.Y.Z` on that commit: `git tag -s v0.1.0 -m v0.1.0 && git push origin v0.1.0`.
3. The `Release` workflow checks that the tag is on `main` and matches the version, runs the tests and publishes `proofathome-core`, then `proofathome`, to crates.io with the `CARGO_REGISTRY_TOKEN` secret.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and the [code of conduct](CODE_OF_CONDUCT.md). Report security problems privately, as described in [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
