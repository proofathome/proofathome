//! Proof@home: volunteer coding agents writing machine-checked proofs for
//! open-source infrastructure.
//!
//! This crate holds the problem file format. A problem is one small proof
//! obligation, pinned to a repository commit and a checker image, with a size
//! class that tells a contributor's client whether it fits their budget.
//!
//! ```
//! let problem = proofathome_core::Problem::from_toml_str(r#"
//! id = "ev-vsock-credit-0007"
//! project = "example-virtio"
//! repo = "https://github.com/proofathome/example-virtio"
//! commit = "0123456789abcdef0123456789abcdef01234567"
//! checker = { tool = "verus", version = "0.2026.10.26.d17fbd0", image = "ghcr.io/proofathome/verus@sha256:0000000000000000000000000000000000000000000000000000000000000000" }
//! check = { crate_root = "example-virtio/src/lib.rs" }
//! target = "example-virtio/src/vsock/credit.rs::peer_avail_credit"
//! spec_files = { "example-virtio/src/vsock/credit_spec.rs" = "1111111111111111111111111111111111111111111111111111111111111111" }
//! editable = ["example-virtio/src/vsock/credit.rs"]
//! mode = "ghost-only"
//! time_budget_s = 120
//! size = "S"
//! difficulty = 3
//! "#).unwrap();
//! assert!(problem.size.fits(500_000));
//! assert_eq!(problem.check_command().argv, ["verus", "--crate-type=lib", "example-virtio/src/lib.rs"]);
//! ```

pub mod problem;

pub use problem::{
    CONTAINER_WORKDIR, Check, CheckCommand, Checker, CheckerTool, EditMode, ImageRef,
    MAX_PROBLEM_FILE_BYTES, ParseError, Problem, Size, Target,
};
