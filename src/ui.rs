//! Rendering. A three-row shell (header / body / status) with a two-pane body.

use chrono::{Local, NaiveDate};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Row,
        Table, Wrap,
    },
};

use crate::app::{
    App, AttachState, ConfirmAction, ConfirmState, DetailNote, EditTarget, Focus, InputState,
    LinksState, LogEntry, MenuAction, MenuState, Mode, PaneRects, SearchState, SearchTarget,
    SortState, Tab, TagState, TagTarget, ThemeState, TlKind, Toast, ToastKind,
};
use crate::model::{AttachmentKind, Meeting, Priority};
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

    // The `^l` activity panel eats a strip off the bottom of the body.
    let body_rect = if app.activity_open {
        let h = (rows[1].height / 3).clamp(6, 16);
        let split = Layout::vertical([Constraint::Min(3), Constraint::Length(h)]).split(rows[1]);
        render_activity(f, split[1], app);
        split[0]
    } else {
        rows[1]
    };

    if app.note_full
        && !matches!(app.mode, Mode::EditBody(_))
        && let Some((title, body, scroll)) = app.full_note_view()
    {
        // `^f` — the note fills the window; no rails, no lists.
        rects.detail = body_rect;
        render_full_note(f, body_rect, app, title, body, scroll);
    } else if app.tab == Tab::Todos || app.tab == Tab::Notes || app.tab == Tab::Meetings {
        if app.tab == Tab::Notes && app.note_expanded && app.focus == Focus::Detail {
            // Full-width note mode — projects + note body (or its editor) only.
            let body = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(body_rect);
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
            .split(body_rect);
            rects.projects = body[0];
            rects.content = body[1];
            rects.detail = body[2];
            render_projects(f, body[0], app);
            render_content(f, body[1], app);
            match (&app.mode, app.tab) {
                (Mode::EditBody(state), _) => render_edit_body_pane(f, body[2], state, app),
                (_, Tab::Todos) if app.showing_todo_note() => {
                    // `n` at todo level: the todo's note takes the whole 3rd pane.
                    render_detail_note(f, body[2], app, DetailNote::Todo);
                }
                (_, Tab::Todos) if app.showing_sub_note() && body[2].height >= 8 => {
                    // `n` in the Subtasks pane: the subtask's note sits in a
                    // section below the still-visible subtask list (4th pane).
                    let split = Layout::vertical([Constraint::Min(4), Constraint::Percentage(42)])
                        .split(body[2]);
                    rects.detail = split[0];
                    render_subtasks(f, split[0], app);
                    render_detail_note(f, split[1], app, DetailNote::Subtask(app.subtask_idx));
                }
                (_, Tab::Todos) => render_subtasks(f, body[2], app),
                (_, Tab::Meetings) => render_meeting_note(f, body[2], app),
                _ => render_note_body(f, body[2], app),
            }
        }
    } else {
        let body = Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(body_rect);
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
        Mode::Weather => render_weather(f, area, app),
        Mode::Theme(state) => render_theme(f, area, state),
        Mode::Notice(title, body) => render_notice(f, area, title, body),
        Mode::Attach(state) => render_attach(f, area, state, app),
        Mode::Tags(state) => render_tags(f, area, state, app),
        Mode::Links(state) => render_links(f, area, state),
        Mode::Menu(state) => render_menu(f, area, state),
        Mode::Sort(state) => render_sort(f, area, state),
        Mode::Search(state) => render_search(f, area, state, app),
        Mode::EditBody(_) => {}
        Mode::Normal => {}
    }

    if let Some(t) = &app.toast {
        render_toast(f, area, t);
    }
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let n = app.store.projects.len();

    let name = Span::styled("  voido", Style::new().fg(accent()).bold());
    let name_w = name.content.chars().count() as u16;
    f.render_widget(Paragraph::new(Line::from(name)), area);

    if app.minimal {
        return;
    }

    // Everything else hugs the right edge, mirroring the footer's breadcrumb.
    // Least useful segment first: a narrow terminal drops them from the front,
    // so the overdue count is the last thing to go.
    let done = app
        .store
        .projects
        .iter()
        .filter(|p| p.is_complete())
        .count();
    let (_, _, overdue, _, _) = app.deadline_stats();
    let mut segs: Vec<(String, Style)> = vec![(
        format!("{n} project{}", if n == 1 { "" } else { "s" }),
        Style::new().fg(subtle()),
    )];
    if done > 0 {
        segs.push((format!("{done} done"), Style::new().fg(subtle())));
    }
    if overdue > 0 {
        segs.push((format!("{overdue} overdue"), Style::new().fg(red()).bold()));
    }

    // Width of the segments still in, separators and the right-hand gap.
    let width_of = |segs: &[(String, Style)]| -> u16 {
        let text: usize = segs.iter().map(|(t, _)| t.chars().count()).sum();
        let seps = segs.len().saturating_sub(1) * 5;
        (text + seps + 2) as u16
    };
    let room = area.width.saturating_sub(name_w + 2);
    while !segs.is_empty() && width_of(&segs) > room {
        segs.remove(0);
    }
    if segs.is_empty() {
        return;
    }

    let mut spans = Vec::new();
    for (i, (text, style)) in segs.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::new().fg(subtle())));
        }
        spans.push(Span::styled(text, style));
    }
    spans.push(Span::raw("  "));
    f.render_widget(Paragraph::new(Line::from(spans).right_aligned()), area);
}

/// Weather glyph + temperature and the clock, as one line — shown centred in the
/// footer.
fn weather_clock_line(app: &App) -> Line<'static> {
    let now = Local::now();
    let date_text = if app.minimal {
        now.format("%H:%M").to_string()
    } else {
        now.format("%a %b %d · %H:%M").to_string()
    };
    let mut spans: Vec<Span> = Vec::new();
    if let Some(w) = &app.weather {
        let deg = if app.minimal {
            String::new()
        } else {
            w.deg().to_string()
        };
        spans.push(Span::styled(
            format!("{} {}°{deg}", w.glyph(), w.temp_i()),
            Style::new().fg(subtle()),
        ));
        spans.push(Span::styled("  ·  ", Style::new().fg(border())));
    }
    spans.push(Span::styled(date_text, Style::new().fg(subtle())));
    Line::from(spans).centered()
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

    let inner_w = list_inner_w(area);

    // One distinct icon per project; colour carries the status.
    let icons = crate::util::project_icons(app.store.projects.iter().map(|p| p.name.as_str()));

    let specs: Vec<RowSpec> = app
        .store
        .projects
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let open = p.open_todos();
            let total = p.todos.len();
            let complete = p.is_complete();
            let is_empty = total == 0 && p.milestones.is_empty() && p.notes.is_empty();

            let (icon_color, name_style) = if complete {
                (
                    green(),
                    Style::new()
                        .fg(subtle())
                        .add_modifier(Modifier::CROSSED_OUT),
                )
            } else if is_empty {
                (subtle(), Style::new().fg(subtle()))
            } else if open == 0 {
                (green(), Style::new().fg(text()))
            } else {
                (accent(), Style::new().fg(text()))
            };

            let tally = if complete {
                ("✓".to_string(), Style::new().fg(green()))
            } else if total == 0 {
                ("·".to_string(), Style::new().fg(border()))
            } else if open == 0 {
                (format!("{total}/{total}"), Style::new().fg(green()))
            } else {
                (
                    format!("{}/{total}", total - open),
                    Style::new().fg(subtle()),
                )
            };

            RowSpec {
                prefix: vec![Span::styled(
                    format!("{} ", icons[i]),
                    Style::new().fg(icon_color),
                )],
                title: p.name.clone(),
                title_style: name_style,
                cells: if inner_w >= 12 { vec![tally] } else { vec![] },
            }
        })
        .collect();

    let mut items: Vec<ListItem> = Vec::with_capacity(app.store.projects.len());
    for (i, line) in meta_rows(inner_w, specs).into_iter().enumerate() {
        let p = &app.store.projects[i];
        let (sd, st) = p.subtask_progress();
        let nc = p.note_count();
        let total = p.todos.len();
        let mut lines = vec![line];

        if app.project_info && i == app.project_idx {
            let label = |s: &str| Span::styled(format!("   {s:<11}"), Style::new().fg(subtle()));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                label("created"),
                Span::styled(
                    p.created.format("%Y-%m-%d").to_string(),
                    Style::new().fg(text()),
                ),
            ]));
            if !p.tags.is_empty() {
                lines.push(Line::from(vec![
                    label("tags"),
                    Span::styled(
                        p.tags
                            .iter()
                            .map(|t| format!("#{t}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                        Style::new().fg(blue()),
                    ),
                ]));
            }
            if total > 0 {
                lines.push(Line::from(vec![
                    label("todos"),
                    Span::styled(
                        format!("{}/{total}", p.done_todos()),
                        Style::new().fg(text()),
                    ),
                ]));
            }
            if st > 0 {
                lines.push(Line::from(vec![
                    label("subtasks"),
                    Span::styled(format!("{sd}/{st}"), Style::new().fg(text())),
                ]));
            }
            if nc > 0 {
                lines.push(Line::from(vec![
                    label("notes"),
                    Span::styled(format!("{nc}"), Style::new().fg(text())),
                ]));
            }
            if !p.description.is_empty() {
                lines.push(Line::from(vec![
                    label("desc"),
                    Span::styled(p.description.clone(), Style::new().fg(text())),
                ]));
            }
            if let Some(repo) = &p.repo {
                lines.push(Line::from(vec![
                    label("repo"),
                    Span::styled(repo.clone(), Style::new().fg(blue())),
                ]));
            }
            if !p.milestones.is_empty() {
                let done = p.milestones.iter().filter(|m| m.done).count();
                let mtot = p.milestones.len();
                let mstyle = if done == mtot {
                    Style::new().fg(green())
                } else {
                    Style::new().fg(subtle())
                };
                lines.push(Line::from(vec![
                    label("milestones"),
                    Span::styled(format!("{done}/{mtot}"), mstyle),
                ]));
            }
            lines.push(Line::from(""));
        }
        items.push(ListItem::new(lines));
    }

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
        Tab::Meetings => render_meetings(f, area, app),
    }
}

