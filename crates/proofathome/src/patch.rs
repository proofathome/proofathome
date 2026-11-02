//! The result patch: every change in the workspace, including new files,
//! against the pinned commit.

use std::fs;
use std::path::Path;

use crate::error::Result;
use crate::fsutil::{create_dir_all, random_hex};
use crate::git;

/// A binary-safe diff of the workspace against `commit`. Uses a temporary
/// index in `tmp_dir`, so the workspace's own index is never touched.
pub fn make(workspace: &Path, commit: &str, tmp_dir: &Path) -> Result<Vec<u8>> {
    create_dir_all(tmp_dir)?;
    let index = tmp_dir.join(format!("index-{}", random_hex()));
    let result = diff_with_index(workspace, commit, &index);
    let _ = fs::remove_file(&index);
    let _ = fs::remove_file(index.with_extension("lock"));
    result
}

fn diff_with_index(workspace: &Path, commit: &str, index: &Path) -> Result<Vec<u8>> {
    let run = |args: &[&str]| {
        let mut command = git::command(Some(workspace));
        command.env("GIT_INDEX_FILE", index).args(args);
        git::output(command)
    };
    run(&["read-tree", commit])?;
    run(&["add", "--all", "--", "."])?;
    run(&[
        "diff",
        "--cached",
        "--binary",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        commit,
        "--",
    ])
}
