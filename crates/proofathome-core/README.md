# proofathome-core

The problem file format of [Proof@home](https://proofathome.org): one small proof obligation, pinned to a repository commit and a checker image, with a size class for budgeting.

```rust
let problem = proofathome_core::Problem::from_toml_str(&text)?;
if problem.size.fits(remaining_tokens) {
    // offer it
}
```

See [docs/problem-format.md](https://github.com/proofathome/proofathome/blob/main/docs/problem-format.md) for the rules. The `proofathome` crate holds the CLI and MCP server.

License: MIT.