/// The bordered block for the middle pane. Its top border carries the tab strip
/// (left) and the project name (right), so no separate tab row is needed.
fn content_block(app: &App, width: u16) -> Block<'static> {
    let focused = app.content_lit();

    let mut spans = Vec::new();
    let mut tabs_w = 0usize;
    for t in Tab::ALL {
        let active = t == app.tab;
        let style = if active {
            Style::new().fg(accent()).bold().bg(sel_bg())
        } else if focused {
            Style::new().fg(subtle())
        } else {
            Style::new().fg(border())
        };
        let label = format!(" {} ", t.strip_label(width));
        tabs_w += label.chars().count();
        spans.push(Span::styled(label, style));
    }
    let tabs = Line::from(spans);

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { accent() } else { border() }))
        .title(tabs);

    // The project name on the right only when it won't crowd the tab strip.
    if let Some(p) = app.current_project() {
        let room = (width as usize).saturating_sub(tabs_w + 4);
        if room >= 8 {
            block = block.title(
                Line::from(Span::styled(
                    format!(" {} ", truncate(&p.name, room)),
                    Style::new().fg(subtle()),
                ))
                .right_aligned(),
            );
        }
    }
    block
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    let block = content_block(app, area.width).padding(Padding::horizontal(1));

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

    let icon = crate::util::project_icons(app.store.projects.iter().map(|p| p.name.as_str()))
        .get(app.project_idx)
        .copied()
        .unwrap_or("\u{f07b}");
    let name_line = if p.is_complete() {
        Line::from(vec![
            Span::styled(format!("{icon}  "), Style::new().fg(green())),
            Span::styled(p.name.clone(), Style::new().fg(green()).bold()),
            Span::styled("   ✔ done", Style::new().fg(green())),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("{icon}  "), Style::new().fg(accent())),
            Span::styled(p.name.clone(), Style::new().fg(text()).bold()),
        ])
    };

    let bar_w = (area.width as usize).saturating_sub(20).clamp(10, 30);
    let label = |s: &str| Span::styled(format!("{s:<11}"), Style::new().fg(subtle()));
    let stat = |d: usize, t: usize| {
        let style = if t > 0 && d == t {
            Style::new().fg(green())
        } else {
            Style::new().fg(text())
        };
        Span::styled(format!("{d}/{t}"), style)
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
        Line::from(vec![label("Todos"), stat(done, total)]),
        Line::from(vec![
            Span::raw("           "),
            Span::styled(progress_bar(done, total, bar_w), Style::new().fg(green())),
        ]),
    ];

    if subs > 0 {
        lines.push(Line::from(vec![label("Subtasks"), stat(subs_done, subs)]));
    }
    lines.push(Line::from(vec![
        label("Notes"),
        Span::styled(p.notes.len().to_string(), Style::new().fg(text())),
    ]));
    if !p.milestones.is_empty() {
        let md = p.milestones.iter().filter(|m| m.done).count();
        lines.push(Line::from(vec![
            label("Milestones"),
            stat(md, p.milestones.len()),
        ]));
    }
    if !p.meetings.is_empty() {
        let held = p.meetings.iter().filter(|m| m.held).count();
        lines.push(Line::from(vec![
            label("Meetings"),
            stat(held, p.meetings.len()),
        ]));
    }
    lines.push(Line::from(""));

    match p.next_milestone() {
        Some(m) => lines.push(Line::from(vec![
            label("Next"),
            Span::styled("◆ ", Style::new().fg(accent())),
            Span::styled(m.title.clone(), Style::new().fg(text())),
            Span::styled(
                format!("   {} · {}", m.date.format("%b %d"), rel(m.date, today)),
                Style::new().fg(subtle()),
            ),
        ])),
        None => lines.push(Line::from(vec![
            label("Next"),
            Span::styled("no upcoming milestones", Style::new().fg(subtle())),
        ])),
    }

    if let Some(m) = p.next_meeting() {
        lines.push(Line::from(vec![
            label("Meeting"),
            Span::styled("\u{f0c0} ", Style::new().fg(accent())),
            Span::styled(m.title.clone(), Style::new().fg(text())),
            Span::styled(
                format!("   {} · {}", m.date.format("%b %d"), meeting_rel(m, today)),
                Style::new().fg(subtle()),
            ),
        ]));
    }

    if let Some(w) = &app.weather {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            label("Weather"),
            Span::styled(format!("{}  ", w.glyph()), Style::new().fg(accent())),
            Span::styled(
                format!("{}°{}  {}", w.temp_i(), w.deg(), w.label()),
                Style::new().fg(text()),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::raw("           "),
            Span::styled(
                format!(
                    "feels {}°{} · wind {} {} · {} · {}  (^w)",
                    w.current.feels_like.round() as i64,
                    w.deg(),
                    w.current.wind.round() as i64,
                    w.unit.wind_label(),
                    w.place,
                    rel_time(w.at),
                ),
                Style::new().fg(subtle()),
            ),
        ]));
    }

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(block),
        area,
    );
}

