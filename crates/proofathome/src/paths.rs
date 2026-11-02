//! Where Proof@home keeps its files, and the config file.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsutil::atomic_write;

pub const HOME_ENV: &str = "PROOFATHOME_HOME";
pub const CATALOG_ENV: &str = "PROOFATHOME_CATALOG";

/// The full form of an existing path. On Windows this expands short (8.3)
/// names such as `GEBRUI~1`: an agent would not see a workspace given in
/// that form as inside its working directory, and would refuse to read it.
fn full_path(path: &Path) -> PathBuf {
    match fs::canonicalize(path) {
        Ok(full) => strip_verbatim(full),
        Err(_) => path.to_path_buf(),
    }
}

/// `canonicalize` on Windows returns `\\?\C:\...`, which many programs
/// can't use. Drop that prefix for drive and UNC paths.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
            if let Some(unc) = rest.strip_prefix(r"UNC\") {
                return PathBuf::from(format!(r"\\{unc}"));
            }
            if rest.as_bytes().get(1) == Some(&b':') {
                return PathBuf::from(rest);
            }
        }
    }
    path
}

/// The Proof@home home directory and the files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    /// `PROOFATHOME_HOME`, else the platform's per-user data directory.
    pub fn from_env() -> Result<Home> {
        let root = match non_empty_var(HOME_ENV) {
            Some(path) => PathBuf::from(path),
            None => platform_data_dir()?.join("proofathome"),
        };
        Home::new(root)
    }

    /// Creates the directory if needed and keeps its full path.
    pub fn new(root: impl Into<PathBuf>) -> Result<Home> {
        let root = absolute(&root.into())?;
        fs::create_dir_all(&root)
            .map_err(|e| Error::new(format!("cannot create {}: {e}", root.display())))?;
        Ok(Home {
            root: full_path(&root),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    pub fn work_dir(&self) -> PathBuf {
        self.root.join("work")
    }

    pub fn workspace(&self, id: &str) -> PathBuf {
        self.work_dir().join(id)
    }

    pub fn submissions_dir(&self) -> PathBuf {
        self.root.join("submissions")
    }

    /// Raw agent output of `start` sessions. Never uploaded.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

fn non_empty_var(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn platform_data_dir() -> Result<PathBuf> {
    let missing = |name: &str| {
        Error::new(format!(
            "cannot find the data directory: {name} is not set; set {HOME_ENV} instead"
        ))
    };
    if cfg!(windows) {
        return non_empty_var("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| missing("LOCALAPPDATA"));
    }
    let home = non_empty_var("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home
            .map(|home| home.join("Library").join("Application Support"))
            .ok_or_else(|| missing("HOME"));
    }
    // The XDG spec says to ignore relative values.
    if let Some(xdg) = non_empty_var("XDG_DATA_HOME").map(PathBuf::from) {
        if xdg.is_absolute() {
            return Ok(xdg);
        }
    }
    home.map(|home| home.join(".local").join("share"))
        .ok_or_else(|| missing("HOME"))
}

pub(crate) fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|e| Error::io("resolve", path, e))
}

/// `<home>/config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Absolute path of the problem catalog directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
}

impl Config {
    pub fn load(home: &Home) -> Result<Config> {
        let path = home.config_file();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(Error::io("read", &path, e)),
        };
        let config: Config = toml::from_str(&text)
            .map_err(|e| Error::new(format!("{} is not valid: {}", path.display(), e.message())))?;
        if let Some(catalog) = &config.catalog {
            if !Path::new(catalog).is_absolute() {
                return Err(Error::new(format!(
                    "{}: `catalog` must be an absolute path, got `{catalog}`",
                    path.display()
                )));
            }
        }
        Ok(config)
    }

    pub fn save(&self, home: &Home) -> Result<()> {
        let text = toml::to_string(self)
            .map_err(|e| Error::new(format!("cannot write the config: {e}")))?;
        atomic_write(&home.config_file(), text.as_bytes())
    }
}

/// Sets `catalog` in the config to the absolute form of `dir`, which must be
/// an existing directory.
pub fn set_catalog(home: &Home, dir: &Path) -> Result<PathBuf> {
    let dir = absolute(dir)?;
    if !dir.is_dir() {
        return Err(Error::new(format!("{} is not a directory", dir.display())));
    }
    let text = dir
        .to_str()
        .ok_or_else(|| Error::new(format!("{} is not valid UTF-8", dir.display())))?;
    let mut config = Config::load(home)?;
    config.catalog = Some(text.to_owned());
    config.save(home)?;
    Ok(dir)
}

/// The catalog directory: `PROOFATHOME_CATALOG`, else the config.
pub fn catalog_dir(home: &Home) -> Result<PathBuf> {
    if let Some(dir) = non_empty_var(CATALOG_ENV) {
        return absolute(Path::new(&dir));
    }
    match Config::load(home)?.catalog {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Err(Error::new(
            "no problem catalog is configured; run `proofathome config set catalog <dir>` \
             or set PROOFATHOME_CATALOG",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_created_and_kept_in_full_form() {
        let dir = std::env::temp_dir().join(format!("pah-home-{}", std::process::id()));
        let home = Home::new(dir.join("nested")).unwrap();
        assert!(home.root().is_dir());
        assert!(!home.root().to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(
            home.root(),
            fs::canonicalize(dir.join("nested"))
                .map(strip_verbatim)
                .unwrap()
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_prefixes_are_dropped() {
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\C:\Users\x")),
            PathBuf::from(r"C:\Users\x")
        );
        assert_eq!(
            strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\x")),
            PathBuf::from(r"\\server\share\x")
        );
    }
}
