# Checker images

Every Proof@home result is checked in a pinned container image. `proofathome check` runs these images locally, and the program's own checker runs the same ones.

| Image | Tool | Base |
| --- | --- | --- |
| `ghcr.io/proofathome/verus:<version>` | Verus `0.2026.10.26.d17fbd0`, with `verus` and `cargo verus` | `rust:1.99.1-slim-trixie` (Verus needs exactly Rust 1.99.1) |
| `ghcr.io/proofathome/kani:<version>` | Kani `0.68.0` (`cargo kani`), CBMC, and the nightly toolchain Kani installs | `rust:1.99.1-slim-trixie` |

Both are `linux/amd64` only.

## What is pinned

- Base images by digest, in the `FROM` lines.
- The Verus release zip and the Kani bundle by SHA-256, next to their URLs in the Dockerfiles. Both hashes match the digests GitHub publishes for the release assets.
- `kani-verifier` with `cargo install --locked --version`.
- The nightly Rust toolchain that `cargo kani setup` installs, by the Kani version.

Packages from Debian (`curl`, `unzip`) are only used to download, and the downloads are checked by hash.

## How the images run

There is no entrypoint: the CLI passes the full command, fixed by the tool (see [problem format](../docs/problem-format.md#the-check-table)).

- Any user ID works. Installed files are readable by everyone and never written at check time. `HOME=/tmp`, `CARGO_TARGET_DIR=/tmp/target`, and Kani lives in `KANI_HOME=/opt/kani`.
- The repository is mounted read-only at `/work`.
- Cargo dependencies come from a cache mounted at `/cargo-cache` with `CARGO_HOME=/cargo-cache`. A fetch step with network access fills it with `cargo fetch --locked`. The check runs with `--network none`, the cache read-only and `CARGO_NET_OFFLINE=true`.

A Verus Cargo check uses `cargo verus focus`: it verifies the crate itself and trusts its locked dependencies such as `vstd`, which contributors cannot change. Building `vstd` still takes about 50 seconds per check on top of the crate itself. Set `time_budget_s` to allow for it.

## Examples

`examples/` holds small crates that double as smoke tests:

| Directory | Command | Expected |
| --- | --- | --- |
| `verus-file` | `verus --crate-type=lib max.rs` | passes |
| `verus-pass` | `cargo verus focus --locked --offline` | passes |
| `verus-fail` | `cargo verus focus --locked --offline` | fails: postcondition not satisfied |
| `kani` | `cargo kani --harness proofs::never_smaller --exact` | passes |
| `kani` | `cargo kani --harness proofs::always_larger --exact` | fails |

Each Cargo example has its own `[workspace]` table and a committed `Cargo.lock`.

## Building and trying an image locally

```sh
docker build --platform linux/amd64 -t proofathome-verus:dev checkers/verus
docker image inspect proofathome-verus:dev --format '{{.Id}}'
```

With Docker's containerd image store (the default in current Docker Desktop), a problem file can name the local image as `proofathome-verus@sha256:<that id>`. With the classic store, push it to a local registry and use `localhost:5000/proofathome-verus@sha256:<digest>`.

## Publishing and updating

The [Checker images workflow](../.github/workflows/checkers.yml) runs on pushes to `main` that touch `checkers/`, and by hand. For each tool it builds the image, runs the examples the way `proofathome check` does, pushes `ghcr.io/proofathome/<tool>:<version>`, and prints the pushed digest in the job summary as a ready `checker = { ... }` line for problem files.

To move to a new version:

1. Change the version `ARG` in the Dockerfile and the matching SHA-256. Take the hash from the release asset's published digest (`gh api repos/<owner>/<repo>/releases/tags/<tag> --jq '.assets[] | .name + " " + .digest'`) and check it against a download.
2. For Verus, move the base image to the Rust version the release needs (`verus --version` shows it), and update `vstd` in the Verus examples and their `Cargo.lock`.
3. Build locally and run the examples.
4. Merge to `main`. Take the digest from the workflow's summary and use it in new problem files. Existing problems keep the digest they were made with.
