mod common;

use std::fs;

use common::{Fixture, SPEC_PATH, TARGET_PATH, git};

#[test]
fn config_set_and_get_the_catalog() {
    let f = Fixture::new("config");
    let mut set = f.command(&["config", "set", "catalog"]);
    set.arg(&f.catalog).env_remove("PROOFATHOME_CATALOG");
    let output = set.output().unwrap();
    assert!(output.status.success());

    let mut get = f.command(&["config", "get", "catalog"]);
    get.env_remove("PROOFATHOME_CATALOG");
    let text = String::from_utf8(get.output().unwrap().stdout).unwrap();
    assert!(text.contains(&f.catalog.display().to_string()), "{text}");
    let config = fs::read_to_string(f.home.join("config.toml")).unwrap();
    assert!(config.starts_with("catalog = "), "{config}");

    let error = f.fails(&["config", "set", "colour", "blue"]);
    assert!(error.contains("unknown config key"), "{error}");
    let error = f.fails(&["config", "set", "catalog", "does-not-exist"]);
    assert!(error.contains("is not a directory"), "{error}");

    fs::write(f.home.join("config.toml"), "catalog = \"/x\"\nextra = 1\n").unwrap();
    let error = f.fails(&["config", "get"]);
    assert!(error.contains("config.toml is not valid"), "{error}");
    fs::write(f.home.join("config.toml"), "catalog = \"relative\"\n").unwrap();
    let mut problems = f.command(&["problems"]);
    problems.env_remove("PROOFATHOME_CATALOG");
    let output = problems.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("absolute path"));
}

#[test]
fn problems_need_a_catalog() {
    let f = Fixture::new("nocatalog");
    let mut command = f.command(&["problems"]);
    command.env_remove("PROOFATHOME_CATALOG");
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("config set catalog"),
        "{output:?}"
    );
}

#[test]
fn lists_and_filters_problems_and_skips_invalid_files() {
    let f = Fixture::new("list");
    f.add_problem("ev-a-0001", "S", 2);
    f.add_problem("ev-b-0002", "L", 1);
    fs::write(f.catalog.join("broken.toml"), "id = 3\n").unwrap();
    fs::write(f.catalog.join("notes.txt"), "not a problem").unwrap();
    fs::create_dir(f.catalog.join("sub")).unwrap();

    let output = f.run(&["problems"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("ev-a-0001") && stdout.contains("ev-b-0002"));
    assert!(stderr.contains("skipped broken.toml"), "{stderr}");

    let small = f.ok(&["problems", "--max-size", "M"]);
    assert!(small.contains("ev-a-0001") && !small.contains("ev-b-0002"));
    let large = f.ok(&["problems", "--size", "l"]);
    assert!(!large.contains("ev-a-0001") && large.contains("ev-b-0002"));
    let none = f.ok(&["problems", "--project", "other"]);
    assert!(none.contains("No problems match"));
    f.ok(&["claim", "ev-a-0001"]);
    let claimed = f.ok(&["problems", "--status", "claimed"]);
    assert!(claimed.contains("ev-a-0001") && claimed.contains("claimed until"));
    let open = f.ok(&["problems", "--status", "open"]);
    assert!(!open.contains("ev-a-0001") && open.contains("ev-b-0002"));
    f.fails(&["problems", "--status", "done"]);
    f.fails(&["problems", "--size", "XXL"]);
}

#[test]
fn duplicate_ids_are_an_error() {
    let f = Fixture::new("dup");
    f.add_problem("ev-a-0001", "S", 2);
    fs::copy(
        f.catalog.join("ev-a-0001.toml"),
        f.catalog.join("copy.toml"),
    )
    .unwrap();
    let error = f.fails(&["problems"]);
    assert!(error.contains("twice"), "{error}");
}

#[test]
fn claims_follow_the_rules() {
    let f = Fixture::new("claims");
    for (i, size) in ["XS", "S", "M", "L"].iter().enumerate() {
        f.add_problem(&format!("ev-c-000{i}"), size, 1);
    }
    let out = f.ok(&["claim", "ev-c-0000"]);
    assert!(out.contains("Claimed ev-c-0000 until"));
    let error = f.fails(&["claim", "ev-c-0000"]);
    assert!(error.contains("already claimed until"), "{error}");
    f.ok(&["claim", "ev-c-0001"]);
    f.ok(&["claim", "ev-c-0003"]);
    let error = f.fails(&["claim", "ev-c-0002"]);
    assert!(error.contains("already hold 3 claims"), "{error}");
    f.ok(&["release", "ev-c-0001"]);
    f.ok(&["claim", "ev-c-0002"]);
    let error = f.fails(&["release", "ev-c-0001"]);
    assert!(error.contains("no active claim"), "{error}");
    let error = f.fails(&["claim", "ev-nope-0001"]);
    assert!(error.contains("no problem `ev-nope-0001`"), "{error}");

    // TTL: 2 h for XS to M, 24 h for L.
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(f.home.join("state/state.json")).unwrap()).unwrap();
    for claim in state["claims"].as_array().unwrap() {
        let ttl = claim["expires_at"].as_u64().unwrap() - claim["claimed_at"].as_u64().unwrap();
        let expected = if claim["id"] == "ev-c-0003" {
            86_400
        } else {
            7_200
        };
        assert_eq!(ttl, expected, "{claim}");
    }
}

