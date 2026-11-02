//! Proof@home: volunteer coding agents writing machine-checked proofs for
//! open-source infrastructure.
//!
//! This crate holds the problem file format. A problem is one small proof
//! obligation, pinned to a repository commit and a checker version, with a
//! size class that tells a contributor's client whether it fits their budget.
//!
//! ```
//! let problem = proofathome::Problem::from_toml_str(r#"
//! id = "ev-vsock-credit-0007"
//! project = "example-virtio"
//! repo = "https://github.com/proofathome/example-virtio"
//! commit = "0123456789abcdef0123456789abcdef01234567"
//! checker = { tool = "verus", version = "0.2026.10.26.d17fbd0", image = "sha256:0000000000000000000000000000000000000000000000000000000000000000" }
//! target = "example-virtio/src/vsock/credit.rs::peer_avail_credit"
//! spec_files = { "example-virtio/src/vsock/credit_spec.rs" = "1111111111111111111111111111111111111111111111111111111111111111" }
//! editable = ["example-virtio/src/vsock/credit.rs"]
//! mode = "ghost-only"
//! time_budget_s = 120
//! size = "S"
//! difficulty = 3
//! "#).unwrap();
//! assert!(problem.size.fits(500_000));
//! ```

pub mod problem;

pub use problem::{
    Checker, CheckerTool, EditMode, MAX_PROBLEM_FILE_BYTES, ParseError, Problem, Size, Target,
};
