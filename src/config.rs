//! Configuration file for voido.
//!
//! Hand-editable JSON at `~/.config/voido/config.json` (macOS:
//! `~/Library/Application Support/voido/config.json`). All keys are optional;
//! a missing or `null` value falls back to the default.
//!
//! ```json
//! {
//!   "storage": "github",          // "local" (default) or "github"
//!   "github_repo": "me/notes",     // owner/repo, or a bare name (owner = you)
//!   "github_file": "notes.json",   // data-file name in the repo (default: voido-data.json)
//!   "github_token": null,          // usually left null — resolved from `gh` or $GITHUB_TOKEN
//!   "theme": "catppuccin-mocha",   // pick live with ^t
//!   "themes": []                    // custom themes, see `crate::theme::ThemeSpec`
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Data-file name used inside the sync repo when `github_file` isn't set.
pub const DEFAULT_SYNC_FILE: &str = "voido-data.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// `"local"` (default) or `"github"`.
    #[serde(default)]
    pub storage: StorageChoice,
    /// `owner/repo` that holds the synced data file. A bare `repo` also works —
    /// the owner is taken from the authenticated account.
    #[serde(default)]
    pub github_repo: Option<String>,
    /// Name of the data file written inside the repo. Defaults to
    /// `voido-data.json`; set this to sync under a different filename.
    #[serde(default)]
    pub github_file: Option<String>,
    /// Personal access token (classic, `repo` scope — or fine-grained with
    /// Contents read/write). Used for data sync and the repo activity view.
    /// Usually left `null`: voido reads `gh auth token` / `$GITHUB_TOKEN` first.
    #[serde(default)]
    pub github_token: Option<String>,
    /// Colour theme slug (e.g. `"dracula"`). Unknown / `null` → the default.
    /// Pick one live with `^t`.
    #[serde(default)]
    pub theme: Option<String>,
    /// Custom themes, added to the `^t` picker. See [`crate::theme::ThemeSpec`].
    #[serde(default)]
    pub themes: Vec<crate::theme::ThemeSpec>,
}

impl Config {
    /// GitHub sync is turned on and has a repo. A usable token is resolved
    /// separately (config value, `gh` CLI, or environment), so it isn't checked
    /// here — see `App::sync_ready`.
    pub fn sync_configured(&self) -> bool {
        self.storage == StorageChoice::GitHub
            && self
                .github_repo
                .as_deref()
                .is_some_and(|r| r.contains('/') && r.len() > 2)
    }

    /// The data-file name to read/write inside the repo: the `github_file`
    /// override if set, otherwise [`DEFAULT_SYNC_FILE`]. Leading slashes and
    /// surrounding whitespace are trimmed.
    pub fn sync_file(&self) -> String {
        self.github_file
            .as_deref()
            .map(|s| s.trim().trim_start_matches('/'))
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SYNC_FILE)
            .to_string()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageChoice {
    GitHub,
    #[default]
    Local,
}

impl Config {
    pub fn path() -> PathBuf {
        let mut dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        dir.push("voido");
        fs::create_dir_all(&dir).ok();
        dir.push("config.json");
        dir
    }

    /// Read the settings file. `Ok(None)` means it doesn't exist yet (first run);
    /// `Err` means it exists but couldn't be read or parsed — the caller should
    /// stop rather than overwrite a file the user was editing.
    pub fn load() -> Result<Option<Self>, String> {
        let path = Self::path();
        let data = match fs::read_to_string(&path) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        serde_json::from_str(&data)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let data = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, data).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_file_defaults_and_overrides() {
        let mut c = Config::default();
        assert_eq!(c.sync_file(), DEFAULT_SYNC_FILE);

        c.github_file = Some("  /notes.json  ".to_string());
        assert_eq!(c.sync_file(), "notes.json", "trims space and leading slash");

        c.github_file = Some(String::new());
        assert_eq!(c.sync_file(), DEFAULT_SYNC_FILE, "blank falls back");

        c.github_file = Some("archive/2026.json".to_string());
        assert_eq!(c.sync_file(), "archive/2026.json", "subpaths kept");
    }

    #[test]
    fn config_parses_with_missing_keys() {
        let c: Config = serde_json::from_str(r#"{ "storage": "github" }"#).unwrap();
        assert_eq!(c.storage, StorageChoice::GitHub);
        assert!(c.github_repo.is_none());
        assert_eq!(c.sync_file(), DEFAULT_SYNC_FILE);
    }
}
