# Problem file format

A problem file is TOML, at most 64 KiB. Unknown fields are refused, so a typo is an error instead of being silently ignored.

```toml
id = "ev-vsock-credit-0007"
project = "example-virtio"
repo = "https://github.com/proofathome/example-virtio"
commit = "0123456789abcdef0123456789abcdef01234567"
checker = { tool = "verus", version = "0.2026.10.26.d17fbd0", image = "sha256:<64 hex>" }
target = "example-virtio/src/vsock/credit.rs::peer_avail_credit"
spec_files = { "example-virtio/src/vsock/credit_spec.rs" = "<sha256>" }
editable = ["example-virtio/src/vsock/credit.rs", "example-virtio/src/vsock/credit_proof.rs"]
mode = "ghost-only"
time_budget_s = 120
size = "S"
difficulty = 3
hints = "The credit counters wrap."
```

| Field | Rule |
| --- | --- |
| `id`, `project` | 2–64 lowercase letters, digits and single hyphens, not starting or ending with a hyphen. |
| `repo` | `https://` URL, at most 200 characters, no credentials, query or fragment. |
| `commit` | Full commit ID: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits. Branch names are refused. |
| `checker.tool` | `verus` or `kani`. |
| `checker.version` | 1–64 letters, digits or `.-+_`. |
| `checker.image` | Checker container image digest: `sha256:` and 64 lowercase hex digits. |
| `target` | `<file>::<item path>`. The file is a repository-relative `.rs` path; the item is one or more Rust identifiers joined by `::`, for example `src/queue.rs::Queue::pop`. |
| `spec_files` | 1–32 repository-relative paths, each with the SHA-256 of its content. A result must leave them unchanged. |
| `editable` | 1–32 distinct repository-relative paths. The only files a result may change or add. Never a spec file. |
| `mode` | `ghost-only`: only ghost code may change; erasing it must give the same exec code (Verus). `proof-files`: whole proof files such as Kani harnesses; the target's own file may not be editable. |
| `time_budget_s` | 1–3600. The checker must accept the result within this time on reference hardware. |
| `size` | Budget class, see below. |
| `difficulty` | 1–5. Scales points. |
| `hints` | Optional, at most 2000 characters, no control characters except newlines, no bidirectional formatting characters. Shown to the agent as untrusted text. |

Repository-relative paths use `/`, have no empty, `.` or `..` segments, no leading `/`, no `\` or `:`, and are at most 256 bytes.

## Sizes

| Size | Expected tokens per attempt |
| --- | --- |
| `XS` | up to 100k |
| `S` | up to 400k |
| `M` | up to 1.5M |
| `L` | up to 5M |
| `XL` | no bound; only for contributors who opt in |

A problem fits a budget when its size's upper bound is at most the remaining tokens. `XL` never fits a token budget. The numbers are placeholders until the pilot measures real attempts.