fn render_notes(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Notes;
    let block = content_block(app, area.width);

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

    let inner_w = list_inner_w(area);
    let specs: Vec<RowSpec> = project
        .notes
        .iter()
        .map(|n| {
            let (mark, mark_style) = if n.pinned {
                ("★ ", Style::new().fg(yellow()))
            } else {
                ("• ", Style::new().fg(subtle()))
            };
            let cell = if n.body.trim().is_empty() || inner_w < 16 {
                (String::new(), Style::default())
            } else {
                let lines = n.body.lines().filter(|l| !l.trim().is_empty()).count();
                (format!("¶ {lines}"), Style::new().fg(subtle()))
            };
            RowSpec {
                prefix: vec![Span::styled(mark, mark_style)],
                title: n.text.clone(),
                title_style: Style::new().fg(text()),
                cells: vec![cell],
            }
        })
        .collect();
    let items: Vec<ListItem> = meta_rows(inner_w, specs)
        .into_iter()
        .map(ListItem::new)
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
    let block = content_block(app, area.width);

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
    let block = match project
        .todos
        .get(app.todo_idx)
        .and_then(|t| t.due.map(|d| (t, d)))
    {
        Some((t, d)) => {
            let style = if !t.done && d < today {
                Style::new().fg(red()).bold()
            } else if d == today {
                Style::new().fg(green())
            } else {
                Style::new().fg(subtle())
            };
            let label = if d == today {
                "due today".to_string()
            } else {
                format!("due {}", d.format("%b %d"))
            };
            block.title_bottom(Line::from(Span::styled(format!(" {label} "), style)))
        }
        None => block,
    };
    let inner_w = list_inner_w(area);
    // Metadata columns, left to right: priority · subtasks · note · file · due.
    // Which columns appear is a whole-pane decision (so rows stay aligned); the
    // narrower the pane, the fewer are kept.
    let show_prio = inner_w >= 16;
    let show_marks = inner_w >= 34;
    let show_subs = inner_w >= 42;
    let specs: Vec<RowSpec> = project
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

            let (sdone, stotal) = t.subtask_progress();
            let subs = if stotal > 0 {
                let style = if sdone == stotal {
                    Style::new().fg(green())
                } else {
                    Style::new().fg(subtle())
                };
                (format!("⊞{sdone}/{stotal}"), style)
            } else {
                (String::new(), Style::default())
            };
            let note = if t.note.trim().is_empty() {
                (String::new(), Style::default())
            } else {
                ("¶".to_string(), Style::new().fg(subtle()))
            };
            let files = if t.attachments.is_empty() {
                (String::new(), Style::default())
            } else {
                (
                    format!("\u{f0c6}{}", t.attachments.len()),
                    Style::new().fg(subtle()),
                )
            };
            let due = match t.due {
                Some(d) => {
                    let style = if !t.done && d < today {
                        Style::new().fg(red()).bold()
                    } else if d == today {
                        Style::new().fg(green())
                    } else {
                        Style::new().fg(subtle())
                    };
                    (d.format("%b %d").to_string(), style)
                }
                None => (String::new(), Style::default()),
            };

            let mut cells = Vec::new();
            if show_prio {
                cells.push(prio_cell(t.priority));
            }
            if show_subs {
                cells.push(subs);
            }
            if show_marks {
                cells.push(note);
                cells.push(files);
            }
            cells.push(due);

            RowSpec {
                prefix: vec![Span::styled(mark, mark_style)],
                title: t.title.clone(),
                title_style,
                cells,
            }
        })
        .collect();
    let mut items: Vec<ListItem> = Vec::with_capacity(project.todos.len());
    for (i, line) in meta_rows(inner_w, specs).into_iter().enumerate() {
        let mut lines = vec![line];
        if app.todo_info && i == app.todo_idx {
            lines.extend(todo_detail_lines(&project.todos[i], today));
        }
        items.push(ListItem::new(lines));
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
    state.select(Some(app.todo_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_subtasks(f: &mut Frame, area: Rect, app: &App) {
    // Focus can be on the list, or handed off to the note pane below it.
    let focused = app.focus == Focus::Detail && !app.sub_note_focus;

    let Some(todo) = app.current_todo() else {
        let block = panel(" Subtasks ".to_string(), focused);
        f.render_widget(
            hint("Pick a todo, press l for subtasks.").block(block),
            area,
        );
        return;
    };

    let (done, total) = todo.subtask_progress();
    let mut markers = String::new();
    if !todo.note.trim().is_empty() {
        markers.push_str("  ¶");
    }
    if !todo.attachments.is_empty() {
        markers.push_str(&format!("  \u{f0c6}{}", todo.attachments.len()));
    }
    let counts = if total > 0 {
        format!("  {done}/{total}")
    } else {
        String::new()
    };
    // Give the parent name whatever the pane width leaves after the fixed parts.
    let fixed = " Subtasks ·  ".chars().count() + counts.chars().count() + markers.chars().count();
    let name = truncate(
        &todo.title,
        (area.width as usize).saturating_sub(fixed + 3).max(8),
    );
    let block = panel(format!(" Subtasks · {name}{counts}{markers} "), focused);

    let today = Local::now().date_naive();
    let block = if let Some(d) = todo.due {
        let style = if !todo.done && d < today {
            Style::new().fg(red()).bold()
        } else if d == today {
            Style::new().fg(green())
        } else {
            Style::new().fg(subtle())
        };
        let label = if d == today {
            "due today".to_string()
        } else {
            format!("due {}", d.format("%b %d"))
        };
        block.title_bottom(Line::from(Span::styled(format!(" {label} "), style)))
    } else {
        block
    };

    if todo.subtasks.is_empty() {
        f.render_widget(
            hint("No subtasks yet — press a to add one.").block(block),
            area,
        );
        return;
    }

    let inner_w = list_inner_w(area);
    let show_prio = inner_w >= 14;
    let show_marks = inner_w >= 26;
    let specs: Vec<RowSpec> = todo
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

            let note = if s.note.trim().is_empty() {
                (String::new(), Style::default())
            } else {
                ("¶".to_string(), Style::new().fg(subtle()))
            };
            let files = if s.attachments.is_empty() {
                (String::new(), Style::default())
            } else {
                (
                    format!("\u{f0c6}{}", s.attachments.len()),
                    Style::new().fg(subtle()),
                )
            };

            let mut cells = Vec::new();
            if show_prio {
                cells.push(prio_cell(s.priority));
            }
            if show_marks {
                cells.push(note);
                cells.push(files);
            }

            RowSpec {
                prefix: vec![Span::styled(mark, mark_style)],
                title: s.title.clone(),
                title_style,
                cells,
            }
        })
        .collect();
    let mut items: Vec<ListItem> = Vec::with_capacity(todo.subtasks.len());
    for (i, line) in meta_rows(inner_w, specs).into_iter().enumerate() {
        let mut lines = vec![line];
        if app.subtask_info && i == app.subtask_idx {
            lines.extend(subtask_detail_lines(&todo.subtasks[i]));
        }
        items.push(ListItem::new(lines));
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
    state.select(Some(app.subtask_idx));
    f.render_stateful_widget(list, area, &mut state);
}

/// A todo's note (fills the 3rd pane) or a subtask's note (section below the
/// subtask list), toggled with `n`. `^d` / `^u` scroll it.
fn render_detail_note(f: &mut Frame, area: Rect, app: &App, which: DetailNote) {
    let Some(todo) = app.current_todo() else {
        return;
    };
    let (title, body, scroll_val) = match which {
        DetailNote::Todo => (
            fit_title(format!(" Note · {} ", todo.title), area.width),
            todo.note.as_str(),
            app.todo_note_scroll,
        ),
        DetailNote::Subtask(i) => match todo.subtasks.get(i) {
            Some(s) => (
                fit_title(format!(" ↳ Note · {} ", s.title), area.width),
                s.note.as_str(),
                app.sub_note_scroll,
            ),
            None => return,
        },
    };
    // The todo note owns the whole pane, and wears the accent in place of the
    // list it belongs to; the subtask note glows only while focus has been
    // stepped into it. Either way exactly one pane is lit.
    let lit = match which {
        DetailNote::Todo => app.focus == Focus::Content,
        DetailNote::Subtask(_) => app.sub_note_focus,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if lit { accent() } else { border() }))
        .title(Span::styled(title, Style::new().fg(subtle()).bold()))
        .padding(Padding::horizontal(1));

    let rendered = app.note_body_lines(body, area.width.saturating_sub(4));
    let total = rendered.len() as u16;
    let view = area.height.saturating_sub(2).max(1);
    let max_scroll = total.saturating_sub(view);
    let scroll = scroll_val.min(max_scroll);

    f.render_widget(
        Paragraph::new(rendered)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );

    if total > view {
        let pct = if max_scroll == 0 {
            100
        } else {
            (scroll as u32 * 100 / max_scroll as u32) as u16
        };
        let keys = if matches!(which, DetailNote::Subtask(_)) && app.sub_note_focus {
            "j/k"
        } else {
            "^d/^u"
        };
        let tag = format!(" {pct}%  {keys} ");
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

/// `^f` — the note on screen, alone, filling the whole body area.
fn render_full_note(f: &mut Frame, area: Rect, app: &App, title: String, body: &str, scroll: u16) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            fit_title(title, area.width),
            Style::new().fg(accent()).bold(),
        ))
        .title(
            Line::from(Span::styled(
                " j/k ^d/^u scroll · esc close ",
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::new(2, 2, 1, 1));

    let inner = block.inner(area);
    let rendered = app.note_body_lines(body, inner.width);
    let total = rendered.len() as u16;
    let max_scroll = total.saturating_sub(inner.height);
    let scroll = scroll.min(max_scroll);

    f.render_widget(
        Paragraph::new(rendered)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );

    if total > inner.height {
        let pct = if max_scroll == 0 {
            100
        } else {
            (scroll as u32 * 100 / max_scroll as u32) as u16
        };
        let tag = format!(" {pct}% ");
        let w = tag.chars().count() as u16;
        if area.width > w + 2 && area.height > 1 {
            let r = Rect {
                x: area.x + area.width - w - 1,
                y: area.y + area.height - 1,
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

/// The Meetings tab: one row per meeting, soonest at whatever position `J`/`K`
/// or the `o` menu put it in.
fn render_meetings(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Meetings;
    let block = content_block(app, area.width);

    let Some(project) = app.current_project() else {
        f.render_widget(
            hint("Create a project to start booking meetings.").block(block),
            area,
        );
        return;
    };
    if project.meetings.is_empty() {
        f.render_widget(
            hint("No meetings yet.  Press a to add one.").block(block),
            area,
        );
        return;
    }

    let today = Local::now().date_naive();
    // Bottom border: how far off the selected meeting is.
    let block = match project.meetings.get(app.meeting_idx) {
        Some(m) => {
            let style = if m.held {
                Style::new().fg(subtle())
            } else if m.date == today {
                Style::new().fg(green()).bold()
            } else if m.date < today {
                Style::new().fg(red())
            } else {
                Style::new().fg(subtle())
            };
            block.title_bottom(Line::from(Span::styled(
                format!(" {} ", meeting_rel(m, today)),
                style,
            )))
        }
        None => block,
    };

    let inner_w = list_inner_w(area);
    let show_when = inner_w >= 24;
    let show_people = inner_w >= 34;
    let specs: Vec<RowSpec> = project
        .meetings
        .iter()
        .map(|m| {
            let (mark, mark_style) = if m.held {
                ("✔ ", Style::new().fg(green()))
            } else if m.date == today {
                ("● ", Style::new().fg(green()))
            } else if m.date < today {
                ("● ", Style::new().fg(red()))
            } else {
                ("○ ", Style::new().fg(subtle()))
            };
            let title_style = if m.held {
                Style::new()
                    .fg(subtle())
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::new().fg(text())
            };

            let mut cells = Vec::new();
            if show_people {
                let people = if m.attendees.is_empty() {
                    (String::new(), Style::default())
                } else {
                    (
                        format!("\u{f0c0}{}", m.attendees.len()),
                        Style::new().fg(subtle()),
                    )
                };
                cells.push(people);
                let note = if m.note.trim().is_empty() {
                    (String::new(), Style::default())
                } else {
                    ("¶".to_string(), Style::new().fg(subtle()))
                };
                cells.push(note);
            }
            if show_when {
                let style = if m.held {
                    Style::new().fg(subtle())
                } else if m.date == today {
                    Style::new().fg(green())
                } else if m.date < today {
                    Style::new().fg(red())
                } else {
                    Style::new().fg(blue())
                };
                cells.push((m.when(), style));
            }

            RowSpec {
                prefix: vec![Span::styled(mark, mark_style)],
                title: m.title.clone(),
                title_style,
                cells,
            }
        })
        .collect();

    let mut items: Vec<ListItem> = Vec::with_capacity(project.meetings.len());
    for (i, line) in meta_rows(inner_w, specs).into_iter().enumerate() {
        let mut lines = vec![line];
        if app.meeting_info && i == app.meeting_idx {
            lines.extend(meeting_detail_lines(&project.meetings[i], today));
        }
        items.push(ListItem::new(lines));
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
    state.select(Some(app.meeting_idx));
    f.render_stateful_widget(list, area, &mut state);
}

/// The third pane on the Meetings tab: the selected meeting's agenda / minutes.
fn render_meeting_note(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Detail;

    let Some(m) = app.current_meeting() else {
        let block = panel(" Meeting ".to_string(), focused);
        f.render_widget(
            hint("Pick a meeting and press l to read its notes.").block(block),
            area,
        );
        return;
    };

    let block = panel(
        fit_title(format!(" {} · {} ", m.title, m.when()), area.width),
        focused,
    )
    .padding(Padding::horizontal(1));

    // Who's in it sits on the bottom border, where the list keeps its own dates.
    let block = if m.attendees.is_empty() {
        block
    } else {
        block.title_bottom(Line::from(Span::styled(
            format!(" \u{f0c0} {} ", m.attendees.join(", ")),
            Style::new().fg(subtle()),
        )))
    };

    if m.note.trim().is_empty() {
        f.render_widget(
            hint("No agenda yet.  Press N to write one in Markdown.").block(block),
            area,
        );
        return;
    }

    let rendered = app.note_body_lines(&m.note, area.width.saturating_sub(4));
    let total = rendered.len() as u16;
    let view = area.height.saturating_sub(2).max(1);
    let max_scroll = total.saturating_sub(view);
    let scroll = app.meeting_note_scroll.min(max_scroll);

    f.render_widget(
        Paragraph::new(rendered)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
    render_scroll_pct(f, area, scroll, max_scroll, total > view);
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

    let block = panel(
        fit_title(format!(" Note · {} ", note.text), area.width),
        focused,
    )
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
    render_scroll_pct(f, area, scroll, max_scroll, total > view);
}

/// `42%` in the pane's top-right corner, so a long note says how much is left.
/// Drawn only when the body actually overflows the pane.
fn render_scroll_pct(f: &mut Frame, area: Rect, scroll: u16, max_scroll: u16, overflows: bool) {
    if !overflows {
        return;
    }
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

fn render_edit_body_pane(f: &mut Frame, area: Rect, state: &crate::app::EditState, app: &App) {
    let name = match state.target {
        EditTarget::NoteBody(_) => app
            .current_note()
            .map(|n| n.text.clone())
            .unwrap_or_else(|| "note".into()),
        EditTarget::TodoNote(_) => app
            .current_todo()
            .map(|t| format!("note · {}", t.title))
            .unwrap_or_else(|| "todo note".into()),
        EditTarget::SubtaskNote(i) => app
            .current_todo()
            .and_then(|t| t.subtasks.get(i))
            .map(|s| format!("note · {}", s.title))
            .unwrap_or_else(|| "subtask note".into()),
        EditTarget::MeetingNote(_) => app
            .current_meeting()
            .map(|m| format!("meeting · {}", m.title))
            .unwrap_or_else(|| "meeting notes".into()),
    };

    let split = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).split(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            fit_title(format!(" edit · {name} "), area.width),
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
    let block = content_block(app, area.width);

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

    let block = if let Some(e) = entries.get(app.timeline_idx) {
        let style = if !e.done && e.date < today {
            Style::new().fg(red()).bold()
        } else if e.date == today {
            Style::new().fg(green())
        } else {
            Style::new().fg(subtle())
        };
        let label = if e.date == today {
            "today".to_string()
        } else {
            rel(e.date, today)
        };
        block.title_bottom(Line::from(Span::styled(format!(" {label} "), style)))
    } else {
        block
    };

    let inner_w = list_inner_w(area);
    let specs: Vec<RowSpec> = entries
        .iter()
        .map(|e| {
            let (icon, icon_style) = match e.kind {
                TlKind::Milestone => ("◆ ", Style::new().fg(accent())),
                TlKind::Todo if e.done => ("✔ ", Style::new().fg(green())),
                TlKind::Todo => ("○ ", Style::new().fg(subtle())),
            };
            let overdue = !e.done && e.date < today;
            let date_style = if overdue {
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
            let rel_style = if overdue {
                Style::new().fg(red()).bold()
            } else if e.date == today {
                Style::new().fg(green())
            } else {
                Style::new().fg(subtle())
            };
            RowSpec {
                prefix: vec![
                    Span::styled(format!("{}  ", e.date.format("%Y-%m-%d")), date_style),
                    Span::styled(icon, icon_style),
                ],
                title: e.label.clone(),
                title_style: label_style,
                cells: vec![(rel(e.date, today), rel_style)],
            }
        })
        .collect();
    let items: Vec<ListItem> = meta_rows(inner_w, specs)
        .into_iter()
        .map(ListItem::new)
        .collect();

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

/// The `^l` panel: two side-by-side tables — app events on the left, the log of
/// data changes on the right.
fn render_activity(f: &mut Frame, area: Rect, app: &App) {
    let cols =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);
    render_log_table(f, cols[0], "Logs", &app.logs);
    render_log_table(f, cols[1], "Changes", &app.changes);
}

fn render_log_table(f: &mut Frame, area: Rect, title: &str, entries: &[LogEntry]) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border()))
        .title(Span::styled(
            format!(" {title} "),
            Style::new().fg(subtle()).bold(),
        ))
        .title(
            Line::from(Span::styled(
                format!(" {} ", entries.len()),
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        );

    if entries.is_empty() {
        f.render_widget(hint("nothing yet").block(block), area);
        return;
    }

    // Show the tail that fits — newest at the bottom, like a console.
    let visible = area.height.saturating_sub(2) as usize;
    let start = entries.len().saturating_sub(visible);
    let rows = entries[start..].iter().map(|e| {
        Row::new(vec![
            Line::from(Span::styled(
                e.at.format("%H:%M:%S").to_string(),
                Style::new().fg(subtle()),
            )),
            Line::from(Span::styled(e.text.clone(), Style::new().fg(text()))),
        ])
    });
    let table = Table::new(rows, [Constraint::Length(8), Constraint::Min(0)]).block(block);
    f.render_widget(table, area);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    // vim-style mode + location segments on the left.
    let (mode_label, mode_color) = match &app.mode {
        Mode::Input(_) | Mode::EditBody(_) | Mode::Search(_) => ("I", green()),
        Mode::Confirm(_) => ("CONFIRM", red()),
        Mode::Notice(..) => ("NOTICE", red()),
        Mode::Normal
        | Mode::Help
        | Mode::GitHub
        | Mode::Weather
        | Mode::Theme(_)
        | Mode::Attach(_)
        | Mode::Tags(_)
        | Mode::Links(_)
        | Mode::Menu(_)
        | Mode::Sort(_) => ("N", accent()),
    };
    let pane_label = match app.focus {
        _ if app.note_full => "NOTE·FULL",
        Focus::Projects => "PROJECTS",
        Focus::Content => match app.tab {
            Tab::Overview => "OVERVIEW",
            Tab::Todos => "TODOS",
            Tab::Notes => "NOTES",
            Tab::Schedule => "SCHEDULE",
            Tab::Meetings => "MEETINGS",
        },
        Focus::Detail => match app.tab {
            Tab::Notes => "NOTE",
            Tab::Meetings => "MINUTES",
            _ if app.sub_note_focus => "SUB·NOTE",
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
    // \u{f09b} github ·  \u{f021} sync-arrows ·  \u{f0ee} cloud-upload. Sync
    // *results* land in a toast, not here — this is just the standing state.
    if app.sync_in_flight {
        spans.push(Span::styled(" \u{f021} ", Style::new().fg(yellow()).bold()));
    } else if app.sync_ready() {
        spans.push(Span::styled(" \u{f09b} ", Style::new().fg(green()).bold()));
        if app.sync_pending > 0 {
            spans.push(Span::styled(
                format!(" \u{f0ee} {} ", app.sync_pending),
                Style::new().fg(yellow()),
            ));
        }
    }
    // Transient feedback that isn't a logged data change (hints, errors).
    if matches!(app.mode, Mode::Normal) && !app.status.is_empty() {
        spans.push(Span::styled(
            format!("  {}", app.status),
            Style::new().fg(accent()),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);

    // Weather + clock, centred in the footer (skipped when it would collide with
    // the pills on a narrow terminal).
    let mid = weather_clock_line(app);
    if area.width >= 56 {
        f.render_widget(Paragraph::new(mid), area);
    }

    // Right-aligned breadcrumb, only when there's comfortable room for it.
    let breadcrumb = if app.minimal {
        String::new()
    } else {
        build_breadcrumb(app)
    };
    let bc_width = breadcrumb.chars().count() as u16;
    // Needs room on the right half without crowding the centred weather/clock.
    if !breadcrumb.is_empty() && area.width > bc_width.saturating_mul(2) + 34 {
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
            Tab::Meetings => parts.push("meetings".into()),
        },
        Focus::Detail => match app.tab {
            Tab::Todos => {
                parts.push("todos".into());
                if let Some(t) = app.current_todo() {
                    parts.push(truncate(&t.title, 20));
                }
                if app.sub_note_focus
                    && let Some(s) = app
                        .current_todo()
                        .and_then(|t| t.subtasks.get(app.subtask_idx))
                {
                    parts.push(format!("{} · note", truncate(&s.title, 16)));
                }
            }
            Tab::Notes => {
                parts.push("notes".into());
                if let Some(n) = app.current_note() {
                    parts.push(truncate(&n.text, 20));
                }
            }
            Tab::Meetings => {
                parts.push("meetings".into());
                if let Some(m) = app.current_meeting() {
                    parts.push(truncate(&m.title, 20));
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
    // Count the rows the text will occupy once wrapped, not just the newlines —
    // GitHub's errors arrive as one long sentence.
    let wrapped: usize = body
        .split('\n')
        .map(|l| {
            l.chars()
                .count()
                .div_ceil((width as usize).saturating_sub(6).max(1))
        })
        .map(|rows| rows.max(1))
        .sum();
    let height = (wrapped as u16 + 4).clamp(6, area.height.saturating_sub(2));
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

/// A transient "sonner" — a small notification in the bottom-right that the app
/// clears on its own after a few seconds (see `App::tick_toast`).
fn render_toast(f: &mut Frame, area: Rect, toast: &Toast) {
    let (accent_c, glyph) = match toast.kind {
        ToastKind::Success => (green(), "\u{f058}"),
        ToastKind::Error => (red(), "\u{f057}"),
        ToastKind::Info => (accent(), "\u{f05a}"),
    };
    let text_w = toast.text.chars().count() as u16;
    let w = (text_w + 7).min(area.width.saturating_sub(2)).max(12);
    let h = 3;
    if area.width < w + 2 || area.height < h + 3 {
        return;
    }
    let rect = Rect {
        x: area.x + area.width - w - 2,
        y: area.y + area.height - h - 2, // clear of the footer row
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent_c))
        .style(Style::new().bg(bg()))
        .padding(Padding::horizontal(1));
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{glyph}  "), Style::new().fg(accent_c).bold()),
            Span::styled(toast.text.clone(), Style::new().fg(text())),
        ]))
        .block(block),
        rect,
    );
}

fn render_weather(f: &mut Frame, area: Rect, app: &App) {
    let Some(w) = &app.weather else {
        return;
    };
    let u = w.deg();
    let c = &w.current;
    let sub = |s: String| Line::from(Span::styled(s, Style::new().fg(subtle())));

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                format!("  {}  ", w.glyph()),
                Style::new().fg(accent()).bold(),
            ),
            Span::styled(
                format!("{}°{u}", w.temp_i()),
                Style::new().fg(text()).bold(),
            ),
            Span::styled(format!("   {}", w.label()), Style::new().fg(text())),
        ]),
        sub(format!(
            "  feels {}°{u}  ·  humidity {}%  ·  cloud {}%",
            c.feels_like.round() as i64,
            c.humidity,
            c.cloud_cover
        )),
        sub(format!(
            "  wind {} {} {}  ·  gusts {} {}",
            c.wind.round() as i64,
            w.unit.wind_label(),
            w.wind_compass(),
            c.wind_gust.round() as i64,
            w.unit.wind_label()
        )),
        sub(format!(
            "  pressure {} hPa  ·  precip {:.1} {}",
            c.pressure.round() as i64,
            c.precip,
            w.unit.precip_label()
        )),
        Line::from(""),
    ];

    for (i, d) in w.days.iter().enumerate() {
        let name = if i == 0 {
            "Today".to_string()
        } else {
            d.date.format("%a").to_string()
        };
        let mut parts = vec![
            Span::styled(format!("  {name:<7}"), Style::new().fg(text()).bold()),
            Span::styled(format!("{} ", d.glyph()), Style::new().fg(accent())),
            Span::styled(
                format!("{}° / {}°", d.t_max.round() as i64, d.t_min.round() as i64),
                Style::new().fg(text()),
            ),
        ];
        let mut extra = String::new();
        if i == 0 && !d.sunrise.is_empty() {
            extra.push_str(&format!("   rise {} · set {}", d.sunrise, d.sunset));
        }
        if let Some(uv) = d.uv_max {
            extra.push_str(&format!("   UV {}", uv.round() as i64));
        }
        if let Some(p) = d.precip_prob {
            extra.push_str(&format!("   rain {p}%"));
        }
        if !extra.is_empty() {
            parts.push(Span::styled(extra, Style::new().fg(subtle())));
        }
        lines.push(Line::from(parts));
    }

    lines.push(Line::from(""));
    lines.push(sub(format!("  updated {} · Open-Meteo", rel_time(w.at))));

    let width = area.width.saturating_sub(8).clamp(40, 74);
    let height = (lines.len() as u16 + 4).clamp(10, area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            format!(" Weather · {} ", w.place),
            Style::new().fg(accent()).bold(),
        ))
        .title(
            Line::from(Span::styled(" any key closes ", Style::new().fg(subtle()))).right_aligned(),
        )
        .padding(Padding::new(2, 2, 1, 1));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn render_confirm(f: &mut Frame, area: Rect, c: &ConfirmState) {
    let width = area.width.saturating_sub(8).clamp(24, 64);
    // Grow the box to fit a multi-line prompt (the settings-import diff), so the
    // y/n row is never pushed out of the frame.
    let prompt_w = (width as usize).saturating_sub(6).max(1);
    let prompt_h: usize = c
        .prompt
        .split('\n')
        .map(|l| l.chars().count().div_ceil(prompt_w).max(1))
        .sum();
    let height = (prompt_h as u16 + 6).min(area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    // Quit and settings imports aren't destructive — style them calmer than a
    // delete.
    let (edge, title) = match c.action {
        ConfirmAction::Quit => (accent(), " Quit "),
        ConfirmAction::ImportSettings(_) => (accent(), " Import settings "),
        _ => (red(), " Confirm "),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(edge))
        .title(Span::styled(title, Style::new().fg(edge).bold()))
        .padding(Padding::new(2, 2, 1, 1));
    let mut body: Vec<Line> = c
        .prompt
        .split('\n')
        .map(|l| Line::from(Span::styled(l.to_string(), Style::new().fg(text()))))
        .collect();
    body.push(Line::from(""));
    body.push(Line::from(vec![
        Span::styled("y", Style::new().fg(green()).bold()),
        Span::styled("  yes        ", Style::new().fg(subtle())),
        Span::styled("n", Style::new().fg(red()).bold()),
        Span::styled("  cancel", Style::new().fg(subtle())),
    ]));
    f.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: true }).block(block),
        rect,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    type Row = (&'static str, &'static str);

    const COL1: &[Row] = &[
        ("", "MOVE"),
        ("h j k l", "move · switch pane"),
        ("w  s", "prev / next project"),
        ("J  K", "reorder selected"),
        ("gg  G", "jump top / bottom"),
        ("esc", "step back · close"),
        ("", ""),
        ("", "VIEWS"),
        ("1 … 5", "views, strip order"),
        ("tab S-tab", "cycle view"),
        ("/", "fuzzy find"),
        ("m", "minimal view"),
        ("", ""),
        ("", "SYSTEM"),
        ("^k", "main menu"),
        ("?  q", "help · quit (asks; ^c skips)"),
        ("^t  ^e", "theme · edit settings"),
        ("^s  ^g", "save to GitHub · link repo"),
        ("^l  ^w", "activity panel · weather"),
    ];
    const COL2: &[Row] = &[
        ("", "PROJECTS"),
        ("a r d", "add · rename · delete"),
        ("l", "open"),
        ("i", "detail panel"),
        ("t", "tags"),
        ("o  R", "sort · repo view"),
        ("", ""),
        ("", "OVERVIEW"),
        ("e  r", "description · rename"),
        ("t", "tags"),
        ("", ""),
        ("", "SCHEDULE"),
        ("a", "add milestone"),
        ("e  d", "edit · delete"),
        ("x  r", "done · reschedule"),
        ("f", "cycle filter"),
        ("l", "jump to todo"),
        ("", ""),
        ("", "MEETINGS"),
        ("a e d", "add · edit · del"),
        ("x  r", "held · reschedule"),
        ("l  N", "read · edit notes"),
        ("i  o", "detail · sort"),
    ];
    const COL3: &[Row] = &[
        ("", "TODOS · SUBTASKS"),
        ("a e d", "add · edit · delete"),
        ("x  space", "toggle done"),
        ("p  o", "priority · sort"),
        ("l", "open subtasks"),
        ("i", "detail panel"),
        ("n  N", "view · edit note"),
        ("l / h", "focus note to scroll · back"),
        ("t  A", "tags · attachments"),
        ("", ""),
        ("", "NOTES"),
        ("a e d", "add · edit · delete"),
        ("x", "pin / unpin"),
        ("o", "sort menu"),
        ("l", "open body"),
        ("", ""),
        ("", "NOTE BODY / EDITOR"),
        ("j k ^d ^u", "scroll · page"),
        ("space", "expand / collapse"),
        ("^f", "full-screen note (esc out)"),
        ("L", "links → open / copy"),
        ("e  ^s", "edit · save & close"),
    ];

    let rows = COL1.len().max(COL2.len()).max(COL3.len()) as u16;
    let width = 96u16.min(area.width);
    let height = (rows + 6).min(area.height);
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
            Line::from(Span::styled(
                " any key to close ",
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let body = Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).split(inner);
    let cols = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .spacing(2)
    .split(body[0]);

    let render_col = |rows: &[Row], w: u16| -> Vec<Line<'static>> {
        rows.iter()
            .map(|(key, desc)| {
                if key.is_empty() && desc.is_empty() {
                    Line::from("")
                } else if key.is_empty() {
                    // Section header with a trailing rule.
                    let fill = (w as usize).saturating_sub(desc.chars().count() + 1);
                    Line::from(vec![
                        Span::styled(format!("{desc} "), Style::new().fg(accent()).bold()),
                        Span::styled("─".repeat(fill), Style::new().fg(border())),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(format!("{key:<10}"), Style::new().fg(text())),
                        Span::styled((*desc).to_string(), Style::new().fg(subtle())),
                    ])
                }
            })
            .collect()
    };

    f.render_widget(Paragraph::new(render_col(COL1, cols[0].width)), cols[0]);
    f.render_widget(Paragraph::new(render_col(COL2, cols[1].width)), cols[1]);
    f.render_widget(Paragraph::new(render_col(COL3, cols[2].width)), cols[2]);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("QUICK-ADD   ", Style::new().fg(accent()).bold()),
            Span::styled("!1 !2 !3", Style::new().fg(text())),
            Span::styled(" priority     ", Style::new().fg(subtle())),
            Span::styled("@YYYY-MM-DD", Style::new().fg(text())),
            Span::styled(" due     ", Style::new().fg(subtle())),
            Span::styled("#tag", Style::new().fg(text())),
            Span::styled(" tag     ", Style::new().fg(subtle())),
            Span::styled("14:30 +person", Style::new().fg(text())),
            Span::styled(" meeting", Style::new().fg(subtle())),
        ])),
        body[1],
    );
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

fn render_attach(f: &mut Frame, area: Rect, state: &AttachState, app: &App) {
    let width = area.width.saturating_sub(8).clamp(40, 84);
    let todo = app
        .current_project()
        .and_then(|p| p.todos.get(state.target.todo_idx));
    let empty: Vec<crate::model::Attachment> = Vec::new();
    let atts = app.attachments_at(state.target).unwrap_or(&empty);
    let height = (atts.len() as u16 + 6).clamp(8, area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    let name = match state.target.sub_idx {
        None => todo.map(|t| truncate(&t.title, 36)).unwrap_or_default(),
        Some(i) => todo
            .and_then(|t| t.subtasks.get(i))
            .map(|s| format!("↳ {}", truncate(&s.title, 34)))
            .unwrap_or_default(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            format!(" Attachments · {name} "),
            Style::new().fg(accent()).bold(),
        ))
        .title(
            Line::from(Span::styled(
                " a add · o open · d del ",
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    if atts.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "No attachments.  Press a to add a link, file or image path  (…  | label  to name it).",
                Style::new().fg(subtle()),
            ))
            .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = atts
        .iter()
        .map(|a| {
            let (tag, tag_style) = match a.kind() {
                AttachmentKind::Link => ("link", Style::new().fg(blue())),
                AttachmentKind::Image => ("img ", Style::new().fg(green())),
                AttachmentKind::File => ("file", Style::new().fg(yellow())),
            };
            let mut spans = vec![
                Span::styled(format!("{tag}  "), tag_style),
                Span::styled(truncate(a.display(), 52), Style::new().fg(text())),
            ];
            if !a.label.trim().is_empty() {
                spans.push(Span::styled(
                    format!("   {}", truncate(&a.value, 32)),
                    Style::new().fg(subtle()),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::new().bg(sel_bg()).bold())
        .highlight_symbol("▍");
    let mut ls = ListState::default();
    ls.select(Some(state.sel.min(atts.len().saturating_sub(1))));
    f.render_stateful_widget(list, inner, &mut ls);
}

fn render_tags(f: &mut Frame, area: Rect, state: &TagState, app: &App) {
    let width = area.width.saturating_sub(8).clamp(34, 60);
    let empty: Vec<String> = Vec::new();
    let tags = app.tags_at(state.target).unwrap_or(&empty);
    let height = (tags.len() as u16 + 6).clamp(8, area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    let name = match state.target {
        TagTarget::Project => app
            .current_project()
            .map(|p| truncate(&p.name, 34))
            .unwrap_or_default(),
        TagTarget::Todo(i) => app
            .current_project()
            .and_then(|p| p.todos.get(i))
            .map(|t| truncate(&t.title, 34))
            .unwrap_or_default(),
        TagTarget::Subtask { todo, sub } => app
            .current_project()
            .and_then(|p| p.todos.get(todo))
            .and_then(|t| t.subtasks.get(sub))
            .map(|s| format!("↳ {}", truncate(&s.title, 32)))
            .unwrap_or_default(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            format!(" Tags · {name} "),
            Style::new().fg(accent()).bold(),
        ))
        .title(
            Line::from(Span::styled(" a add · d del ", Style::new().fg(subtle()))).right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    if tags.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "No tags.  Press a to add one or more (space-separated).",
                Style::new().fg(subtle()),
            ))
            .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = tags
        .iter()
        .map(|t| {
            ListItem::new(Line::from(Span::styled(
                format!("#{t}"),
                Style::new().fg(blue()),
            )))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(Style::new().bg(sel_bg()).bold())
        .highlight_symbol("▍");
    let mut ls = ListState::default();
    ls.select(Some(state.sel.min(tags.len().saturating_sub(1))));
    f.render_stateful_widget(list, inner, &mut ls);
}

/// The `L` overlay: links pulled from the note on screen.
fn render_links(f: &mut Frame, area: Rect, state: &LinksState) {
    let width = area.width.saturating_sub(6).clamp(44, 92);
    let height = (state.items.len() as u16 + 6).clamp(8, area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(" Links ", Style::new().fg(accent()).bold()))
        .title(
            Line::from(Span::styled(
                " o open · y copy · esc close ",
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let url_w = inner.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = state
        .items
        .iter()
        .map(|(label, url)| {
            let mut lines = vec![Line::from(Span::styled(
                truncate(url, url_w.max(8)),
                Style::new().fg(blue()).add_modifier(Modifier::UNDERLINED),
            ))];
            if label != url {
                lines.push(Line::from(Span::styled(
                    format!("  {}", truncate(label, url_w.saturating_sub(2).max(6))),
                    Style::new().fg(subtle()),
                )));
            }
            ListItem::new(lines)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::new().bg(sel_bg()).bold())
        .highlight_symbol("▍");
    let mut ls = ListState::default();
    ls.select(Some(state.sel.min(state.items.len().saturating_sub(1))));
    f.render_stateful_widget(list, inner, &mut ls);
}

fn render_search(f: &mut Frame, area: Rect, state: &SearchState, app: &App) {
    let width = area.width.saturating_sub(6).clamp(40, 96);
    let height = area.height.saturating_sub(4).clamp(10, 26);
    let rect = popup(area, width, height);
    overlay(f, rect);

    let results = app.search_results();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(" Find ", Style::new().fg(accent()).bold()))
        .title(
            Line::from(Span::styled(
                format!(
                    " {} match{} ",
                    results.len(),
                    if results.len() == 1 { "" } else { "es" }
                ),
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let rows = Layout::vertical([
        Constraint::Length(1), // input
        Constraint::Length(1), // rule
        Constraint::Min(0),    // results
        Constraint::Length(1), // hint
    ])
    .split(inner);

    // Input line: "› <query>"
    f.render_widget(
        Paragraph::new(Span::styled("›", Style::new().fg(accent()).bold())),
        rows[0],
    );
    let inp = Rect {
        x: rows[0].x + 2,
        y: rows[0].y,
        width: rows[0].width.saturating_sub(2),
        height: 1,
    };
    let mut ta = state.editor.clone();
    ta.set_cursor_line_style(Style::new());
    ta.set_selection_style(Style::new().bg(sel_bg()));
    f.render_widget(&ta, inp);

    f.render_widget(
        Paragraph::new(Span::styled(
            "─".repeat(rows[1].width as usize),
            Style::new().fg(border()),
        )),
        rows[1],
    );

    let inner_w = rows[2].width.saturating_sub(1) as usize; // 1 = highlight gutter
    let crumb_max = (inner_w / 3).clamp(10, 40);
    let specs: Vec<RowSpec> = results
        .iter()
        .map(|h| {
            let (glyph, gstyle) = match h.target {
                SearchTarget::Project => ("●", Style::new().fg(accent())),
                SearchTarget::Todo(_) => ("○", Style::new().fg(subtle())),
                SearchTarget::Subtask { .. } => ("↳", Style::new().fg(subtle())),
                SearchTarget::Note(_) => ("¶", Style::new().fg(yellow())),
            };
            // Indent one step per nesting level so depth reads at a glance.
            let indent = "  ".repeat(h.target.depth());
            let crumbs = if h.crumbs.is_empty() {
                (String::new(), Style::default())
            } else {
                (
                    truncate(&h.crumbs.join("  ›  "), crumb_max),
                    Style::new().fg(subtle()),
                )
            };
            let context = match &h.context {
                Some(c) => (
                    truncate(c, crumb_max),
                    Style::new().fg(if c.contains("overdue") {
                        red()
                    } else {
                        subtle()
                    }),
                ),
                None => (String::new(), Style::default()),
            };
            RowSpec {
                prefix: vec![Span::styled(format!("{indent}{glyph} "), gstyle)],
                title: h.label.clone(),
                title_style: Style::new().fg(text()),
                cells: vec![context, crumbs],
            }
        })
        .collect();

    let items: Vec<ListItem> = if specs.is_empty() {
        vec![ListItem::new(Span::styled(
            "  no matches",
            Style::new().fg(subtle()),
        ))]
    } else {
        meta_rows(inner_w, specs)
            .into_iter()
            .map(ListItem::new)
            .collect()
    };

    let list = List::new(items)
        .highlight_style(Style::new().bg(sel_bg()).bold())
        .highlight_symbol("▍");
    let mut ls = ListState::default();
    if !results.is_empty() {
        ls.select(Some(state.sel.min(results.len() - 1)));
    }
    f.render_stateful_widget(list, rows[2], &mut ls);

    f.render_widget(
        Paragraph::new(Span::styled(
            "  ↑↓ move · enter open · esc cancel",
            Style::new().fg(subtle()),
        )),
        rows[3],
    );
}

/// The `^k` main menu: a hub for the global actions.
fn render_menu(f: &mut Frame, area: Rect, state: &MenuState) {
    let entries = MenuAction::ENTRIES;
    let width = area.width.saturating_sub(6).clamp(30, 44);
    let height = (entries.len() as u16 + 4).min(area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(" Menu ", Style::new().fg(accent()).bold()))
        .title(
            Line::from(Span::styled(
                " enter · esc close ",
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let hint_w = entries.iter().map(|e| e.3.len()).max().unwrap_or(0);
    let label_w = (inner.width as usize).saturating_sub(4 + hint_w + 1);
    let items: Vec<ListItem> = entries
        .iter()
        .map(|(_, glyph, label, hint)| {
            let label_txt = format!("{:<label_w$}", truncate(label, label_w.max(4)));
            ListItem::new(Line::from(vec![
                Span::styled(format!("{glyph}  "), Style::new().fg(accent())),
                Span::styled(label_txt, Style::new().fg(text())),
                Span::styled(format!(" {hint:>hint_w$}"), Style::new().fg(subtle())),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::new().bg(sel_bg()).bold())
        .highlight_symbol("▍");
    let mut ls = ListState::default();
    ls.select(Some(state.sel.min(entries.len().saturating_sub(1))));
    f.render_stateful_widget(list, inner, &mut ls);
}

/// The `o` ordering menu: the orderings the list in focus offers, each spelling
/// out which way round it runs, with the one already in force marked.
fn render_sort(f: &mut Frame, area: Rect, state: &SortState) {
    let keys = state.scope.keys();
    let width = area.width.saturating_sub(6).clamp(34, 50);
    let height = (keys.len() as u16 + 4).min(area.height.saturating_sub(2));
    let rect = popup(area, width, height);
    overlay(f, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent()))
        .title(Span::styled(
            format!(" Sort {} ", state.scope.label()),
            Style::new().fg(accent()).bold(),
        ))
        .title(
            Line::from(Span::styled(
                " enter · r reverse · esc ",
                Style::new().fg(subtle()),
            ))
            .right_aligned(),
        )
        .padding(Padding::symmetric(2, 1));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let hint_w = keys
        .iter()
        .map(|k| k.hint(state.reverse).chars().count())
        .max()
        .unwrap_or(0);
    let label_w = (inner.width as usize).saturating_sub(4 + hint_w + 3);
    let items: Vec<ListItem> = keys
        .iter()
        .map(|k| {
            // A dot marks the ordering the list is already in; the arrow shows
            // which way that one runs, which can differ from the pending flip.
            let active = state.active.filter(|o| o.key == *k);
            let mark = match active {
                Some(o) => {
                    let arrow = if o.reverse { "↑" } else { "↓" };
                    Span::styled(format!("{arrow} "), Style::new().fg(green()).bold())
                }
                None => Span::raw("  "),
            };
            let label_txt = format!("{:<label_w$}", truncate(k.label(), label_w.max(4)));
            let label_style = if active.is_some() {
                Style::new().fg(text()).bold()
            } else {
                Style::new().fg(text())
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{}  ", k.glyph()), Style::new().fg(accent())),
                mark,
                Span::styled(label_txt, label_style),
                Span::styled(
                    format!(" {:>hint_w$}", k.hint(state.reverse)),
                    Style::new().fg(subtle()),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .highlight_style(Style::new().bg(sel_bg()).bold())
        .highlight_symbol("▍");
    let mut ls = ListState::default();
    ls.select(Some(state.sel.min(keys.len().saturating_sub(1))));
    f.render_stateful_widget(list, inner, &mut ls);
}

// ---- helpers -------------------------------------------------------

/// Clip a pane title to the pane's own width (less the border cells) so the
/// header uses the whole span available rather than a fixed character budget.
fn fit_title(text: String, width: u16) -> String {
    truncate(&text, (width as usize).saturating_sub(4).max(8))
}

/// Content width of a bordered list pane: the pane minus its two borders and the
/// one-column highlight gutter the `List` widget reserves.
fn list_inner_w(area: Rect) -> usize {
    area.width.saturating_sub(3) as usize
}

/// One right-hand metadata cell — text plus its style. An empty string is a
/// blank slot that still holds the column's width, keeping everything aligned.
type Cell = (String, Style);

/// One list row before layout: a fixed `prefix` (mark, or date + icon), the
/// flexible title + its style, and metadata cells in left-to-right column order.
struct RowSpec<'a> {
    prefix: Vec<Span<'a>>,
    title: String,
    title_style: Style,
    cells: Vec<Cell>,
}

/// Lay out a batch of rows so the metadata lines up in fixed vertical columns:
/// every row gets the *same* title width, and each cell is right-aligned inside
/// its column (blank where a row has nothing). Columns that are empty for every
/// row — or that don't leave room for a title — are dropped from the right.
fn meta_rows<'a>(inner_w: usize, rows: Vec<RowSpec<'a>>) -> Vec<Line<'a>> {
    let n_cols = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let mut col_w = vec![0usize; n_cols];
    for r in &rows {
        for (j, (txt, _)) in r.cells.iter().enumerate() {
            if !txt.is_empty() {
                col_w[j] = col_w[j].max(txt.chars().count() + 2); // +2 = gap
            }
        }
    }
    let prefix_w = rows
        .iter()
        .map(|r| r.prefix.iter().map(Span::width).sum::<usize>())
        .max()
        .unwrap_or(0);

    // Trim columns from the right until a reasonable title still fits.
    let mut active = n_cols;
    while active > 0 && prefix_w + col_w[..active].iter().sum::<usize>() + 8 > inner_w {
        active -= 1;
    }
    let meta_w: usize = col_w[..active].iter().sum();

    rows.into_iter()
        .map(|r| {
            // Per-row title width so every line still totals `inner_w` (the meta
            // columns stay aligned) even when prefixes differ in length — the
            // fuzzy finder indents deeper items.
            let this_prefix_w: usize = r.prefix.iter().map(Span::width).sum();
            let title_w = inner_w.saturating_sub(this_prefix_w + meta_w).max(3);
            let shown = truncate(&r.title, title_w);
            let pad = title_w.saturating_sub(shown.chars().count());
            let mut spans = r.prefix;
            spans.push(Span::styled(shown, r.title_style));
            spans.push(Span::raw(" ".repeat(pad)));
            for (j, &w) in col_w.iter().take(active).enumerate() {
                if w == 0 {
                    continue; // column empty for every row
                }
                let (txt, style) = r.cells.get(j).cloned().unwrap_or_default();
                spans.push(Span::raw(" ".repeat(w.saturating_sub(txt.chars().count()))));
                if !txt.is_empty() {
                    spans.push(Span::styled(txt, style));
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// The compact priority cell: `↑` (high) / `↓` (low), blank for medium.
fn prio_cell(p: Priority) -> Cell {
    match p {
        Priority::High => ("↑".into(), Style::new().fg(red()).bold()),
        Priority::Low => ("↓".into(), Style::new().fg(blue())),
        Priority::Medium => (String::new(), Style::default()),
    }
}

/// `#a #b` joined for the `i` detail panel.
fn join_tags(tags: &[String]) -> String {
    tags.iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The left-hand label span for a row in an `i` detail panel.
fn info_label(s: &str) -> Span<'static> {
    Span::styled(format!("   {s:<11}"), Style::new().fg(subtle()))
}

fn info_row(label: &str, value: String, style: Style) -> Line<'static> {
    Line::from(vec![info_label(label), Span::styled(value, style)])
}

/// The inline detail panel shown under a todo when `i` is pressed.
fn todo_detail_lines(t: &crate::model::Todo, today: NaiveDate) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(""),
        info_row(
            "priority",
            t.priority.label().to_string(),
            Style::new().fg(text()),
        ),
    ];
    if let Some(d) = t.due {
        lines.push(info_row(
            "due",
            format!("{} · {}", d.format("%Y-%m-%d"), rel(d, today)),
            Style::new().fg(text()),
        ));
    }
    if !t.tags.is_empty() {
        lines.push(info_row(
            "tags",
            join_tags(&t.tags),
            Style::new().fg(blue()),
        ));
    }
    let (sd, st) = t.subtask_progress();
    if st > 0 {
        lines.push(info_row(
            "subtasks",
            format!("{sd}/{st}"),
            Style::new().fg(text()),
        ));
    }
    if !t.note.trim().is_empty() {
        lines.push(info_row(
            "note",
            "yes  (n / N)".into(),
            Style::new().fg(subtle()),
        ));
    }
    if !t.attachments.is_empty() {
        lines.push(info_row(
            "files",
            format!("{}  (A)", t.attachments.len()),
            Style::new().fg(subtle()),
        ));
    }
    lines.push(Line::from(""));
    lines
}

/// The inline detail panel shown under a subtask when `i` is pressed.
fn subtask_detail_lines(s: &crate::model::Subtask) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(""),
        info_row(
            "priority",
            s.priority.label().to_string(),
            Style::new().fg(text()),
        ),
    ];
    if !s.tags.is_empty() {
        lines.push(info_row(
            "tags",
            join_tags(&s.tags),
            Style::new().fg(blue()),
        ));
    }
    if !s.note.trim().is_empty() {
        lines.push(info_row(
            "note",
            "yes  (n / N)".into(),
            Style::new().fg(subtle()),
        ));
    }
    if !s.attachments.is_empty() {
        lines.push(info_row(
            "files",
            format!("{}  (A)", s.attachments.len()),
            Style::new().fg(subtle()),
        ));
    }
    lines.push(Line::from(""));
    lines
}

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

/// How far off a meeting is, in words. Unlike a deadline, a meeting in the past
/// isn't "overdue" — it just already happened.
fn meeting_rel(m: &Meeting, today: NaiveDate) -> String {
    let when = match (m.date - today).num_days() {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        -1 => "yesterday".to_string(),
        d if d < 0 => format!("{}d ago", -d),
        d => format!("in {d}d"),
    };
    match (&m.time, m.held) {
        (Some(t), false) => format!("{when} · {t}"),
        (_, true) => format!("{when} · held"),
        (None, false) => when,
    }
}

/// The `i` panel under a meeting row.
fn meeting_detail_lines(m: &Meeting, today: NaiveDate) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(""),
        info_row(
            "when",
            format!("{} · {}", m.date.format("%Y-%m-%d"), meeting_rel(m, today)),
            Style::new().fg(text()),
        ),
    ];
    lines.push(info_row(
        "attendees",
        if m.attendees.is_empty() {
            "nobody yet  (e · +name)".to_string()
        } else {
            m.attendees.join(", ")
        },
        Style::new().fg(if m.attendees.is_empty() {
            subtle()
        } else {
            text()
        }),
    ));
    lines.push(info_row(
        "notes",
        if m.note.trim().is_empty() {
            "none  (N)".to_string()
        } else {
            format!(
                "{} lines  (l / N)",
                m.note.lines().filter(|l| !l.trim().is_empty()).count()
            )
        },
        Style::new().fg(subtle()),
    ));
    lines.push(Line::from(""));
    lines
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

#[cfg(test)]
mod header_tests {
    use super::*;
    use crate::app::App;
    use crate::config::Config;
    use crate::model::Store;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// The header row as text, at a given terminal width.
    fn header(app: &mut App, width: u16) -> String {
        app.clamp();
        let mut terminal = Terminal::new(TestBackend::new(width, 6)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..width)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect()
    }

    #[test]
    fn the_name_stays_left_and_the_counts_go_right() {
        let mut app = App::new(Store::sample(), Config::default(), None, None);
        // One finished project and one overdue todo, so every segment shows.
        for t in &mut app.store.projects[1].todos {
            t.done = true;
        }
        for m in &mut app.store.projects[1].milestones {
            m.done = true;
        }
        app.store.projects[0].todos[1].due =
            Some(Local::now().date_naive() - chrono::Duration::days(3));

        let row = header(&mut app, 100);
        assert!(row.starts_with("  voido"), "{row:?}");
        assert_eq!(
            row.trim_end(),
            row.trim_end_matches(' '),
            "the counts keep a right-hand gap"
        );
        assert!(row.ends_with("1 overdue  "), "{row:?}");
        assert!(
            row.contains("2 projects  ·  1 done  ·  1 overdue"),
            "{row:?}"
        );

        // Too narrow for all three: the least useful segments drop first, so the
        // overdue count is the last one standing.
        let row = header(&mut app, 30);
        assert!(row.starts_with("  voido"), "{row:?}");
        assert!(row.ends_with("1 overdue  "), "{row:?}");
        assert!(!row.contains("projects"), "{row:?}");

        // `m` (minimal) leaves the name alone.
        app.minimal = true;
        assert_eq!(header(&mut app, 100).trim_end(), "  voido");
    }
}

#[cfg(test)]
mod note_pane_tests {
    use super::*;
    use crate::app::{App, Focus, Tab};
    use crate::config::Config;
    use crate::model::{Project, Store, Todo};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    /// One project, one todo carrying a note — enough to put a note on screen.
    fn app_with_todo_note() -> App {
        let mut project = Project::new("P");
        let mut todo = Todo::new("write it up");
        todo.note = "# Heading\n\nSome body text.".into();
        project.todos.push(todo);
        let mut store = Store::default();
        store.projects.push(project);

        let mut app = App::new(store, Config::default(), None, None);
        app.tab = Tab::Todos;
        app.focus = Focus::Content;
        app.todo_note_open = true;
        app
    }

    fn draw(app: &mut App) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        app.clamp();
        terminal.draw(|f| render(f, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Colour of a pane's top-left corner — its border style.
    fn corner(buf: &Buffer, r: Rect) -> Color {
        buf[(r.x, r.y)].style().fg.unwrap()
    }

    #[test]
    fn the_note_takes_the_accent_from_the_todo_list() {
        let mut app = app_with_todo_note();
        assert!(app.showing_todo_note(), "the note is on screen");
        let buf = draw(&mut app);
        let rects = *app.pane_rects.borrow();

        assert_eq!(corner(&buf, rects.detail), accent(), "the note is lit");
        assert_ne!(
            corner(&buf, rects.content),
            accent(),
            "the todo list it belongs to steps back"
        );
    }

    #[test]
    fn the_todo_list_is_lit_again_once_the_note_is_closed() {
        let mut app = app_with_todo_note();
        app.todo_note_open = false;
        let buf = draw(&mut app);
        let rects = *app.pane_rects.borrow();
        assert_eq!(corner(&buf, rects.content), accent());
    }

    #[test]
    fn moving_to_the_projects_rail_dims_the_note_too() {
        let mut app = app_with_todo_note();
        app.focus = Focus::Projects;
        let buf = draw(&mut app);
        let rects = *app.pane_rects.borrow();
        assert_eq!(corner(&buf, rects.projects), accent(), "one lit pane");
        assert_ne!(corner(&buf, rects.detail), accent());
    }

    #[test]
    fn a_full_screen_note_hides_the_panes() {
        let mut app = app_with_todo_note();
        app.note_full = true;
        let buf = draw(&mut app);
        let rects = *app.pane_rects.borrow();

        assert_eq!(rects.projects.area(), 0, "no projects rail");
        assert_eq!(rects.content.area(), 0, "no todo list");
        assert!(rects.detail.width > 100, "the note has the whole width");

        let top: String = (0..120)
            .map(|x| buf[(x, rects.detail.y)].symbol())
            .collect();
        assert!(top.contains("Note · write it up"), "{top}");
        assert!(top.contains("esc close"), "{top}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(title: &str, cells: &[&str]) -> RowSpec<'static> {
        RowSpec {
            prefix: vec![Span::raw("○ ")],
            title: title.to_string(),
            title_style: Style::default(),
            cells: cells
                .iter()
                .map(|c| (c.to_string(), Style::default()))
                .collect(),
        }
    }

    #[test]
    fn meta_rows_keeps_every_row_the_same_width() {
        // Rows with wildly different content — including one that has *no*
        // metadata at all — must still line up (this was the "no subtasks looks
        // weird" bug).
        let rows = vec![
            spec("short", &["↑", "⊞1/3", "Sep 01"]),
            spec("a considerably longer todo title", &["", "", "Sep 09"]),
            spec("mid length one", &["↓", "", ""]),
        ];
        let lines = meta_rows(60, rows);
        let w0 = lines[0].width();
        assert!(w0 <= 60);
        assert!(
            lines.iter().all(|l| l.width() == w0),
            "columns stay aligned"
        );
    }

    #[test]
    fn meta_rows_drops_columns_when_too_narrow() {
        let cells = ["↑", "⊞1/3", "¶", "Sep 01"];
        let wide = meta_rows(60, vec![spec("title", &cells)]);
        let narrow = meta_rows(18, vec![spec("title", &cells)]);
        assert!(narrow[0].width() <= 18);
        assert!(wide[0].width() > narrow[0].width());
    }

    #[test]
    fn meta_rows_aligns_meta_with_uneven_prefixes() {
        // The fuzzy finder indents deeper hits — different prefix widths must
        // still leave the right-hand column aligned (every row == inner_w).
        let mk = |prefix: &str, title: &str, crumb: &str| RowSpec {
            prefix: vec![Span::raw(prefix.to_string())],
            title: title.to_string(),
            title_style: Style::default(),
            cells: vec![(crumb.to_string(), Style::default())],
        };
        let lines = meta_rows(
            50,
            vec![
                mk("● ", "Website", ""),
                mk("    ↳ ", "token refresh", "Website › OAuth"),
            ],
        );
        assert_eq!(lines[0].width(), lines[1].width());
    }
}