#[test]
fn expired_claims_count_as_open() {
    let f = Fixture::new("expired");
    f.add_problem("ev-a-0001", "S", 1);
    fs::create_dir_all(f.home.join("state")).unwrap();
    let claims: Vec<String> = (0..3)
        .map(|i| format!(r#"{{"id":"ev-old-000{i}","claimed_at":1,"expires_at":2}}"#))
        .chain([r#"{"id":"ev-a-0001","claimed_at":1,"expires_at":2}"#.to_owned()])
        .collect();
    fs::write(
        f.home.join("state/state.json"),
        format!(r#"{{"claims":[{}],"submitted":[]}}"#, claims.join(",")),
    )
    .unwrap();
    let open = f.ok(&["problems", "--status", "open"]);
    assert!(open.contains("ev-a-0001"));
    f.ok(&["claim", "ev-a-0001"]);
    let state = fs::read_to_string(f.home.join("state/state.json")).unwrap();
    assert!(
        !state.contains("ev-old"),
        "expired claims are dropped: {state}"
    );
}

#[test]
fn damaged_state_is_reported() {
    let f = Fixture::new("damaged");
    f.add_problem("ev-a-0001", "S", 1);
    fs::create_dir_all(f.home.join("state")).unwrap();
    fs::write(f.home.join("state/state.json"), "{nope").unwrap();
    let error = f.fails(&["claim", "ev-a-0001"]);
    assert!(error.contains("is damaged"), "{error}");
}

#[test]
fn work_prepares_and_resumes_the_workspace() {
    let f = Fixture::new("work");
    f.add_problem("ev-a-0001", "S", 1);
    let path = f.ok(&["work", "ev-a-0001"]);
    let ws = f.workspace("ev-a-0001");
    assert_eq!(path.trim(), ws.display().to_string());
    assert_eq!(git(&ws, &["rev-parse", "HEAD"]), f.commit);
    assert!(ws.join(SPEC_PATH).is_file());

    fs::write(ws.join(TARGET_PATH), "// edited\n").unwrap();
    let output = f.run(&["work", "ev-a-0001"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Kept the existing workspace"));
    assert_eq!(
        fs::read_to_string(ws.join(TARGET_PATH)).unwrap(),
        "// edited\n"
    );

    let shown = f.ok(&["show", "ev-a-0001"]);
    assert!(shown.contains("verus --crate-type=lib example-virtio/src/lib.rs"));
    assert!(shown.contains("untrusted"));
    assert!(shown.contains(&ws.display().to_string()));
}

#[test]
fn work_refuses_a_problem_whose_spec_hash_is_wrong() {
    let f = Fixture::new("badspec");
    f.add_problem("ev-a-0001", "S", 1);
    let path = f.catalog.join("ev-a-0001.toml");
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace(&f.spec_hash, &"0".repeat(64));
    fs::write(&path, text).unwrap();
    let error = f.fails(&["work", "ev-a-0001"]);
    assert!(error.contains("is broken"), "{error}");
    assert!(!f.workspace("ev-a-0001").exists());
    let leftovers: Vec<_> = fs::read_dir(f.home.join("work"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    assert!(leftovers.is_empty(), "temporary clone left behind");
}

#[test]
fn a_changed_git_config_stops_git_commands() {
    let f = Fixture::new("gitconfig");
    f.add_problem("ev-a-0001", "S", 1);
    f.ok(&["work", "ev-a-0001"]);
    let config = f.workspace("ev-a-0001").join(".git/config");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str("[core]\n\tfsmonitor = touch-owned\n");
    fs::write(&config, text).unwrap();
    let error = f.fails(&["check", "ev-a-0001"]);
    assert!(
        error.contains("changed since the workspace was prepared"),
        "{error}"
    );
    assert!(!f.runtime_log.exists());
}

#[test]
fn check_reports_every_policy_violation_without_running_the_checker() {
    let f = Fixture::new("policy");
    f.add_problem("ev-a-0001", "S", 1);
    f.ok(&["work", "ev-a-0001"]);
    let ws = f.workspace("ev-a-0001");
    fs::write(ws.join(TARGET_PATH), "// allowed edit\n").unwrap();
    fs::write(ws.join("README.md"), "changed\n").unwrap();
    fs::write(ws.join("example-virtio/new.rs"), "fn x() {}\n").unwrap();
    fs::write(
        ws.join(SPEC_PATH),
        "pub open spec fn credit_ok() -> bool { false }\n",
    )
    .unwrap();
    git(
        &ws,
        &[
            "mv",
            "example-virtio/src/lib.rs",
            "example-virtio/src/root.rs",
        ],
    );

    let output = f.fails(&["check", "ev-a-0001"]);
    assert!(output.contains("`README.md` changed"), "{output}");
    assert!(
        output.contains("`example-virtio/new.rs` changed"),
        "{output}"
    );
    assert!(
        output.contains("`example-virtio/src/root.rs` changed"),
        "{output}"
    );
    assert!(
        output.contains("`example-virtio/src/lib.rs` changed"),
        "{output}"
    );
    assert!(output.contains("spec file `example-virtio/src/vsock/credit_spec.rs` was changed"));
    assert!(
        !output.contains(&format!("`{TARGET_PATH}` changed")),
        "{output}"
    );
    assert!(output.contains("local pre-check"));
    assert!(!f.runtime_log.exists(), "the checker must not run");

    fs::remove_file(ws.join(SPEC_PATH)).unwrap();
    let output = f.fails(&["check", "ev-a-0001"]);
    assert!(output.contains("was deleted"), "{output}");
}

#[test]
fn check_runs_the_pinned_image_with_a_fixed_command() {
    let f = Fixture::new("check");
    f.add_problem("ev-a-0001", "S", 1);
    f.ok(&["work", "ev-a-0001"]);
    let ws = f.workspace("ev-a-0001");
    fs::write(ws.join(TARGET_PATH), "// proof\n").unwrap();

    let out = f.ok(&["check", "ev-a-0001"]);
    assert!(out.contains("PASSED"), "{out}");
    assert!(out.contains("1 verified, 0 errors"), "{out}");
    let args = f.runtime_args();
    let image = format!("ghcr.io/proofathome/verus@sha256:{}", "a".repeat(64));
    let at = args.iter().position(|a| *a == image).expect("image");
    assert_eq!(
        &args[at + 1..],
        ["verus", "--crate-type=lib", "example-virtio/src/lib.rs"]
    );
    let joined = args.join(" ");
    for part in [
        "run --rm --name pah-ev-a-0001-",
        "--network none",
        "--cap-drop ALL",
        "--security-opt no-new-privileges",
        "--pids-limit 512",
        "--memory 8g",
        "--workdir /work",
    ] {
        assert!(joined.contains(part), "missing {part:?} in {joined}");
    }
    let mount = format!("type=bind,source={},target=/work,readonly", ws.display());
    assert!(args.contains(&mount), "{joined}");

    let mut failing = f.command(&["check", "ev-a-0001"]);
    failing.env("PAH_FAKE_EXIT", "1");
    let output = failing.output().unwrap();
    assert!(!output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("FAILED: the checker exited with code 1"),
        "{text}"
    );
}

#[test]
fn check_without_a_runtime_says_what_is_missing() {
    let f = Fixture::new("noruntime");
    f.add_problem("ev-a-0001", "S", 1);
    f.ok(&["work", "ev-a-0001"]);
    let mut command = f.command(&["check", "ev-a-0001"]);
    command.env("PROOFATHOME_CONTAINER_RUNTIME", "no-such-runtime-xyz");
    let output = command.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not in PATH"));
}

#[test]
fn submit_refuses_when_the_check_fails() {
    let f = Fixture::new("submitfail");
    f.add_problem("ev-a-0001", "S", 1);
    f.ok(&["work", "ev-a-0001"]);
    fs::write(f.workspace("ev-a-0001").join("README.md"), "x\n").unwrap();
    let output = f.fails(&["submit", "ev-a-0001"]);
    assert!(output.contains("Not submitted"), "{output}");
    assert!(!f.home.join("submissions").exists());
    let claimed = f.ok(&["problems", "--status", "claimed"]);
    assert!(claimed.contains("ev-a-0001"));
}

#[test]
fn submit_writes_a_patch_that_applies_and_keeps_the_index() {
    let f = Fixture::new("submit");
    f.add_problem("ev-a-0001", "S", 1);
    f.ok(&["work", "ev-a-0001"]);
    let ws = f.workspace("ev-a-0001");
    fs::write(
        ws.join(TARGET_PATH),
        "pub fn peer_avail_credit() -> u32 { 1 }\n",
    )
    .unwrap();
    fs::write(
        ws.join("example-virtio/src/vsock/credit_proof.rs"),
        "// new\n",
    )
    .unwrap();
    let blob: Vec<u8> = (0..=255u8).chain([0, 0, 1]).collect();
    fs::write(ws.join("example-virtio/blob.bin"), &blob).unwrap();

    let error = f.fails(&["submit", "ev-a-0001", "--summary", "bad\u{202E}text"]);
    assert!(error.contains("bidirectional"), "{error}");

    let out = f.ok(&[
        "submit",
        "ev-a-0001",
        "--summary",
        "Added the loop invariant.",
    ]);
    assert!(out.contains("Submitted ev-a-0001"), "{out}");
    let dir = f.home.join("submissions").join("ev-a-0001");
    let run = fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    assert!(out.contains(&run.display().to_string()));

    let patch = fs::read(run.join("result.patch")).unwrap();
    let text = String::from_utf8_lossy(&patch);
    assert!(text.contains("GIT binary patch"), "{text}");
    assert!(text.contains("b/example-virtio/src/vsock/credit_proof.rs"));

    let summary: serde_json::Value =
        serde_json::from_slice(&fs::read(run.join("summary.json")).unwrap()).unwrap();
    assert_eq!(summary["problem_id"], "ev-a-0001");
    assert_eq!(summary["commit"], f.commit.as_str());
    assert_eq!(summary["checker"]["version"], "0.2026.10.26");
    assert_eq!(summary["summary"], "Added the loop invariant.");
    assert!(
        summary["output_tail"]
            .as_str()
            .unwrap()
            .contains("verified")
    );
    assert!(summary["check_duration_ms"].is_u64());

    // The workspace's own index is untouched: nothing staged.
    assert_eq!(git(&ws, &["diff", "--cached", "--name-only"]), "");

    // The patch recreates the result on a clean checkout.
    let clean = f.root.join("clean");
    git(
        &f.root,
        &[
            "clone",
            "--quiet",
            &f.remotes.join("example-virtio").to_string_lossy(),
            &clean.to_string_lossy(),
        ],
    );
    git(&clean, &["checkout", "--quiet", &f.commit]);
    git(
        &clean,
        &["apply", &run.join("result.patch").to_string_lossy()],
    );
    assert_eq!(
        fs::read(clean.join("example-virtio/blob.bin")).unwrap(),
        blob
    );
    assert_eq!(
        fs::read_to_string(clean.join(TARGET_PATH)).unwrap(),
        "pub fn peer_avail_credit() -> u32 { 1 }\n"
    );

    let submitted = f.ok(&["problems", "--status", "submitted"]);
    assert!(submitted.contains("ev-a-0001"));
    let status = f.ok(&["status"]);
    assert!(status.contains("Submitted: 1"), "{status}");
    assert!(status.contains("Claims (0 of 3)"), "{status}");
    let error = f.fails(&["submit", "ev-a-0001"]);
    assert!(error.contains("already submitted"), "{error}");
    let error = f.fails(&["claim", "ev-a-0001"]);
    assert!(error.contains("already submitted"), "{error}");
}

#[cfg(unix)]
#[test]
fn symlinks_in_editable_paths_are_refused() {
    let f = Fixture::new("symlink");
    f.add_problem("ev-a-0001", "S", 1);
    f.ok(&["work", "ev-a-0001"]);
    let ws = f.workspace("ev-a-0001");
    std::os::unix::fs::symlink(
        "/etc/passwd",
        ws.join("example-virtio/src/vsock/credit_proof.rs"),
    )
    .unwrap();
    let output = f.fails(&["check", "ev-a-0001"]);
    assert!(output.contains("symbolic link"), "{output}");
}

#[cfg(unix)]
#[test]
fn setup_registers_with_claude_idempotently() {
    let f = Fixture::new("setup");
    let log = f.root.join("claude.log");
    let claude = f.root.join("bin").join("claude");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    common::write_script(
        &claude,
        &format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\n[ \"$2\" = get ] && exit 0\nexit 0\n",
            log.display()
        ),
    );
    let mut command = f.command(&["setup"]);
    command
        .env("PROOFATHOME_CLAUDE_BIN", &claude)
        .env("PATH", f.root.join("bin"))
        .env_remove("PROOFATHOME_CONTAINER_RUNTIME");
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let calls = fs::read_to_string(&log).unwrap();
    let exe = env!("CARGO_BIN_EXE_proofathome");
    let lines: Vec<&str> = calls.lines().collect();
    assert_eq!(lines[0], "mcp get proofathome");
    assert_eq!(lines[1], "mcp remove -s user proofathome");
    assert_eq!(
        lines[2],
        format!("mcp add -s user proofathome -- {exe} mcp")
    );
    assert!(stdout.contains("Codex: not found"));
    assert!(stdout.contains("Container runtime: missing"));
    assert!(stdout.contains("\"mcpServers\""));
}
