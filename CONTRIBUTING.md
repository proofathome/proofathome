# Contributing

Thanks for helping. This file is about contributing to Proof@home's own code. Solving problems through the program is described in the [README](README.md).

## Before you start

- For anything larger than a small fix, open an issue first so we can agree on the approach.
- Keep each pull request to one logical change.

## Code

- Rust only. The toolchain is pinned in `rust-toolchain.toml`.
- Before pushing, run:
  ```sh
  cargo fmt --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  ```
- Add tests for every change in behaviour, including the cases that must be refused. The problem parser is a trust boundary: every new field needs tests for invalid values.
- New dependencies need a reason in the pull request. Prefer the standard library.
- Treat anything from a problem file, a repository or a checker as untrusted input. Never pass it to a shell or use it to build a path without validating it.

## Commits

- Write commit titles in the imperative, at most 72 characters, for example `Refuse problem files with duplicate editable paths`.
- Signed commits are welcome and required for maintainers.

## License

By contributing, you agree that your contribution is licensed under the [MIT license](LICENSE), the same as the rest of the project.
