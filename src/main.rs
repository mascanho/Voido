//! voido — a keyboard-first TUI for todos, projects and timelines.

mod app;
mod config;
mod github;
mod md;
mod model;
mod storage;
mod storage_sqlite;
mod theme;
mod ui;
mod util;
mod weather;

use std::error::Error;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use app::App;
use config::Config;
use ratatui::{
    DefaultTerminal,
    crossterm::{
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
        execute,
    },
};
use storage_sqlite::SqliteStorage;

fn main() -> Result<(), Box<dyn Error>> {
    let config = ensure_config()?;
    for warning in theme::install_custom(&config.themes) {
        eprintln!("voido: {warning}");
    }
    theme::set_slug(config.theme.as_deref());

    let db = match SqliteStorage::open(&db_path()) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("voido: could not open the database: {e}");
            eprintln!(
                "voido: the file at {} may be corrupt — move it aside to start fresh.",
                db_path().display()
            );
            return Err(e.into());
        }
    };

    let mut store = match load_store(&db) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("voido: could not load your data: {e}");
            eprintln!(
                "voido: the database at {} may be corrupt — move it aside to start fresh.",
                db_path().display()
            );
            return Err(e.into());
        }
    };

    // Resolve a token once (config value, then `gh` CLI, then environment).
    let token = github::resolve_token(config.github_token.as_deref());

    // Pull the latest from GitHub before the UI opens, when sync is set up.
    let mut sync_sha = None;
    let mut startup_log: Vec<String> = vec![format!("voido {}", env!("CARGO_PKG_VERSION"))];
    if config.sync_configured() {
        if let Some(token) = token.as_deref() {
            eprint!("voido: syncing with GitHub… ");
            let _ = std::io::stderr().flush();
            match sync_pull(&config, token) {
                Ok(Some((remote, sha))) => {
                    store = remote;
                    if let Err(e) = db.save(&store) {
                        eprintln!("(local save failed: {e})");
                        startup_log.push(format!("startup pull: local save failed ({e})"));
                    } else {
                        eprintln!("pulled latest.");
                        startup_log.push("pulled latest from GitHub".into());
                    }
                    sync_sha = Some(sha);
                }
                Ok(None) => {
                    eprintln!("no data in the repo yet — it will be created on exit.");
                    startup_log.push("GitHub repo empty — will be created on exit".into());
                }
                Err(e) => {
                    eprintln!("pull failed ({e}) — starting from local data.");
                    startup_log.push(format!("startup pull failed: {e}"));
                }
            }
        } else {
            eprintln!(
                "voido: GitHub sync is on but no token was found — run `gh auth login`, set \
                 GITHUB_TOKEN, or press ^s in the app."
            );
            startup_log.push("GitHub sync on but no token found".into());
        }
    }

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let mut app = App::new(store, config, sync_sha, token);
    for line in startup_log {
        app.push_log(line);
    }
    let result = run(&mut terminal, &mut app, &db);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();

    // Best-effort final local save, even if the loop bailed with an error.
    if let Err(e) = db.save(&app.store) {
        eprintln!("voido: could not save data: {e}");
    }

    // Persist any settings changed in-app (e.g. sync just configured).
    let _ = app.config.save();

    // Push to GitHub on the way out.
    if app.sync_ready() {
        let token = app.sync_token.clone().unwrap_or_default();
        match sync_push(&app.config, &token, &app.store, app.sync_sha.as_deref()) {
            Ok(_) => eprintln!("voido: synced with GitHub."),
            Err(e) => eprintln!("voido: GitHub push failed: {e}"),
        }
    }

    result
}

fn ensure_config() -> Result<Config, Box<dyn Error>> {
    match Config::load() {
        Ok(Some(config)) => return Ok(config),
        Ok(None) => {}
        Err(e) => {
            eprintln!("voido: your settings file couldn't be read — fix or delete it:");
            eprintln!("voido:   {e}");
            return Err(e.into());
        }
    }

    // First run: start local. GitHub sync is a one-keystroke opt-in from inside
    // the app (`^s`), which auto-detects a `gh` login and creates the repo.
    println!();
    println!("  Welcome to voido — your data lives locally at");
    println!("    {}", db_path().display());
    println!();
    println!("  Settings (repo, data-file name, token) are in");
    println!("    {}", Config::path().display());
    println!();
    println!("  To back it up to GitHub, press ^s in the app.");
    println!();

    let config = Config::default();
    config.save()?;
    Ok(config)
}

fn db_path() -> PathBuf {
    let mut dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    dir.push("voido");
    std::fs::create_dir_all(&dir).ok();
    dir.push("voido.db");
    dir
}

