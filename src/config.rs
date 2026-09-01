//! Configuration file for voido.
//!
//! Hand-editable TOML at `~/.config/voido/config.toml` (macOS:
//! `~/Library/Application Support/voido/config.toml`). All keys are optional;
//! an omitted value falls back to the default. voido writes the file with a
//! comment block documenting every key (rewritten on each save, so notes added
//! in the body don't persist — the header always does). A pre-existing
//! `config.json` from an older build is converted to TOML automatically on
//! first load (the old file is kept as `config.json.bak`).
//!
//! ```toml
//! storage = "github"          # "local" (default) or "github"
//! github_repo = "me/notes"    # owner/repo, or a bare name (owner = you)
//! github_file = "notes.json"  # data-file name in the repo (default: voido-data.json)
//! # github_token = "…"        # usually omitted — resolved from `gh` or $GITHUB_TOKEN
//! theme = "catppuccin-mocha"  # pick live with ^t
//!
//! # custom themes, added to the ^t picker — see `crate::theme::ThemeSpec`
//! # [[themes]]
//! # name = "My Theme"
//! # accent = "#89b4fa"
//! # …
//! ```

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Data-file name used inside the sync repo when `github_file` isn't set.
pub const DEFAULT_SYNC_FILE: &str = "voido-data.json";

/// Paths tried, in order, when settings are imported from a repo that was named
/// without a file — the usual places a dotfiles repo keeps them.
pub const SETTINGS_CANDIDATES: &[&str] = &[
    "config.toml",
    "voido.toml",
    "voido/config.toml",
    ".config/voido/config.toml",
    "config/voido/config.toml",
    "config.json",
];

