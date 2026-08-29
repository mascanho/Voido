//! Rendering. A three-row shell (header / body / status) with a two-pane body.

use chrono::{Local, NaiveDate};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
    },
};

use crate::app::{
    App, ConfirmState, Focus, InputState, Mode, PaneRects, Tab, ThemeState, TimelineEntry, TlKind,
};
use crate::model::Priority;
use crate::theme::{accent, bg, blue, border, green, on_accent, red, sel_bg, subtle, text, yellow};
use crate::util::truncate;

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    // Paint the themed background; everything below draws on top of it.
    f.render_widget(Block::default().style(Style::new().bg(bg())), area);
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_header(f, rows[0], app);

    let mut rects = PaneRects::default();

    if app.tab == Tab::Todos || app.tab == Tab::Notes {
        if app.tab == Tab::Notes && app.note_expanded && app.focus == Focus::Detail {
            // Full-width note mode — projects + note body (or its editor) only.
            let body = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(rows[1]);
            rects.projects = body[0];
            rects.detail = body[1];
            render_projects(f, body[0], app);
            if let Mode::EditBody(state) = &app.mode {
                render_edit_body_pane(f, body[1], state, app);
            } else {
                render_note_body(f, body[1], app);
            }
        } else {
            let body = Layout::horizontal([
                Constraint::Percentage(30),
                Constraint::Percentage(30),
                Constraint::Percentage(40),
            ])
            .split(rows[1]);
            rects.projects = body[0];
            rects.content = body[1];
            rects.detail = body[2];
            render_projects(f, body[0], app);
            render_content(f, body[1], app);
            if app.tab == Tab::Todos {
                render_subtasks(f, body[2], app);
            } else if let Mode::EditBody(state) = &app.mode {
                render_edit_body_pane(f, body[2], state, app);
            } else {
                render_note_body(f, body[2], app);
            }
        }
    } else {
        let body = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(rows[1]);
        rects.projects = body[0];
        rects.content = body[1];
        render_projects(f, body[0], app);
        render_content(f, body[1], app);
    }

    *app.pane_rects.borrow_mut() = rects;

    render_footer(f, rows[2], app);

    match &app.mode {
        Mode::Input(input) => render_input(f, area, input),
        Mode::Confirm(c) => render_confirm(f, area, c),
        Mode::Help => render_help(f, area),
        Mode::GitHub => render_github(f, area, app),
        Mode::Theme(state) => render_theme(f, area, state),
        Mode::Notice(title, body) => render_notice(f, area, title, body),
        Mode::EditBody(_) => {}
        Mode::Normal => {}
    }
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(13)]).split(area);
    let n = app.store.projects.len();
    let done = app
        .store
        .projects
        .iter()
        .filter(|p| p.is_complete())
        .count();
    let count = if done > 0 {
        format!(
            "  ·  {n} project{}  ·  {done} done",
            if n == 1 { "" } else { "s" }
        )
    } else {
        format!("  ·  {n} project{}", if n == 1 { "" } else { "s" })
    };
    let spans = vec![
        Span::styled("  voido", Style::new().fg(accent()).bold()),
        Span::styled(count, Style::new().fg(subtle())),
    ];
    let title = Line::from(spans);
    let date = Line::from(Span::styled(
        format!("{}  ", Local::now().date_naive()),
        Style::new().fg(subtle()),
    ))
    .right_aligned();
    f.render_widget(Paragraph::new(title), cols[0]);
    f.render_widget(Paragraph::new(date), cols[1]);
}

