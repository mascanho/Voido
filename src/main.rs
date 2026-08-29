//! shiki — a keyboard-first TUI for todos, projects and timelines.

mod app;
mod config;
mod github;
mod md;
mod model;
mod storage;
mod storage_sqlite;
mod ui;

use std::error::Error;
use std::path::PathBuf;

use app::App;
use config::{Config, StorageChoice};
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyEventKind},
};

fn main() -> Result<(), Box<dyn Error>> {
    let config = ensure_config()?;
    let store = load_store(&config)?;
    let mut terminal = ratatui::init();
    let mut app = App::new(store);
    let result = run(&mut terminal, &mut app, &config);
    ratatui::restore();

    if let Err(e) = save_store(&app.store, &config) {
        eprintln!("shiki: could not save data: {e}");
    }
    result
}

fn ensure_config() -> Result<Config, Box<dyn Error>> {
    if let Some(config) = Config::load() {
        return Ok(config);
    }

    // First launch — prompt user
    println!();
    println!("  Welcome to shiki!");
    println!();
    println!("  Would you like to sync your data with GitHub?");
    println!("  (your projects will be stored in a hidden repo called 'voido-data')");
    println!();
    print!("  Use GitHub? [y/N]: ");

    use std::io::Write;
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim().to_lowercase();

    let config = if input == "y" || input == "yes" {
        println!();
        print!("  Enter your GitHub repo (owner/repo) or press Enter for 'voido-data': ");
        std::io::stdout().flush()?;

        let mut repo = String::new();
        std::io::stdin().read_line(&mut repo)?;
        let repo = repo.trim().to_string();
        let repo = if repo.is_empty() {
            "voido-data".to_string()
        } else {
            repo
        };

        Config {
            storage: StorageChoice::GitHub,
            github_repo: Some(repo),
        }
    } else {
        Config {
            storage: StorageChoice::Local,
            github_repo: None,
        }
    };

    config.save()?;
    println!();
    println!("  Config saved. Starting shiki...");
    println!();

    Ok(config)
}

fn db_path() -> PathBuf {
    let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push("shiki");
    std::fs::create_dir_all(&dir).ok();
    dir.push("shiki.db");
    dir
}

fn load_store(config: &Config) -> Result<model::Store, Box<dyn Error>> {
    match config.storage {
        StorageChoice::Local => {
            // Try SQLite first, fall back to JSON
            let path = db_path();
            if path.exists() {
                let sqlite = storage_sqlite::SqliteStorage::open(&path)?;
                Ok(sqlite.load())
            } else {
                // Try loading from legacy JSON
                Ok(storage::load())
            }
        }
        StorageChoice::GitHub => {
            let path = db_path();
            if path.exists() {
                let sqlite = storage_sqlite::SqliteStorage::open(&path)?;
                Ok(sqlite.load())
            } else {
                Ok(storage::load())
            }
        }
    }
}

fn save_store(store: &model::Store, config: &Config) -> Result<(), Box<dyn Error>> {
    match config.storage {
        StorageChoice::Local => {
            let path = db_path();
            let sqlite = storage_sqlite::SqliteStorage::open(&path)?;
            sqlite.save(store)?;
        }
        StorageChoice::GitHub => {
            let path = db_path();
            let sqlite = storage_sqlite::SqliteStorage::open(&path)?;
            sqlite.save(store)?;
            // TODO: Phase 3 — push to GitHub
        }
    }
    Ok(())
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    _config: &Config,
) -> Result<(), Box<dyn Error>> {
    while !app.should_quit {
        app.clamp();
        terminal.draw(|frame| ui::render(frame, app))?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }

        if app.dirty {
            if let Err(e) = save_store(&app.store, _config) {
                app.status = format!("save error: {e}");
            }
            app.dirty = false;
        }
    }
    Ok(())
}
