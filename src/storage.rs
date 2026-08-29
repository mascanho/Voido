//! Legacy JSON store: read-only now, kept so existing `data.json` files are
//! imported into SQLite on first run.

use std::fs;
use std::path::PathBuf;

use crate::model::Store;

pub fn data_path() -> PathBuf {
    let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push("voido");
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