/// Load from SQLite, importing a legacy `data.json` on first run when the
/// database is still empty.
fn load_store(db: &SqliteStorage) -> Result<model::Store, String> {
    let store = db.load()?;
    if store.projects.is_empty() {
        let legacy = storage::load();
        if !legacy.projects.is_empty() {
            db.save(&legacy)?;
            return Ok(legacy);
        }
        return Ok(legacy); // seeded sample data
    }
    Ok(store)
}

/// Fetch the synced store from the configured repo. `Ok(None)` = repo reachable
/// but no data file yet.
fn sync_pull(config: &Config, token: &str) -> Result<Option<(model::Store, String)>, String> {
    let client = sync_client(config, token)?;
    match client.pull()? {
        Some(remote) => {
            let store: model::Store = serde_json::from_str(&remote.json)
                .map_err(|e| format!("remote data is unreadable: {e}"))?;
            Ok(Some((store, remote.sha)))
        }
        None => Ok(None),
    }
}

fn sync_push(
    config: &Config,
    token: &str,
    store: &model::Store,
    sha: Option<&str>,
) -> Result<String, String> {
    let client = sync_client(config, token)?;
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    match client.push(&json, sha) {
        Ok(s) => Ok(s),
        // File exists / changed since we pulled — refetch its SHA and overwrite.
        Err(e)
            if sha.is_none()
                || e.contains("409")
                || e.contains("422")
                || e.contains("conflict") =>
        {
            let latest = client.pull()?.map(|r| r.sha);
            client.push(&json, latest.as_deref())
        }
        Err(e) => Err(e),
    }
}

fn sync_client(config: &Config, token: &str) -> Result<github::SyncClient, String> {
    let repo = config
        .github_repo
        .as_deref()
        .ok_or_else(|| "no sync repo configured".to_string())?;
    let (owner, name) = match github::parse_repo_string(repo) {
        Some(pair) => pair,
        // A bare name — resolve the owner from the token's account.
        None => (
            github::authed_login(token)?,
            repo.trim().trim_matches('/').to_string(),
        ),
    };
    github::SyncClient::new(token, &owner, &name, &config.sync_file())
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    db: &SqliteStorage,
) -> Result<(), Box<dyn Error>> {
    app.clamp();
    terminal.draw(|frame| ui::render(frame, app))?;

    while !app.should_quit {
        let mut redraw = false;

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key);
                    redraw = true;
                }
                Event::Mouse(me) => {
                    if app.handle_mouse(me) {
                        redraw = true;
                    }
                }
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }

        // `^e` — drop out of the TUI, run the user's editor on the settings
        // file, then reload it.
        if app.open_settings {
            app.open_settings = false;
            match edit_settings(terminal) {
                Ok(()) => match Config::load() {
                    Ok(Some(cfg)) => {
                        app.reload_config(cfg);
                        app.toast(app::ToastKind::Info, "Settings reloaded");
                    }
                    Ok(None) => {}
                    Err(e) => app.toast(app::ToastKind::Error, format!("Settings: {e}")),
                },
                Err(e) => app.toast(app::ToastKind::Error, e),
            }
            redraw = true;
        }

        // Kick off / pick up background work (GitHub fetches, data syncs, weather).
        app.maybe_refresh_weather();
        if app.poll_background() {
            redraw = true;
        }

        let mut saved = false;
        if app.dirty {
            if let Err(e) = db.save(&app.store) {
                app.status = format!("save error: {e}");
            }
            app.dirty = false;
            app.note_unsynced_edit();
            saved = true;
            redraw = true;
        }

        // Fold this pass's status message into the ^l activity panel.
        app.record_activity(saved);

        // Expire the transient toast, if any.
        if app.tick_toast() {
            redraw = true;
        }

        if redraw {
            app.clamp();
            terminal.draw(|frame| ui::render(frame, app))?;
        }
    }
    Ok(())
}

/// Suspend the TUI, open the settings file in `$VISUAL` / `$EDITOR` (falling
/// back to `vi`), then restore the terminal. `EDITOR` may include arguments,
/// e.g. `code -w`.
fn edit_settings(terminal: &mut DefaultTerminal) -> Result<(), String> {
    let path = Config::path();
    let spec = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let mut parts = spec.split_whitespace();
    let Some(program) = parts.next() else {
        return Err("no editor set — set $EDITOR".into());
    };
    let args: Vec<&str> = parts.collect();

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();

    let status = std::process::Command::new(program)
        .args(&args)
        .arg(&path)
        .status();

    *terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let _ = terminal.clear();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{program} exited without saving ({s})")),
        Err(e) => Err(format!("could not run {program}: {e}")),
    }
}
