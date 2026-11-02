//! A local problem catalog: the `*.toml` problem files directly inside one
//! directory. Subdirectories are not scanned.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use proofathome_core::{MAX_PROBLEM_FILE_BYTES, Problem};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    /// Valid problems, sorted by ID.
    pub problems: Vec<Problem>,
    /// One line per file that was skipped, naming the file and the reason.
    pub warnings: Vec<String>,
}

/// Reads every `*.toml` file in `dir`. Invalid files are skipped with a
/// warning; two files with the same problem ID are an error.
pub fn load(dir: &Path) -> Result<Catalog> {
    let entries = fs::read_dir(dir).map_err(|e| Error::io("read the catalog", dir, e))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io("read the catalog", dir, e))?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml") && path.is_file() {
            paths.push(path);
        }
    }
    paths.sort();

    let mut by_id: BTreeMap<String, (Problem, String)> = BTreeMap::new();
    let mut warnings = Vec::new();
    for path in paths {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let problem = match read_problem(&path) {
            Ok(problem) => problem,
            Err(reason) => {
                warnings.push(format!("skipped {name}: {reason}"));
                continue;
            }
        };
        if let Some((_, other)) = by_id.get(&problem.id) {
            return Err(Error::new(format!(
                "the catalog has problem `{}` twice, in {other} and {name}",
                problem.id
            )));
        }
        by_id.insert(problem.id.clone(), (problem, name));
    }
    Ok(Catalog {
        problems: by_id.into_values().map(|(problem, _)| problem).collect(),
        warnings,
    })
}

fn read_problem(path: &Path) -> std::result::Result<Problem, String> {
    let len = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_PROBLEM_FILE_BYTES as u64 {
        return Err(format!(
            "problem file is {len} bytes, the limit is {MAX_PROBLEM_FILE_BYTES}"
        ));
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    Problem::from_toml_str(&text).map_err(|e| e.to_string())
}