fn render_projects(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Projects;
    let block = panel(" Projects ".to_string(), focused);

    if app.store.projects.is_empty() {
        f.render_widget(
            hint("No projects yet.  Press a to create one.").block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = app
        .store
        .projects
        .iter()
        .map(|p| {
            let open = p.open_todos();
            let (dot, name_style, tail) = if p.is_complete() {
                (
                    Span::styled("✔ ", Style::new().fg(green())),
                    Style::new()
                        .fg(subtle())
                        .add_modifier(Modifier::CROSSED_OUT),
                    Span::raw(""),
                )
            } else if open == 0 {
                let tail = if p.todos.is_empty() {
                    Span::raw("")
                } else {
                    Span::styled("  clear", Style::new().fg(subtle()))
                };
                (
                    Span::styled("● ", Style::new().fg(green())),
                    Style::new().fg(text()),
                    tail,
                )
            } else {
                (
                    Span::styled("● ", Style::new().fg(accent())),
                    Style::new().fg(text()),
                    Span::styled(format!("  {open}"), Style::new().fg(subtle())),
                )
            };
            ListItem::new(Line::from(vec![
                dot,
                Span::styled(p.name.clone(), name_style),
                tail,
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(sel_bg()).fg(accent()).bold()
        } else {
            Style::new().bg(sel_bg())
        })
        .highlight_symbol(if focused { "▍" } else { " " });
    let mut state = ListState::default();
    state.select(Some(app.project_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_content(f: &mut Frame, area: Rect, app: &App) {
    match app.tab {
        Tab::Overview => render_overview(f, area, app),
        Tab::Todos => render_todos(f, area, app),
        Tab::Notes => render_notes(f, area, app),
        Tab::Schedule => render_timeline(f, area, app),
    }
}

/// The bordered block for the middle pane. Its top border carries the tab strip
/// (left) and the project name (right), so no separate tab row is needed.
fn content_block(app: &App) -> Block<'static> {
    let focused = app.focus == Focus::Content;

    let mut spans = Vec::new();
    for t in Tab::ALL {
        let active = t == app.tab;
        let style = if active {
            Style::new().fg(accent()).bold().bg(sel_bg())
        } else if focused {
            Style::new().fg(subtle())
        } else {
            Style::new().fg(border())
        };
        spans.push(Span::styled(format!(" {} ", t.title()), style));
    }
    let tabs = Line::from(spans);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { accent() } else { border() }))
        .title(tabs);

    if let Some(p) = app.current_project() {
        block = block.title(
            Line::from(Span::styled(
                format!(" {} ", truncate(&p.name, 28)),
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        );
    }
    block
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    let block = content_block(app).padding(Padding::horizontal(1));

    let Some(p) = app.current_project() else {
        f.render_widget(hint("No project selected.").block(block), area);
        return;
    };

    let today = Local::now().date_naive();
    let total = p.todos.len();
    let done = p.done_todos();
    let subs: usize = p.todos.iter().map(|t| t.subtasks.len()).sum();
    let subs_done: usize = p
        .todos
        .iter()
        .flat_map(|t| &t.subtasks)
        .filter(|s| s.done)
        .count();

    let name_line = if p.is_complete() {
        Line::from(vec![
            Span::styled(p.name.clone(), Style::new().fg(green()).bold()),
            Span::styled("   ✔ done", Style::new().fg(green())),
        ])
    } else {
        Line::from(Span::styled(p.name.clone(), Style::new().fg(text()).bold()))
    };

    let mut lines = vec![
        name_line,
        Line::from(Span::styled(
            if p.description.is_empty() {
                "no description — press e to add one".to_string()
            } else {
                p.description.clone()
            },
            Style::new().fg(subtle()),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Todos     ", Style::new().fg(subtle())),
            Span::styled(format!("{done}/{total} done"), Style::new().fg(text())),
        ]),
        Line::from(vec![
            Span::styled("          ", Style::new().fg(subtle())),
            Span::styled(progress_bar(done, total, 22), Style::new().fg(green())),
        ]),
    ];

    if subs > 0 {
        lines.push(Line::from(vec![
            Span::styled("Subtasks  ", Style::new().fg(subtle())),
            Span::styled(format!("{subs_done}/{subs} done"), Style::new().fg(text())),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Notes     ", Style::new().fg(subtle())),
        Span::styled(format!("{}", p.notes.len()), Style::new().fg(text())),
    ]));
    lines.push(Line::from(""));

    match p.next_milestone() {
        Some(m) => lines.push(Line::from(vec![
            Span::styled("Next      ", Style::new().fg(subtle())),
            Span::styled("◆ ", Style::new().fg(accent())),
            Span::styled(m.title.clone(), Style::new().fg(text())),
            Span::styled(
                format!("  {} · {}", m.date.format("%Y-%m-%d"), rel(m.date, today)),
                Style::new().fg(subtle()),
            ),
        ])),
        None => lines.push(Line::from(vec![
            Span::styled("Next      ", Style::new().fg(subtle())),
            Span::styled("no upcoming milestones", Style::new().fg(subtle())),
        ])),
    }

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(block),
        area,
    );
}

fn render_notes(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Notes;
    let block = content_block(app);

    let Some(project) = app.current_project() else {
        f.render_widget(hint("Select a project to keep notes.").block(block), area);
        return;
    };
    if project.notes.is_empty() {
        f.render_widget(
            hint("No notes yet.  Press a to jot one down · x pins it.").block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = project
        .notes
        .iter()
        .map(|n| {
            let (mark, mark_style) = if n.pinned {
                ("★ ", Style::new().fg(yellow()))
            } else {
                ("• ", Style::new().fg(subtle()))
            };
            let mut spans = vec![
                Span::styled(mark, mark_style),
                Span::styled(n.text.clone(), Style::new().fg(text())),
            ];
            if !n.body.trim().is_empty() {
                spans.push(Span::styled(" ¶", Style::new().fg(subtle())));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(sel_bg()).fg(accent()).bold()
        } else {
            Style::new().bg(sel_bg())
        })
        .highlight_symbol(if focused { "▍" } else { " " });
    let mut state = ListState::default();
    state.select(Some(app.note_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_todos(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Todos;
    let block = content_block(app);

    let Some(project) = app.current_project() else {
        f.render_widget(
            hint("Create a project to start adding todos.").block(block),
            area,
        );
        return;
    };
    if project.todos.is_empty() {
        f.render_widget(
            hint("No todos yet.  Press a to add one.").block(block),
            area,
        );
        return;
    }

    let today = Local::now().date_naive();
    let items: Vec<ListItem> = project
        .todos
        .iter()
        .map(|t| {
            let (mark, mark_style) = if t.done {
                ("✔ ", Style::new().fg(green()))
            } else {
                ("○ ", Style::new().fg(subtle()))
            };
            let title_style = if t.done {
                Style::new()
                    .fg(subtle())
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::new().fg(text())
            };
            let mut spans = vec![
                Span::styled(mark, mark_style),
                Span::styled(t.title.clone(), title_style),
                Span::styled(format!("  {}", t.priority.label()), prio_style(t.priority)),
            ];
            if let Some(d) = t.due {
                let style = if !t.done && d < today {
                    Style::new().fg(red()).bold()
                } else {
                    Style::new().fg(subtle())
                };
                spans.push(Span::styled(format!("  {}", d.format("%b %d")), style));
            }
            let (sdone, stotal) = t.subtask_progress();
            if stotal > 0 {
                let style = if sdone == stotal {
                    Style::new().fg(green())
                } else {
                    Style::new().fg(subtle())
                };
                spans.push(Span::styled(format!("  ⊞ {sdone}/{stotal}"), style));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(sel_bg()).bold()
        } else {
            Style::new().bg(sel_bg())
        })
        .highlight_symbol(if focused { "▍" } else { " " });
    let mut state = ListState::default();
    state.select(Some(app.todo_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_subtasks(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Detail;

    let Some(todo) = app.current_todo() else {
        let block = panel(" Subtasks ".to_string(), focused);
        f.render_widget(
            hint("Pick a todo, press l for subtasks.").block(block),
            area,
        );
        return;
    };

    let parent_name = truncate(&todo.title, 24);
    let (done, total) = todo.subtask_progress();
    let title = if total > 0 {
        format!(" Subtasks · {parent_name}  {done}/{total} ")
    } else {
        format!(" Subtasks · {parent_name} ")
    };
    let block = panel(title, focused);

    if todo.subtasks.is_empty() {
        f.render_widget(
            hint("No subtasks yet — press a to add one.").block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = todo
        .subtasks
        .iter()
        .map(|s| {
            let (mark, mark_style) = if s.done {
                ("✔ ", Style::new().fg(green()))
            } else {
                ("○ ", Style::new().fg(subtle()))
            };
            let title_style = if s.done {
                Style::new()
                    .fg(subtle())
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::new().fg(text())
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, mark_style),
                Span::styled(s.title.clone(), title_style),
                Span::styled(format!("  {}", s.priority.label()), prio_style(s.priority)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(sel_bg()).bold()
        } else {
            Style::new().bg(sel_bg())
        })
        .highlight_symbol(if focused { "▍" } else { " " });
    let mut state = ListState::default();
    state.select(Some(app.subtask_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_note_body(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Detail;

    let Some(note) = app.current_note() else {
        let block = panel(" Note ".to_string(), focused);
        f.render_widget(
            hint("Pick a note and press l to read it.").block(block),
            area,
        );
        return;
    };

    let block = panel(format!(" Note · {} ", truncate(&note.text, 24)), focused)
        .padding(Padding::horizontal(1));

    if note.body.trim().is_empty() {
        f.render_widget(
            hint("Empty note.  Press e to write it in Markdown.").block(block),
            area,
        );
        return;
    }

    let rendered = app.note_body_lines(&note.body, area.width.saturating_sub(4));
    let total = rendered.len() as u16;
    let view = area.height.saturating_sub(2).max(1);
    let max_scroll = total.saturating_sub(view);
    let scroll = app.note_scroll.min(max_scroll);

    let para = Paragraph::new(rendered)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(para, area);

    if total > view {
        let pct = if max_scroll == 0 {
            100
        } else {
            (scroll as u32 * 100 / max_scroll as u32) as u16
        };
        let tag = format!(" {pct}% ");
        let w = tag.chars().count() as u16;
        if area.width > w + 2 {
            let r = Rect {
                x: area.x + area.width - w - 1,
                y: area.y,
                width: w,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Span::styled(tag, Style::new().fg(subtle()))),
                r,
            );
        }
    }
}

fn render_edit_body_pane(f: &mut Frame, area: Rect, state: &crate::app::EditState, app: &App) {
    let title = app
        .current_note()
        .map(|n| truncate(&n.text, 40))
        .unwrap_or_else(|| "note".into());

    let split = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            format!(" edit · {title} "),
            Style::new().fg(accent()).bold(),
        ))
        .padding(Padding::horizontal(1));

    let mut ta = state.textarea.clone();
    ta.set_block(block);
    ta.set_cursor_line_style(Style::new());
    ta.set_selection_style(Style::new().bg(sel_bg()));
    ta.set_line_number_style(Style::new().fg(border()));
    f.render_widget(&ta, split[0]);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  esc", Style::new().fg(accent()).bold()),
            Span::styled(" / ", Style::new().fg(subtle())),
            Span::styled("^s", Style::new().fg(accent()).bold()),
            Span::styled(
                "  save & close     markdown: # heading · - list · **bold** · `code` · > quote",
                Style::new().fg(subtle()),
            ),
        ])),
        split[1],
    );
}

fn render_timeline(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Schedule;
    let block = content_block(app);

    let (total, done, _overdue, _today_count, _this_week) = app.deadline_stats();

    if total == 0 {
        f.render_widget(
            hint("Nothing scheduled.  Press a to add a milestone, or give a todo a due date.")
                .block(block),
            area,
        );
        return;
    }

    let today = Local::now().date_naive();
    let entries = app.timeline();

    let make_item = |e: &TimelineEntry| -> ListItem {
        let (icon, icon_style) = match e.kind {
            TlKind::Milestone => ("◆ ", Style::new().fg(accent())),
            TlKind::Todo if e.done => ("✔ ", Style::new().fg(green())),
            TlKind::Todo => ("○ ", Style::new().fg(subtle())),
        };
        let date_style = if !e.done && e.date < today {
            Style::new().fg(red())
        } else if e.date == today {
            Style::new().fg(green())
        } else {
            Style::new().fg(blue())
        };
        let label_style = if e.done {
            Style::new()
                .fg(subtle())
                .add_modifier(Modifier::CROSSED_OUT)
        } else {
            Style::new().fg(text())
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{}  ", e.date.format("%Y-%m-%d")), date_style),
            Span::styled(icon, icon_style),
            Span::styled(e.label.clone(), label_style),
            Span::styled(
                format!("   {}", rel(e.date, today)),
                Style::new().fg(subtle()),
            ),
        ]))
    };

    let items: Vec<ListItem> = entries.iter().map(make_item).collect();

    // Progress bar in top-right
    if area.width > 40 {
        let pct = if total == 0 { 0 } else { (done * 100) / total };
        let bar_text = format!(" {done}/{total} {pct}% ");
        let bw = bar_text.chars().count() as u16;
        if area.width > bw + 2 {
            let r = Rect {
                x: area.x + area.width - bw - 2,
                y: area.y,
                width: bw,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Span::styled(bar_text, Style::new().fg(subtle()))),
                r,
            );
        }
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(sel_bg()).bold()
        } else {
            Style::new().bg(sel_bg())
        })
        .highlight_symbol(if focused { "▍" } else { " " });
    let mut state = ListState::default();
    state.select(Some(app.timeline_idx));
    f.render_stateful_widget(list, area, &mut state);
}

/// The most useful keys for wherever focus currently is.
fn context_hints(app: &App) -> &'static str {
    match (app.focus, app.tab) {
        (Focus::Projects, _) => "a add · r rename · l open · o github · ^g link · ^y sync",
        (Focus::Content, Tab::Overview) => "e description · r rename",
        (Focus::Content, Tab::Todos) => {
            "a add · e edit · x done · p priority · J/K move · l subtasks · d delete"
        }
        (Focus::Content, Tab::Notes) => "a add · e title · x pin · J/K move · l open · d delete",
        (Focus::Content, Tab::Schedule) => {
            "a milestone · x done · r reschedule · f filter · l jump · d delete"
        }
        (Focus::Detail, Tab::Notes) => "j/k scroll · ^d/^u page · e edit · space expand · h back",
        (Focus::Detail, _) => "a add · e edit · x done · p priority · J/K move · d del · h back",
    }
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    // vim-style mode + location segments on the left.
    let (mode_label, mode_color) = match &app.mode {
        Mode::Input(_) | Mode::EditBody(_) => ("INSERT", green()),
        Mode::Confirm(_) => ("CONFIRM", red()),
        Mode::Notice(..) => ("NOTICE", red()),
        Mode::Normal | Mode::Help | Mode::GitHub | Mode::Theme(_) => ("NORMAL", accent()),
    };
    let pane_label = match app.focus {
        Focus::Projects => "PROJECTS",
        Focus::Content => match app.tab {
            Tab::Overview => "OVERVIEW",
            Tab::Todos => "TODOS",
            Tab::Notes => "NOTES",
            Tab::Schedule => "SCHEDULE",
        },
        Focus::Detail => match app.tab {
            Tab::Notes => "NOTE",
            _ => "SUBTASKS",
        },
    };

    let mut spans = vec![
        Span::styled(
            format!(" {mode_label} "),
            Style::new().fg(on_accent()).bg(mode_color).bold(),
        ),
        Span::styled(
            format!(" {pane_label} "),
            Style::new().fg(mode_color).bg(sel_bg()).bold(),
        ),
    ];
    // GitHub sync state, right after the pane segment. Nerd Font glyphs:
    // \u{f09b} github ·  \u{f021} sync-arrows ·  \u{f0ee} cloud-upload ·  \u{f00c} check.
    if app.sync_in_flight {
        spans.push(Span::styled(
            " \u{f021} ",
            Style::new().fg(on_accent()).bg(yellow()).bold(),
        ));
    } else if app.sync_ready() {
        spans.push(Span::styled(
            " \u{f09b} ",
            Style::new().fg(green()).bg(sel_bg()).bold(),
        ));
        if app.sync_pending > 0 {
            spans.push(Span::styled(
                format!(" \u{f0ee} {} ", app.sync_pending),
                Style::new().fg(yellow()).bg(sel_bg()),
            ));
        } else if let Some(t) = app.last_sync {
            spans.push(Span::styled(
                format!(" \u{f00c} {} ", rel_time(t)),
                Style::new().fg(subtle()).bg(sel_bg()),
            ));
        }
    }
    spans.push(Span::raw("  "));
    match &app.mode {
        Mode::Input(_) => spans.push(Span::styled(
            "enter  save    esc  cancel",
            Style::new().fg(subtle()),
        )),
        Mode::Confirm(_) => spans.push(Span::styled(
            "y  confirm    n  cancel",
            Style::new().fg(subtle()),
        )),
        Mode::EditBody(_) => spans.push(Span::styled(
            "esc / ^s  save & close",
            Style::new().fg(subtle()),
        )),
        Mode::Theme(_) => spans.push(Span::styled(
            "j / k  preview    enter  apply    esc  cancel",
            Style::new().fg(subtle()),
        )),
        Mode::Help | Mode::GitHub | Mode::Notice(..) => {
            spans.push(Span::styled("any key  close", Style::new().fg(subtle())))
        }
        Mode::Normal => {
            if !app.status.is_empty() {
                spans.push(Span::styled(
                    format!("{}   ", app.status),
                    Style::new().fg(accent()),
                ));
            }
            spans.push(Span::styled(context_hints(app), Style::new().fg(subtle())));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);

    // Right-aligned breadcrumb, only when there's comfortable room for it.
    let breadcrumb = build_breadcrumb(app);
    let bc_width = breadcrumb.chars().count() as u16;
    if !breadcrumb.is_empty() && area.width > bc_width + 40 {
        let r = Rect {
            x: area.x + area.width - bc_width - 1,
            y: area.y,
            width: bc_width,
            height: 1,
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                breadcrumb,
                Style::new().fg(subtle()),
            ))),
            r,
        );
    }
}

fn build_breadcrumb(app: &App) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Project name
    if let Some(p) = app.current_project() {
        parts.push(p.name.clone());
    } else {
        return String::new();
    }

    match app.focus {
        Focus::Projects => {
            parts.push("projects".into());
        }
        Focus::Content => match app.tab {
            Tab::Overview => parts.push("overview".into()),
            Tab::Todos => parts.push("todos".into()),
            Tab::Notes => parts.push("notes".into()),
            Tab::Schedule => parts.push("schedule".into()),
        },
        Focus::Detail => match app.tab {
            Tab::Todos => {
                parts.push("todos".into());
                if let Some(t) = app.current_todo() {
                    parts.push(truncate(&t.title, 20));
                }
            }
            Tab::Notes => {
                parts.push("notes".into());
                if let Some(n) = app.current_note() {
                    parts.push(truncate(&n.text, 20));
                }
            }
            _ => {}
        },
    }

    parts.join(" > ")
}

// ---- overlays -------------------------------------------------------

fn render_input(f: &mut Frame, area: Rect, input: &InputState) {
    let width = area.width.saturating_sub(8).clamp(24, 74);
    let rect = popup(area, width, 5);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            format!(" {} ", input.title),
            Style::new().fg(accent()).bold(),
        ))
        .padding(Padding::new(2, 2, 1, 1));

    let mut ta = input.editor.clone();
    ta.set_block(block);
    ta.set_cursor_line_style(Style::new());
    ta.set_selection_style(Style::new().bg(sel_bg()));
    f.render_widget(&ta, rect);
}

fn render_notice(f: &mut Frame, area: Rect, title: &str, body: &str) {
    let width = area.width.saturating_sub(8).clamp(30, 76);
    let lines: Vec<Line> = body
        .split('\n')
        .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(text()))))
        .collect();
    let height = (lines.len() as u16 + 4).clamp(6, area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(red()))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(red()).bold(),
        ))
        .title(
            Line::from(Span::styled(" any key closes ", Style::new().fg(subtle()))).right_aligned(),
        )
        .padding(Padding::new(2, 2, 1, 1));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        rect,
    );
}

