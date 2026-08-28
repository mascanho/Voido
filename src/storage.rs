//! Load/save the store as pretty JSON under the platform data directory.

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::model::Store;

pub fn data_path() -> PathBuf {
    let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push("shiki");
    dir.push("data.json");
    dir
}

/// Load the store. Missing file -> seeded sample data. Corrupt file -> empty store.
pub fn load() -> Store {
    match fs::read_to_string(data_path()) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => Store::sample(),
    }
}

pub fn save(store: &Store) -> io::Result<()> {
    let path = data_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(store).map_err(io::Error::other)?;
    fs::write(path, json)
}
