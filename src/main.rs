//! shiki — a keyboard-first TUI for todos, projects and timelines.

mod app;
mod md;
mod model;
mod storage;
mod ui;

use std::error::Error;

use app::App;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyEventKind},
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut terminal = ratatui::init();
    let mut app = App::new(storage::load());
    let result = run(&mut terminal, &mut app);
    ratatui::restore();

    if let Err(e) = storage::save(&app.store) {
        eprintln!("shiki: could not save data: {e}");
    }
    result
}

fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<(), Box<dyn Error>> {
    while !app.should_quit {
        app.clamp();
        terminal.draw(|frame| ui::render(frame, app))?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }

        if app.dirty {
            storage::save(&app.store)?;
            app.dirty = false;
        }
    }
    Ok(())
}