fn render_confirm(f: &mut Frame, area: Rect, c: &ConfirmState) {
    let width = area.width.saturating_sub(8).clamp(24, 64);
    let rect = popup(area, width, 7);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(red()))
        .title(Span::styled(" Confirm ", Style::new().fg(red()).bold()))
        .padding(Padding::new(2, 2, 1, 1));
    let text = vec![
        Line::from(Span::styled(c.prompt.clone(), Style::new().fg(text()))),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::new().fg(green()).bold()),
            Span::styled("  yes        ", Style::new().fg(subtle())),
            Span::styled("n", Style::new().fg(red()).bold()),
            Span::styled("  cancel", Style::new().fg(subtle())),
        ]),
    ];
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(block),
        rect,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    // ("", "HEADING") = section header · ("", "") = blank · (key, desc) = binding
    type Row = (&'static str, &'static str);
    const COL1: &[Row] = &[
        ("", "MOVE"),
        ("h j k l", "move / switch pane"),
        ("w  s", "prev / next project"),
        ("tab  S-tab", "prev / next view"),
        ("1 … 4", "jump to a view"),
        ("gg  G", "top / bottom"),
        ("esc", "back out one pane"),
        ("", ""),
        ("", "GENERAL"),
        ("?  q", "help · quit"),
        ("^t", "change theme"),
        ("^e", "edit settings file"),
        ("^y", "sync to GitHub"),
        ("^g", "link project's repo"),
        ("click", "focus pane / row"),
        ("", ""),
        ("", "QUICK-ADD (todo)"),
        ("!1 !2 !3", "priority"),
        ("@date", "due  (YYYY-MM-DD)"),
    ];
    const COL2: &[Row] = &[
        ("", "PROJECTS"),
        ("a  r  d", "add / rename / del"),
        ("l", "open project"),
        ("o", "show repo activity"),
        ("", ""),
        ("", "OVERVIEW"),
        ("e", "edit description"),
        ("r", "rename project"),
        ("", ""),
        ("", "SCHEDULE"),
        ("a", "add milestone"),
        ("e  d", "edit / delete"),
        ("x", "toggle done"),
        ("r", "reschedule date"),
        ("f", "cycle date filter"),
        ("l", "jump to the todo"),
    ];
    const COL3: &[Row] = &[
        ("", "TODOS / SUBTASKS"),
        ("a  e  d", "add / edit / delete"),
        ("x / space", "toggle done"),
        ("p", "cycle priority"),
        ("J  K", "move up / down"),
        ("l", "open subtasks"),
        ("", ""),
        ("", "NOTES"),
        ("a  e  d", "add / edit / delete"),
        ("x / space", "pin / unpin"),
        ("J  K", "move up / down"),
        ("l", "open note body"),
        ("", ""),
        ("", "NOTE BODY"),
        ("j k ^d ^u", "scroll · page"),
        ("space", "expand / collapse"),
        ("e  i", "edit  (^s = save)"),
    ];

    let render_col = |rows: &[Row]| -> Vec<Line<'static>> {
        rows.iter()
            .map(|(key, desc)| {
                if key.is_empty() && desc.is_empty() {
                    Line::from("")
                } else if key.is_empty() {
                    Line::from(Span::styled(
                        (*desc).to_string(),
                        Style::new().fg(accent()).bold(),
                    ))
                } else {
                    Line::from(vec![
                        Span::styled(format!("{key:<10} "), Style::new().fg(yellow())),
                        Span::styled((*desc).to_string(), Style::new().fg(subtle())),
                    ])
                }
            })
            .collect()
    };

    let rows = COL1.len().max(COL2.len()).max(COL3.len());
    let width = 110u16.min(area.width);
    let height = (rows as u16 + 2).min(area.height);
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            " Keybindings ",
            Style::new().fg(accent()).bold(),
        ))
        .title(
            Line::from(Span::styled(" any key closes ", Style::new().fg(subtle()))).right_aligned(),
        )
        .padding(Padding::horizontal(2));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let cols = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .spacing(2)
    .split(inner);
    f.render_widget(Paragraph::new(render_col(COL1)), cols[0]);
    f.render_widget(Paragraph::new(render_col(COL2)), cols[1]);
    f.render_widget(Paragraph::new(render_col(COL3)), cols[2]);
}

