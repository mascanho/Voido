//! Configuration file for shiki.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub storage: StorageChoice,
    #[serde(default)]
    pub github_repo: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum StorageChoice {
    GitHub,
    Local,
}

impl Default for StorageChoice {
    fn default() -> Self {
        StorageChoice::Local
    }
}

impl Config {
    pub fn path() -> PathBuf {
        let mut dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        dir.push("shiki");
        fs::create_dir_all(&dir).ok();
        dir.push("config.json");
        dir
    }

    pub fn load() -> Option<Self> {
        let path = Self::path();
        let data = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        let data = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&path, data).map_err(|e| e.to_string())
    }

    pub fn is_first_launch() -> bool {
        !Self::path().exists()
    }
}
