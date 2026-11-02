use proofathome_core::{
    Check, CheckCommand, CheckerTool, EditMode, MAX_PROBLEM_FILE_BYTES, ParseError, Problem, Size,
};

const VALID: &str = include_str!("fixtures/ev-vsock-credit-0007.toml");

/// Replaces the line starting with `key =` in the valid fixture.
fn with_line(key: &str, line: &str) -> String {
    let prefix = format!("{key} =");
    let mut found = false;
    let out = VALID
        .lines()
        .map(|l| {
            if l.starts_with(&prefix) {
                found = true;
                line.to_owned()
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(found, "fixture has no `{key}` line");
    out
}

fn invalid_field(text: &str) -> String {
    match Problem::from_toml_str(text) {
        Err(ParseError::Invalid { field, .. }) => field,
        other => panic!("expected an invalid field, got {other:?}"),
    }
}

fn assert_toml_error(text: &str) {
    match Problem::from_toml_str(text) {
        Err(ParseError::Toml(_)) => {}
        other => panic!("expected a TOML error, got {other:?}"),
    }
}

#[test]
fn parses_the_valid_fixture() {
    let p = Problem::from_toml_str(VALID).unwrap();
    assert_eq!(p.id, "ev-vsock-credit-0007");
    assert_eq!(p.project, "example-virtio");
    assert_eq!(p.checker.tool, CheckerTool::Verus);
    assert_eq!(p.checker.version, "0.2026.10.26.d17fbd0");
    assert_eq!(p.checker.image.name, "ghcr.io/proofathome/verus");
    assert_eq!(p.checker.image.digest, format!("sha256:{}", "0".repeat(64)));
    assert_eq!(
        p.check,
        Check::Verus {
            crate_root: "example-virtio/src/lib.rs".to_owned()
        }
    );
    assert_eq!(p.target.file, "example-virtio/src/vsock/credit.rs");
    assert_eq!(p.target.item, "peer_avail_credit");
    assert_eq!(p.mode, EditMode::GhostOnly);
    assert_eq!(p.size, Size::S);
    assert_eq!(p.difficulty, 3);
    assert_eq!(p.time_budget_s, 120);
    assert_eq!(p.editable.len(), 2);
    assert_eq!(p.spec_files.len(), 1);
    assert!(p.hints.is_some());
}

#[test]
fn accepts_a_kani_proof_files_problem() {
    let text = VALID
        .replace("tool = \"verus\"", "tool = \"kani\"")
        .replace("version = \"0.2026.10.26.d17fbd0\"", "version = \"0.66.0\"");
    let text = with_line_in(&text, "mode", "mode = \"proof-files\"");
    let text = with_line_in(&text, "check", KANI_CHECK);
    let text = with_line_in(
        &text,
        "editable",
        "editable = [\"example-virtio/src/vsock/credit_proof.rs\"]",
    );
    let p = Problem::from_toml_str(&text).unwrap();
    assert_eq!(p.checker.tool, CheckerTool::Kani);
    assert_eq!(p.mode, EditMode::ProofFiles);
    assert_eq!(
        p.check,
        Check::Kani {
            harness: "proofs::credit_wraps".to_owned(),
            manifest_dir: Some("example-virtio".to_owned()),
        }
    );
}

fn with_line_in(text: &str, key: &str, line: &str) -> String {
    let prefix = format!("{key} =");
    text.lines()
        .map(|l| if l.starts_with(&prefix) { line } else { l })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn proof_files_mode_refuses_the_targets_own_file() {
    let text = with_line("mode", "mode = \"proof-files\"");
    assert_eq!(invalid_field(&text), "editable");
}

#[test]
fn refuses_unknown_and_missing_fields() {
    assert_toml_error(&format!("{VALID}\nreward = 100\n"));
    assert_toml_error(&with_line("difficulty", ""));
    assert_toml_error(&VALID.replace("image = ", "registry = \"docker.io\", image = "));
}

#[test]
fn refuses_unknown_enum_values() {
    assert_toml_error(&with_line("size", "size = \"XXL\""));
    assert_toml_error(&with_line("mode", "mode = \"anything\""));
    assert_toml_error(&VALID.replace("tool = \"verus\"", "tool = \"lean\""));
}

#[test]
fn refuses_files_over_the_size_limit() {
    let text = format!("{VALID}\n#{}\n", "x".repeat(MAX_PROBLEM_FILE_BYTES));
    assert!(matches!(
        Problem::from_toml_str(&text),
        Err(ParseError::TooLarge { .. })
    ));
}

#[test]
fn refuses_bad_ids() {
    for id in [
        "", "a", "Upper", "-lead", "trail-", "dou--ble", "sp ace", "ünï",
    ] {
        let text = with_line("id", &format!("id = \"{id}\""));
        assert_eq!(invalid_field(&text), "id", "id {id:?}");
    }
    let long = "a".repeat(65);
    assert_eq!(
        invalid_field(&with_line("id", &format!("id = \"{long}\""))),
        "id"
    );
}

#[test]
fn refuses_bad_repo_urls() {
    for repo in [
        "http://github.com/x/y",
        "git@github.com:x/y.git",
        "https://user:pw@github.com/x/y",
        "https://github.com/x/y?ref=evil",
        "https://github.com/x/y#frag",
        "https:///x/y",
        "https://github.com/x y",
    ] {
        let text = with_line("repo", &format!("repo = \"{repo}\""));
        assert_eq!(invalid_field(&text), "repo", "repo {repo:?}");
    }
}

#[test]
fn refuses_short_or_uppercase_commits() {
    for commit in ["abc123", &"A".repeat(40), &"0".repeat(41), "main"] {
        let text = with_line("commit", &format!("commit = \"{commit}\""));
        assert_eq!(invalid_field(&text), "commit", "commit {commit:?}");
    }
    let sha256_repo = with_line("commit", &format!("commit = \"{}\"", "a".repeat(64)));
    Problem::from_toml_str(&sha256_repo).unwrap();
}

#[test]
fn refuses_bad_checker_pins() {
    let text = VALID.replace("0.2026.10.26.d17fbd0", "latest; rm -rf /");
    assert_eq!(invalid_field(&text), "checker.version");
}

const IMAGE: &str = "ghcr.io/proofathome/verus@sha256:";

fn with_image(image: &str) -> String {
    let zeros = "0".repeat(64);
    VALID.replace(&format!("{IMAGE}{zeros}"), image)
}

#[test]
fn accepts_pinned_image_references() {
    let zeros = "0".repeat(64);
    for name in [
        "verus",
        "proofathome/verus",
        "ghcr.io/proofathome/verus",
        "localhost:5000/verus",
        "registry.example.org:443/a/b/c_d.e-f",
    ] {
        let text = with_image(&format!("{name}@sha256:{zeros}"));
        let p = Problem::from_toml_str(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(p.checker.image.name, name);
        assert_eq!(
            p.checker.image.to_string(),
            format!("{name}@sha256:{zeros}")
        );
    }
}

#[test]
fn refuses_unpinned_or_malformed_images() {
    let zeros = "0".repeat(64);
    let long_name = "a".repeat(201);
    for image in [
        format!("sha256:{zeros}"),
        format!("@sha256:{zeros}"),
        "ghcr.io/x/verus:latest".to_owned(),
        format!("ghcr.io/x/verus:1.0@sha256:{zeros}"),
        format!("ghcr.io/x/verus@sha512:{zeros}"),
        format!("ghcr.io/x/verus@sha256:{}", "0".repeat(63)),
        format!("ghcr.io/x/verus@sha256:{}", "A".repeat(64)),
        format!("ghcr.io/x/verus@sha256:{zeros}@sha256:{zeros}"),
        format!("GHCR.io/x/verus@sha256:{zeros}"),
        format!("ghcr.io//verus@sha256:{zeros}"),
        format!("ghcr.io/x/verus/@sha256:{zeros}"),
        format!("ghcr.io/-x/verus@sha256:{zeros}"),
        format!("ghcr.io:port/verus@sha256:{zeros}"),
        format!("ghcr.io:123456/verus@sha256:{zeros}"),
        format!("ghcr.io/x y/verus@sha256:{zeros}"),
        format!("{long_name}@sha256:{zeros}"),
    ] {
        assert_eq!(
            invalid_field(&with_image(&image)),
            "checker.image",
            "image {image:?}"
        );
    }
}

const KANI_CHECK: &str =
    "check = { harness = \"proofs::credit_wraps\", manifest_dir = \"example-virtio\" }";

fn kani(check_line: &str) -> String {
    let text = VALID.replace("tool = \"verus\"", "tool = \"kani\"");
    with_line_in(&text, "check", check_line)
}

#[test]
fn verus_runs_a_fixed_command_read_only() {
    let p = Problem::from_toml_str(VALID).unwrap();
    assert_eq!(
        p.check_command(),
        CheckCommand {
            argv: vec![
                "verus".to_owned(),
                "--crate-type=lib".to_owned(),
                "example-virtio/src/lib.rs".to_owned(),
            ],
            workdir: "/work".to_owned(),
            writable: false,
        }
    );
}

#[test]
fn kani_runs_a_fixed_command_in_the_manifest_dir() {
    let p = Problem::from_toml_str(&kani(KANI_CHECK)).unwrap();
    let command = p.check_command();
    assert_eq!(
        command.argv,
        ["cargo", "kani", "--harness", "proofs::credit_wraps"]
    );
    assert_eq!(command.workdir, "/work/example-virtio");
    assert!(command.writable);

    let p = Problem::from_toml_str(&kani("check = { harness = \"credit_wraps\" }")).unwrap();
    assert_eq!(
        p.check,
        Check::Kani {
            harness: "credit_wraps".to_owned(),
            manifest_dir: None,
        }
    );
    assert_eq!(p.check_command().workdir, "/work");
}

#[test]
fn refuses_check_keys_for_the_wrong_tool() {
    let text = with_line("check", "check = { harness = \"proofs::h\" }");
    assert_eq!(invalid_field(&text), "check");
    let text = with_line(
        "check",
        "check = { crate_root = \"src/lib.rs\", manifest_dir = \"x\" }",
    );
    assert_eq!(invalid_field(&text), "check");
    let text = kani("check = { crate_root = \"src/lib.rs\" }");
    assert_eq!(invalid_field(&text), "check");
    let text = kani("check = { harness = \"h\", crate_root = \"src/lib.rs\" }");
    assert_eq!(invalid_field(&text), "check");
}

#[test]
fn refuses_missing_check_keys() {
    assert_eq!(invalid_field(&with_line("check", "check = {}")), "check");
    assert_eq!(invalid_field(&kani("check = {}")), "check");
    assert_eq!(
        invalid_field(&kani("check = { manifest_dir = \"x\" }")),
        "check"
    );
    assert_toml_error(&with_line("check", ""));
}

#[test]
fn refuses_unknown_check_keys() {
    assert_toml_error(&with_line(
        "check",
        "check = { crate_root = \"src/lib.rs\", command = \"sh\" }",
    ));
    assert_toml_error(&kani("check = { harness = \"h\", args = [\"--x\"] }"));
}

#[test]
fn refuses_bad_check_values() {
    for line in [
        "check = { crate_root = \"src/lib.txt\" }",
        "check = { crate_root = \"../lib.rs\" }",
        "check = { crate_root = \"/src/lib.rs\" }",
    ] {
        assert_eq!(invalid_field(&with_line("check", line)), "check", "{line}");
    }
    for line in [
        "check = { harness = \"\" }",
        "check = { harness = \"proofs::\" }",
        "check = { harness = \"a b\" }",
        "check = { harness = \"--all\" }",
        "check = { harness = \"h\", manifest_dir = \"../x\" }",
        "check = { harness = \"h\", manifest_dir = \"x/\" }",
    ] {
        assert_eq!(invalid_field(&kani(line)), "check", "{line}");
    }
}

#[test]
fn refuses_escaping_paths() {
    for path in [
        "../outside.rs",
        "/etc/passwd",
        "a//b.rs",
        "a/./b.rs",
        "a\\\\b.rs",
        "C:/x.rs",
        "a/b/",
    ] {
        let text = with_line("editable", &format!("editable = [\"{path}\"]"));
        assert_eq!(invalid_field(&text), "editable", "path {path:?}");
    }
}

#[test]
fn refuses_bad_editable_lists() {
    assert_eq!(
        invalid_field(&with_line("editable", "editable = []")),
        "editable"
    );
    let twice = "editable = [\"a.rs\", \"a.rs\"]";
    assert_eq!(invalid_field(&with_line("editable", twice)), "editable");
    let many = (0..33).map(|i| format!("\"f{i}.rs\"")).collect::<Vec<_>>();
    let many = format!("editable = [{}]", many.join(", "));
    assert_eq!(invalid_field(&with_line("editable", &many)), "editable");
}

#[test]
fn refuses_editable_spec_files() {
    let text = with_line(
        "editable",
        "editable = [\"example-virtio/src/vsock/credit_spec.rs\"]",
    );
    assert_eq!(invalid_field(&text), "editable");
}

#[test]
fn refuses_bad_spec_files() {
    assert_eq!(
        invalid_field(&with_line("spec_files", "spec_files = {}")),
        "spec_files"
    );
    let bad_hash = "spec_files = { \"spec.rs\" = \"abc\" }";
    assert_eq!(
        invalid_field(&with_line("spec_files", bad_hash)),
        "spec_files.\"spec.rs\""
    );
    let bad_path = format!("spec_files = {{ \"../spec.rs\" = \"{}\" }}", "1".repeat(64));
    assert_eq!(
        invalid_field(&with_line("spec_files", &bad_path)),
        "spec_files"
    );
}

#[test]
fn refuses_bad_targets() {
    for target in [
        "credit.rs",
        "credit.rs::",
        "credit.txt::f",
        "../credit.rs::f",
        "credit.rs::f()",
        "credit.rs::Queue::",
        "credit.rs::1abc",
        "credit.rs::_",
    ] {
        let text = with_line("target", &format!("target = \"{target}\""));
        assert_eq!(invalid_field(&text), "target", "target {target:?}");
    }
    let nested = with_line("target", "target = \"src/queue.rs::Queue::pop\"");
    let p = Problem::from_toml_str(&nested).unwrap();
    assert_eq!(p.target.item, "Queue::pop");
}

#[test]
fn refuses_out_of_range_numbers() {
    for line in ["time_budget_s = 0", "time_budget_s = 3601"] {
        assert_eq!(
            invalid_field(&with_line("time_budget_s", line)),
            "time_budget_s"
        );
    }
    for line in ["difficulty = 0", "difficulty = 6"] {
        assert_eq!(invalid_field(&with_line("difficulty", line)), "difficulty");
    }
    assert_toml_error(&with_line("difficulty", "difficulty = -1"));
}

#[test]
fn refuses_hints_with_hidden_characters() {
    for hint in ["\\u0007bell", "\\u202Eevil", "\\u2066isolate", "tab\\there"] {
        let text = with_line("hints", &format!("hints = \"{hint}\""));
        assert_eq!(invalid_field(&text), "hints", "hint {hint:?}");
    }
    let long = with_line("hints", &format!("hints = \"{}\"", "x".repeat(2001)));
    assert_eq!(invalid_field(&long), "hints");
    let multiline = with_line("hints", "hints = \"line one\\nline two\"");
    Problem::from_toml_str(&multiline).unwrap();
}

#[test]
fn hints_are_optional() {
    let p = Problem::from_toml_str(&with_line("hints", "")).unwrap();
    assert_eq!(p.hints, None);
}

#[test]
fn sizes_fit_budgets() {
    assert!(Size::XS.fits(100_000));
    assert!(!Size::S.fits(399_999));
    assert!(Size::S.fits(400_000));
    assert!(!Size::XL.fits(u64::MAX));
    assert_eq!(Size::largest_fitting(50_000), None);
    assert_eq!(Size::largest_fitting(600_000), Some(Size::S));
    assert_eq!(Size::largest_fitting(u64::MAX), Some(Size::L));
    assert!(Size::ALL.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(Size::M.to_string(), "M");
}

#[test]
fn errors_name_the_problem() {
    let error = Problem::from_toml_str(&with_line("difficulty", "difficulty = 9")).unwrap_err();
    assert_eq!(error.to_string(), "`difficulty` must be between 1 and 5");
}