fn render_github(f: &mut Frame, area: Rect, app: &App) {
    let Some(info) = &app.gh_cache else {
        let rect = popup(area, 50, 5);
        overlay(f, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(accent()))
            .title(Span::styled(" GitHub ", Style::new().fg(accent()).bold()))
            .padding(Padding::horizontal(2));
        f.render_widget(Paragraph::new("No data loaded.").block(block), rect);
        return;
    };

    let width = area.width.saturating_sub(8).clamp(40, 80);
    let height = area.height.saturating_sub(4).min(30);
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(" GitHub ", Style::new().fg(accent()).bold()))
        .padding(Padding::horizontal(1));

    let mut lines: Vec<Line> = Vec::new();

    // Commits
    lines.push(Line::from(Span::styled(
        "  Commits",
        Style::new().fg(accent()).bold(),
    )));
    if info.commits.is_empty() {
        lines.push(Line::from(Span::styled(
            "    no recent commits",
            Style::new().fg(subtle()),
        )));
    } else {
        for c in info.commits.iter().take(5) {
            let sha = &c.sha[..7.min(c.sha.len())];
            let msg = c.commit.message.lines().next().unwrap_or("");
            let msg = if msg.chars().count() > 40 {
                let kept: String = msg.chars().take(39).collect();
                format!("{kept}…")
            } else {
                msg.to_string()
            };
            lines.push(Line::from(vec![
                Span::styled(format!("    {sha} "), Style::new().fg(blue())),
                Span::styled(msg, Style::new().fg(text())),
            ]));
        }
    }
    lines.push(Line::from(""));

    // Pull Requests
    lines.push(Line::from(Span::styled(
        "  Open PRs",
        Style::new().fg(accent()).bold(),
    )));
    let prs: Vec<_> = info.prs.iter().filter(|p| p.state == "open").collect();
    if prs.is_empty() {
        lines.push(Line::from(Span::styled(
            "    no open PRs",
            Style::new().fg(subtle()),
        )));
    } else {
        for pr in prs.iter().take(5) {
            lines.push(Line::from(vec![
                Span::styled(format!("    #{} ", pr.number), Style::new().fg(blue())),
                Span::styled(pr.title.clone(), Style::new().fg(text())),
            ]));
        }
    }
    lines.push(Line::from(""));

    // Issues
    lines.push(Line::from(Span::styled(
        "  Open Issues",
        Style::new().fg(accent()).bold(),
    )));
    let issues: Vec<_> = info.issues.iter().filter(|i| i.state == "open").collect();
    if issues.is_empty() {
        lines.push(Line::from(Span::styled(
            "    no open issues",
            Style::new().fg(subtle()),
        )));
    } else {
        for issue in issues.iter().take(5) {
            lines.push(Line::from(vec![
                Span::styled(format!("    #{} ", issue.number), Style::new().fg(blue())),
                Span::styled(issue.title.clone(), Style::new().fg(text())),
            ]));
        }
    }

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, rect);
}

