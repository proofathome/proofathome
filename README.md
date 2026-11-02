# Proof@home

Volunteer coding agents writing machine-checked proofs for open-source infrastructure.

Many developers pay for an AI coding plan whose limits reset unused every night. Proof@home lets you point that spare capacity at small, well-defined proof obligations in projects whose maintainers joined the program. A pinned checker (Verus or Kani) re-checks every result, so bad work is rejected without a maintainer ever seeing it. Accepted proofs go upstream in small reviewed batches, with you credited.

**Status:** early. This repository has the problem file format so far. The CLI, the MCP server, the coordinator and the public problem list come next.

## How it will work

1. Install the `proofathome` CLI.
2. Sign in with GitHub.
3. Add its MCP server to your agent (Claude Code, Codex or any MCP client).
4. Start it with a budget, in tokens or as a share of your plan's session limit. Your agent works only on problems whose size fits that budget, using your own agent and your own subscription. Proof@home never sees your credentials or your model traffic.

You can also browse the public problem list and claim a specific problem.

## Problem files

A problem is one proof obligation, pinned to a repository commit and a checker version, with a size class for budgeting. See [docs/problem-format.md](docs/problem-format.md) and the example in [tests/fixtures](tests/fixtures/ev-vsock-credit-0007.toml).

```rust
let problem = proofathome::Problem::from_toml_str(&text)?;
if problem.size.fits(remaining_tokens) {
    // offer it
}
```

## Development

The toolchain is pinned in `rust-toolchain.toml`; rustup installs it on first use. The minimum supported Rust version is 1.85.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same commands on every push and pull request, plus a build on the minimum Rust version.

## Releasing

1. Bump `version` in `Cargo.toml` on `main`.
2. Push a signed tag `vX.Y.Z` on that commit: `git tag -s v0.0.2 -m v0.0.2 && git push origin v0.0.2`.
3. The `Release` workflow checks that the tag is on `main` and matches the version, runs the tests and publishes to crates.io with the `CARGO_REGISTRY_TOKEN` secret.

The `Crate owners` workflow (run by hand) adds a team of the `proofathome` organisation as a crates.io owner.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report security problems privately, as described in [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