/// Paths tried, in order, when data is imported from a repo that was named
/// without a file: the name voido syncs under, then the usual hand-made spots.
/// A `github_file` override is tried ahead of these — see `App::data_candidates`.
pub const DATA_CANDIDATES: &[&str] = &[
    DEFAULT_SYNC_FILE,
    "data/voido-data.json",
    "voido/voido-data.json",
    "voido.json",
    "data.json",
];

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
    /// Weather location for the header / Overview line: a place name
    /// (`"Lisbon"`), a `"lat,lon"` pair, or `"auto"` (IP-based). Empty / unset
    /// disables it — no network call is made.
    #[serde(default)]
    pub weather: Option<String>,
    /// Temperature unit for `weather`: `"c"` (default) or `"f"`.
    #[serde(default)]
    pub weather_unit: Option<String>,
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

    /// Parse settings text fetched from somewhere other than the settings file
    /// — TOML, falling back to the older JSON shape so a `config.json` kept in a
    /// dotfiles repo still imports.
    pub fn parse(text: &str) -> Result<Self, String> {
        let toml_err = match toml::from_str::<Self>(text) {
            Ok(c) => return Ok(c),
            Err(e) => e,
        };
        serde_json::from_str::<Self>(text).map_err(|_| toml_err.to_string())
    }

    /// What adopting `incoming` would change, one line per key, for the import
    /// confirmation. Empty means the two are already equivalent.
    ///
    /// `github_token` is never imported — a token in a repo is either a leak or
    /// someone else's — so a token in `incoming` shows up as an explicit skip.
    pub fn import_diff(&self, incoming: &Self) -> Vec<String> {
        let mut out = Vec::new();
        let mut row = |key: &str, from: String, to: String| {
            if from != to {
                out.push(format!("{key:<12}  {from} → {to}"));
            }
        };
        let show = |v: &Option<String>| match v.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "(unset)".to_string(),
        };
        let storage = |c: &Self| match c.storage {
            StorageChoice::GitHub => "github".to_string(),
            StorageChoice::Local => "local".to_string(),
        };

        row("storage", storage(self), storage(incoming));
        row(
            "sync repo",
            show(&self.github_repo),
            show(&incoming.github_repo),
        );
        row(
            "sync file",
            show(&self.github_file),
            show(&incoming.github_file),
        );
        row("theme", show(&self.theme), show(&incoming.theme));
        row("weather", show(&self.weather), show(&incoming.weather));
        row(
            "weather unit",
            show(&self.weather_unit),
            show(&incoming.weather_unit),
        );
        row(
            "custom themes",
            format!("{}", self.themes.len()),
            format!("{}", incoming.themes.len()),
        );
        if incoming.github_token.is_some() {
            out.push("github_token in the file is ignored (yours is kept)".into());
        }
        out
    }

    /// Replace these settings with `incoming`, keeping the local `github_token`.
    pub fn apply_import(&mut self, incoming: Self) {
        let token = self.github_token.take();
        *self = incoming;
        self.github_token = token;
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
    /// The `voido` config directory, created if missing.
    fn dir() -> PathBuf {
        let mut dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        dir.push("voido");
        fs::create_dir_all(&dir).ok();
        dir
    }

    /// Path to the settings file (`config.toml`).
    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Read the settings file. `Ok(None)` means it doesn't exist yet (first run);
    /// `Err` means it exists but couldn't be read or parsed — the caller should
    /// stop rather than overwrite a file the user was editing.
    ///
    /// If there's no `config.toml` but an older `config.json` is present, it's
    /// parsed, rewritten as TOML, and the JSON kept as `config.json.bak`.
    pub fn load() -> Result<Option<Self>, String> {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(data) => toml::from_str(&data)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::migrate_from_json(),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// One-time conversion of a legacy `config.json` to `config.toml`.
    fn migrate_from_json() -> Result<Option<Self>, String> {
        let json = Self::dir().join("config.json");
        let data = match fs::read_to_string(&json) {
            Ok(d) => d,
            Err(_) => return Ok(None), // genuine first run
        };
        let config: Self =
            serde_json::from_str(&data).map_err(|e| format!("{}: {e}", json.display()))?;
        config.save()?;
        let _ = fs::rename(&json, json.with_extension("json.bak"));
        Ok(Some(config))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        fs::write(&path, self.to_toml()?).map_err(|e| e.to_string())
    }

    /// The settings file's full text: a commented guide to every key, then the
    /// current values. voido rewrites this on exit, so hand-added comments in the
    /// body don't survive — the header always does.
    fn to_toml(&self) -> Result<String, String> {
        let body = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        Ok(format!("{SETTINGS_HEADER}\n{body}"))
    }
}

/// Leading comment block written to `config.toml` on every save.
const SETTINGS_HEADER: &str = "\
# voido settings (TOML). Every key is optional — delete a line for its default.
# Press ^e in the app to edit this file; ^t to pick a theme; ^s to set up sync.
#
#   storage       \"local\" (default) or \"github\". \"github\" turns on data sync.
#   github_repo   \"owner/repo\", or a bare name (owner = the token's account).
#   github_file   data-file name inside the repo. Default: \"voido-data.json\".
#                 Subpaths like \"data/notes.json\" work.
#   github_token  Personal access token. Usually omit this — voido falls back to
#                 `gh auth token` and then $GITHUB_TOKEN.
#   theme         Colour theme slug, e.g. \"dracula\". Unknown -> default.
#   weather       Place name, \"lat,lon\", or \"auto\" (IP-based). Empty = off.
#   weather_unit  \"c\" (default) or \"f\".
#
# ^s fills in `storage` and `github_repo` for you. ^k -> \"Settings from GitHub\"
# replaces this file with one kept in a repo (your `github_token` is never
# imported, and voido shows the changes before applying them).
#
# Custom themes are appended as [[themes]] tables (all slots #rrggbb; `on_accent`
# optional, defaults to `bg`). The name is slugified for `theme`; reuse a
# built-in's slug to override it. See the README for the full list.
#
#   [[themes]]
#   name   = \"My Neon\"
#   accent = \"#ff00ff\"
#   green  = \"#00ff88\"
#   red    = \"#ff3355\"
#   yellow = \"#ffee00\"
#   blue   = \"#22ddff\"
#   text   = \"#e8e8ff\"
#   subtle = \"#7a7a99\"
#   border = \"#333355\"
#   sel_bg = \"#222244\"
#   bg     = \"#0a0a14\"
";

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
        let c: Config = toml::from_str(r#"storage = "github""#).unwrap();
        assert_eq!(c.storage, StorageChoice::GitHub);
        assert!(c.github_repo.is_none());
        assert_eq!(c.sync_file(), DEFAULT_SYNC_FILE);
    }

    #[test]
    fn parse_accepts_toml_and_legacy_json() {
        let toml = Config::parse(r#"theme = "dracula""#).unwrap();
        assert_eq!(toml.theme.as_deref(), Some("dracula"));

        let json = Config::parse(r#"{"theme":"dracula","storage":"github"}"#).unwrap();
        assert_eq!(json.theme.as_deref(), Some("dracula"));
        assert_eq!(json.storage, StorageChoice::GitHub);

        assert!(Config::parse("this is neither").is_err());
    }

    #[test]
    fn import_reports_changes_and_keeps_the_local_token() {
        let mut local = Config {
            theme: Some("dracula".into()),
            github_token: Some("local-secret".into()),
            ..Config::default()
        };
        let incoming = Config {
            storage: StorageChoice::GitHub,
            github_repo: Some("me/notes".into()),
            theme: Some("dracula".into()),
            weather: Some("Lisbon".into()),
            github_token: Some("theirs".into()),
            ..Config::default()
        };

        let diff = local.import_diff(&incoming);
        assert!(diff.iter().any(|l| l.starts_with("storage")));
        assert!(diff.iter().any(|l| l.contains("me/notes")));
        assert!(diff.iter().any(|l| l.contains("Lisbon")));
        assert!(
            !diff.iter().any(|l| l.starts_with("theme")),
            "unchanged keys are left out: {diff:?}"
        );
        assert!(
            diff.iter().any(|l| l.contains("github_token")),
            "a token in the file is called out as skipped"
        );

        local.apply_import(incoming);
        assert_eq!(local.github_token.as_deref(), Some("local-secret"));
        assert_eq!(local.github_repo.as_deref(), Some("me/notes"));
        assert_eq!(local.weather.as_deref(), Some("Lisbon"));
        assert_eq!(local.storage, StorageChoice::GitHub);
    }

    #[test]
    fn import_diff_is_empty_for_identical_settings() {
        let c = Config {
            theme: Some("dracula".into()),
            weather: Some("Lisbon".into()),
            ..Config::default()
        };
        assert!(c.import_diff(&c.clone()).is_empty());
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut c = Config {
            storage: StorageChoice::GitHub,
            github_repo: Some("me/notes".into()),
            theme: Some("dracula".into()),
            ..Config::default()
        };
        c.themes.push(crate::theme::ThemeSpec {
            name: "Mine".into(),
            slug: None,
            accent: "#89b4fa".into(),
            green: "#a6e3a1".into(),
            red: "#f38ba8".into(),
            yellow: "#f9e2af".into(),
            blue: "#89b4fa".into(),
            text: "#cdd6f4".into(),
            subtle: "#9399b2".into(),
            border: "#45475a".into(),
            sel_bg: "#313244".into(),
            bg: "#1e1e2e".into(),
            on_accent: None,
        });
        let text = c.to_toml().unwrap();
        assert!(text.starts_with("# voido settings"), "keeps the comment header");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back.storage, StorageChoice::GitHub);
        assert_eq!(back.github_repo.as_deref(), Some("me/notes"));
        assert_eq!(back.theme.as_deref(), Some("dracula"));
        assert_eq!(back.themes.len(), 1);
        assert_eq!(back.themes[0].name, "Mine");
        // Omitted optionals stay omitted, not serialised as empty strings.
        assert!(back.github_token.is_none());
        assert!(
            !text.lines().any(|l| l.trim_start().starts_with("github_token =")),
            "no active github_token key"
        );
    }
}