fn render_theme(f: &mut Frame, area: Rect, state: &ThemeState) {
    let themes = crate::theme::registry();
    let current = &crate::theme::current().slug;
    let width = 48u16.min(area.width);
    let height = (themes.len() as u16 + 4).min(area.height);
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(" Theme ", Style::new().fg(accent()).bold()))
        .title(
            Line::from(Span::styled(
                " j/k preview · enter apply ",
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let items: Vec<ListItem> = themes
        .iter()
        .map(|t| {
            let mark = if &t.slug == current {
                Span::styled("● ", Style::new().fg(accent()))
            } else {
                Span::raw("  ")
            };
            let swatch = |c| Span::styled("█", Style::new().fg(c));
            let p = t.palette;
            ListItem::new(Line::from(vec![
                mark,
                Span::styled(
                    format!("{:<22}", truncate(&t.name, 22)),
                    Style::new().fg(text()),
                ),
                Span::raw("  "),
                swatch(p.accent),
                swatch(p.green),
                swatch(p.yellow),
                swatch(p.red),
                swatch(p.blue),
            ]))
        })
        .collect();

    let list = List::new(items).highlight_style(Style::new().bg(sel_bg()).bold());
    let mut ls = ListState::default();
    ls.select(Some(state.idx));
    f.render_stateful_widget(list, inner, &mut ls);
}

// ---- helpers -------------------------------------------------------

fn panel(title: String, focused: bool) -> Block<'static> {
    let color = if focused { accent() } else { subtle() };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { accent() } else { border() }))
        .title(Span::styled(title, Style::new().fg(color).bold()))
}

fn hint(text: &str) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(format!("  {text}"), Style::new().fg(subtle()))),
    ])
    .wrap(Wrap { trim: false })
}

fn prio_style(p: Priority) -> Style {
    match p {
        Priority::High => Style::new().fg(red()),
        Priority::Medium => Style::new().fg(yellow()),
        Priority::Low => Style::new().fg(blue()),
    }
}

fn rel(date: NaiveDate, today: NaiveDate) -> String {
    match (date - today).num_days() {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        -1 => "yesterday".to_string(),
        d if d < 0 => format!("{}d overdue", -d),
        d => format!("in {d}d"),
    }
}

/// Coarse "time since" label for the last sync. Refreshes only when the UI
/// redraws (a keypress or background event), which is close enough here.
fn rel_time(t: chrono::DateTime<Local>) -> String {
    let secs = (Local::now() - t).num_seconds().max(0);
    match secs {
        0..=44 => "just now".to_string(),
        45..=5399 => format!("{}m ago", (secs + 30) / 60),
        5400..=86_399 => format!("{}h ago", (secs + 1800) / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

fn progress_bar(done: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("{}  —", "░".repeat(width));
    }
    let filled = (done * width) / total;
    let pct = (done * 100) / total;
    format!(
        "{}{}  {pct}%",
        "█".repeat(filled),
        "░".repeat(width - filled)
    )
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 3,
        width,
        height,
    }
}

/// Blank out a popup area and repaint it with the themed background, so overlay
/// text reads correctly on every theme (including light ones).
fn overlay(f: &mut Frame, rect: Rect) {
    f.render_widget(Clear, rect);
    f.render_widget(Block::default().style(Style::new().bg(bg())), rect);
}
