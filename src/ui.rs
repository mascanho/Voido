//! Rendering. A three-row shell (header / body / status) with a two-pane body.

use chrono::{Local, NaiveDate};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Tabs, Wrap,
    },
};

use crate::app::{App, ConfirmState, Focus, InputState, Mode, Tab, TlKind, TimelineEntry};
use crate::model::Priority;

pub(crate) const ACCENT: Color = Color::Rgb(203, 166, 247); // mauve
pub(crate) const GREEN: Color = Color::Rgb(166, 227, 161);
pub(crate) const RED: Color = Color::Rgb(243, 139, 168);
pub(crate) const YELLOW: Color = Color::Rgb(249, 226, 175);
pub(crate) const BLUE: Color = Color::Rgb(137, 180, 250);
pub(crate) const SUBTLE: Color = Color::Rgb(127, 132, 156);
pub(crate) const BORDER: Color = Color::Rgb(69, 71, 90);
pub(crate) const SEL_BG: Color = Color::Rgb(49, 50, 68);

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .split(area);

    render_header(f, rows[0], app);

    if app.tab == Tab::Todos || app.tab == Tab::Notes {
        let body = Layout::horizontal([
            Constraint::Length(42),
            Constraint::Min(22),
            Constraint::Percentage(48),
        ])
        .split(rows[1]);
        render_projects(f, body[0], app);
        render_content(f, body[1], app);
        if app.tab == Tab::Todos {
            render_subtasks(f, body[2], app);
        } else if let Mode::EditBody(state) = &app.mode {
            render_edit_body_pane(f, body[2], state, app);
        } else {
            render_note_body(f, body[2], app);
        }
    } else {
        let body =
            Layout::horizontal([Constraint::Length(42), Constraint::Min(24)]).split(rows[1]);
        render_projects(f, body[0], app);
        render_content(f, body[1], app);
    }

    render_footer(f, rows[2], app);

    match &app.mode {
        Mode::Input(input) => render_input(f, area, input),
        Mode::Confirm(c) => render_confirm(f, area, c),
        Mode::Help => render_help(f, area),
        Mode::EditBody(_) => {}
        Mode::Normal => {}
    }
}

fn render_header(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(13)]).split(area);
    let title = Line::from(vec![
        Span::styled("  shiki", Style::new().fg(ACCENT).bold()),
        Span::styled("   todos · projects · timelines", Style::new().fg(SUBTLE)),
        Span::styled(
            format!("    {} projects", app.store.projects.len()),
            Style::new().fg(BORDER),
        ),
    ]);
    let date = Line::from(Span::styled(
        format!("{}  ", Local::now().date_naive()),
        Style::new().fg(SUBTLE),
    ))
    .right_aligned();
    f.render_widget(Paragraph::new(title), cols[0]);
    f.render_widget(Paragraph::new(date), cols[1]);
}

fn render_projects(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Projects;
    let block = panel(" Projects ".to_string(), focused);

    if app.store.projects.is_empty() {
        f.render_widget(hint("No projects yet.  Press a to create one.").block(block), area);
        return;
    }

    let items: Vec<ListItem> = app
        .store
        .projects
        .iter()
        .map(|p| {
            let open = p.open_todos();
            let dot = if open == 0 {
                Span::styled("● ", Style::new().fg(GREEN))
            } else {
                Span::styled("● ", Style::new().fg(ACCENT))
            };
            let tail = if open == 0 {
                Span::styled("  clear", Style::new().fg(SUBTLE))
            } else {
                Span::styled(format!("  {open}"), Style::new().fg(SUBTLE))
            };
            ListItem::new(Line::from(vec![
                dot,
                Span::styled(p.name.clone(), Style::new().fg(Color::White)),
                tail,
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(SEL_BG).fg(ACCENT).bold()
        } else {
            Style::new()
        })
        .highlight_symbol(if focused { "▍ " } else { "  " });
    let mut state = ListState::default();
    state.select(Some(app.project_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_content(f: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(area);
    render_tabs(f, rows[0], app);
    match app.tab {
        Tab::Overview => render_overview(f, rows[1], app),
        Tab::Todos => render_todos(f, rows[1], app),
        Tab::Notes => render_notes(f, rows[1], app),
        Tab::Schedule => render_timeline(f, rows[1], app),
    }
}

fn render_tabs(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content;
    let name = app
        .current_project()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "no project".into());
    let selected = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    let titles: Vec<String> = Tab::ALL.iter().map(|t| format!("  {}  ", t.title())).collect();
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::new().fg(SUBTLE))
        .highlight_style(Style::new().fg(ACCENT).bold().bg(SEL_BG))
        .divider("")
        .block(panel(format!(" {name} "), focused));
    f.render_widget(tabs, area);
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Overview;
    let block = panel(" Overview ".to_string(), focused);

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

    let mut lines = vec![
        Line::from(Span::styled(
            p.name.clone(),
            Style::new().fg(Color::White).bold(),
        )),
        Line::from(Span::styled(
            if p.description.is_empty() {
                "no description — press e to add one".to_string()
            } else {
                p.description.clone()
            },
            Style::new().fg(SUBTLE),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Todos     ", Style::new().fg(SUBTLE)),
            Span::styled(format!("{done}/{total} done"), Style::new().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("          ", Style::new().fg(SUBTLE)),
            Span::styled(progress_bar(done, total, 22), Style::new().fg(GREEN)),
        ]),
    ];

    if subs > 0 {
        lines.push(Line::from(vec![
            Span::styled("Subtasks  ", Style::new().fg(SUBTLE)),
            Span::styled(
                format!("{subs_done}/{subs} done"),
                Style::new().fg(Color::White),
            ),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Notes     ", Style::new().fg(SUBTLE)),
        Span::styled(format!("{}", p.notes.len()), Style::new().fg(Color::White)),
    ]));
    lines.push(Line::from(""));

    match p.next_milestone() {
        Some(m) => lines.push(Line::from(vec![
            Span::styled("Next      ", Style::new().fg(SUBTLE)),
            Span::styled("◆ ", Style::new().fg(ACCENT)),
            Span::styled(m.title.clone(), Style::new().fg(Color::White)),
            Span::styled(
                format!("  {} · {}", m.date.format("%Y-%m-%d"), rel(m.date, today)),
                Style::new().fg(SUBTLE),
            ),
        ])),
        None => lines.push(Line::from(vec![
            Span::styled("Next      ", Style::new().fg(SUBTLE)),
            Span::styled("no upcoming milestones", Style::new().fg(SUBTLE)),
        ])),
    }

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(block),
        area,
    );
}

fn render_notes(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Notes;
    let block = panel(" Notes ".to_string(), focused);

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
                ("★ ", Style::new().fg(YELLOW))
            } else {
                ("• ", Style::new().fg(SUBTLE))
            };
            let mut spans = vec![
                Span::styled(mark, mark_style),
                Span::styled(n.text.clone(), Style::new().fg(Color::White)),
            ];
            if !n.body.trim().is_empty() {
                spans.push(Span::styled(" ¶", Style::new().fg(SUBTLE)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(SEL_BG).bold()
        } else {
            Style::new()
        })
        .highlight_symbol(if focused { "▍ " } else { "  " });
    let mut state = ListState::default();
    state.select(Some(app.note_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_todos(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Todos;
    let block = panel(" Todos ".to_string(), focused);

    let Some(project) = app.current_project() else {
        f.render_widget(hint("Create a project to start adding todos.").block(block), area);
        return;
    };
    if project.todos.is_empty() {
        f.render_widget(hint("No todos yet.  Press a to add one.").block(block), area);
        return;
    }

    let today = Local::now().date_naive();
    let items: Vec<ListItem> = project
        .todos
        .iter()
        .map(|t| {
            let (mark, mark_style) = if t.done {
                ("✔ ", Style::new().fg(GREEN))
            } else {
                ("○ ", Style::new().fg(SUBTLE))
            };
            let title_style = if t.done {
                Style::new().fg(SUBTLE).add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::new().fg(Color::White)
            };
            let prio_style = match t.priority {
                Priority::High => Style::new().fg(RED),
                Priority::Medium => Style::new().fg(YELLOW),
                Priority::Low => Style::new().fg(BLUE),
            };
            let mut spans = vec![
                Span::styled(mark, mark_style),
                Span::styled(t.title.clone(), title_style),
                Span::styled(format!("  {}", t.priority.label()), prio_style),
            ];
            if let Some(d) = t.due {
                let style = if !t.done && d < today {
                    Style::new().fg(RED).bold()
                } else {
                    Style::new().fg(SUBTLE)
                };
                spans.push(Span::styled(format!("  {}", d.format("%b %d")), style));
            }
            let (sdone, stotal) = t.subtask_progress();
            if stotal > 0 {
                let style = if sdone == stotal {
                    Style::new().fg(GREEN)
                } else {
                    Style::new().fg(SUBTLE)
                };
                spans.push(Span::styled(format!("  ⊞ {sdone}/{stotal}"), style));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(SEL_BG).bold()
        } else {
            Style::new()
        })
        .highlight_symbol(if focused { "▍ " } else { "  " });
    let mut state = ListState::default();
    state.select(Some(app.todo_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_subtasks(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Detail;

    let Some(todo) = app.current_todo() else {
        let block = panel(" Subtasks ".to_string(), focused);
        f.render_widget(
            hint("Pick a todo and press l to break it into subtasks.").block(block),
            area,
        );
        return;
    };

    let parent_name = truncate(&todo.title, 28);
    let (done, total) = todo.subtask_progress();
    let title = if total > 0 {
        format!(" {parent_name}  {done}/{total} ")
    } else {
        format!(" {parent_name} ")
    };
    let block = panel(title, focused);

    if todo.subtasks.is_empty() {
        f.render_widget(
            hint("No subtasks yet.  Press a to add the first one.").block(block),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = todo
        .subtasks
        .iter()
        .map(|s| {
            let (mark, mark_style) = if s.done {
                ("✔ ", Style::new().fg(GREEN))
            } else {
                ("○ ", Style::new().fg(SUBTLE))
            };
            let title_style = if s.done {
                Style::new().fg(SUBTLE).add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::new().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::styled(mark, mark_style),
                Span::styled(s.title.clone(), title_style),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(SEL_BG).bold()
        } else {
            Style::new()
        })
        .highlight_symbol(if focused { "▍ " } else { "  " });
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

    let block = panel(format!(" Note · {} ", truncate(&note.text, 24)), focused);

    if note.body.trim().is_empty() {
        f.render_widget(
            hint("Empty note.  Press e to write it in Markdown.").block(block),
            area,
        );
        return;
    }

    let rendered = crate::md::render(&note.body, area.width.saturating_sub(4));
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
                Paragraph::new(Span::styled(tag, Style::new().fg(SUBTLE))),
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
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(
            format!(" edit · {title} "),
            Style::new().fg(ACCENT).bold(),
        ))
        .padding(Padding::horizontal(1));

    let mut ta = state.textarea.clone();
    ta.set_block(block);
    ta.set_cursor_line_style(Style::new());
    ta.set_selection_style(Style::new().bg(SEL_BG));
    ta.set_line_number_style(Style::new().fg(BORDER));
    f.render_widget(&ta, split[0]);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("  esc", Style::new().fg(ACCENT).bold()),
            Span::styled(" / ", Style::new().fg(SUBTLE)),
            Span::styled("^s", Style::new().fg(ACCENT).bold()),
            Span::styled(
                "  save & close     markdown: # heading · - list · **bold** · `code` · > quote",
                Style::new().fg(SUBTLE),
            ),
        ])),
        split[1],
    );
}

fn render_timeline(f: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Content && app.tab == Tab::Schedule;
    let block = panel(" Schedule ".to_string(), focused);

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
            TlKind::Milestone => ("◆ ", Style::new().fg(ACCENT)),
            TlKind::Todo if e.done => ("✔ ", Style::new().fg(GREEN)),
            TlKind::Todo => ("○ ", Style::new().fg(SUBTLE)),
        };
        let date_style = if !e.done && e.date < today {
            Style::new().fg(RED)
        } else if e.date == today {
            Style::new().fg(GREEN)
        } else {
            Style::new().fg(BLUE)
        };
        let label_style = if e.done {
            Style::new().fg(SUBTLE).add_modifier(Modifier::CROSSED_OUT)
        } else {
            Style::new().fg(Color::White)
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!("{}  ", e.date.format("%Y-%m-%d")), date_style),
            Span::styled(icon, icon_style),
            Span::styled(e.label.clone(), label_style),
            Span::styled(format!("   {}", rel(e.date, today)), Style::new().fg(SUBTLE)),
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
                Paragraph::new(Span::styled(bar_text, Style::new().fg(SUBTLE))),
                r,
            );
        }
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::new().bg(SEL_BG).bold()
        } else {
            Style::new()
        })
        .highlight_symbol(if focused { "▍ " } else { "  " });
    let mut state = ListState::default();
    state.select(Some(app.timeline_idx));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    let keys = match &app.mode {
        Mode::Input(_) => "enter  confirm      esc  cancel",
        Mode::Confirm(_) => "y  yes      n  cancel",
        Mode::EditBody(_) => "esc / ^s  save & close",
        Mode::Help => "any key  close",
        Mode::Normal => match app.focus {
            Focus::Projects => {
                "j/k move   a add   r rename   d delete   l open   tab panel   ? help   q quit"
            }
            Focus::Content => match app.tab {
                Tab::Overview => "e edit description   r rename project   o/t/n/d switch tab   h back",
                Tab::Todos => {
                    "j/k move   a add   e edit   x done   p priority   J/K reorder   l subtasks   d delete   t tab"
                }
                Tab::Notes => {
                    "j/k move   a add   e edit   x pin   J/K reorder   l open   d delete   t tab"
                }
                Tab::Schedule => {
                    "j/k move   a add   x done   r reschedule   Enter jump   d delete   0/1/2/3 filter"
                }
            },
            Focus::Detail => match app.tab {
                Tab::Notes => "j/k scroll   ^d/^u page   e edit markdown   h back",
                _ => "j/k move   a add   e edit   x done   J/K reorder   d delete   h back to todo",
            },
        },
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::new().fg(Color::Black).bg(ACCENT).bold(),
        ),
        Span::raw("  "),
        Span::styled(keys, Style::new().fg(SUBTLE)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// ---- overlays -------------------------------------------------------

fn render_input(f: &mut Frame, area: Rect, input: &InputState) {
    let width = area.width.saturating_sub(8).clamp(24, 74);
    let rect = popup(area, width, 5);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(
            format!(" {} ", input.title),
            Style::new().fg(ACCENT).bold(),
        ))
        .padding(Padding::new(2, 2, 1, 1));
    let body = Line::from(vec![
        Span::styled("› ", Style::new().fg(ACCENT)),
        Span::styled(input.value.clone(), Style::new().fg(Color::White)),
    ]);
    f.render_widget(Paragraph::new(body).block(block), rect);

    let cursor_x = rect.x + 5 + input.value.chars().count() as u16;
    let max_x = rect.x + rect.width.saturating_sub(2);
    f.set_cursor_position((cursor_x.min(max_x), rect.y + 2));
}

fn render_confirm(f: &mut Frame, area: Rect, c: &ConfirmState) {
    let width = area.width.saturating_sub(8).clamp(24, 64);
    let rect = popup(area, width, 7);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(RED))
        .title(Span::styled(" Confirm ", Style::new().fg(RED).bold()))
        .padding(Padding::new(2, 2, 1, 1));
    let text = vec![
        Line::from(Span::styled(c.prompt.clone(), Style::new().fg(Color::White))),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::new().fg(GREEN).bold()),
            Span::styled("  yes        ", Style::new().fg(SUBTLE)),
            Span::styled("n", Style::new().fg(RED).bold()),
            Span::styled("  cancel", Style::new().fg(SUBTLE)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: true }).block(block),
        rect,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    let raw = [
        "  Global",
        "    1 2 3            focus panel (projects / middle / right)",
        "    tab              cycle panels",
        "    t / T            next / previous tab",
        "    o t n s          jump to Overview / Todos / Notes / Schedule",
        "    gg / G           jump to top / bottom",
        "    ? / q            help / quit",
        "",
        "  Tabs   Overview · Todos · Notes · Schedule",
        "    Overview         e edit description · r rename project",
        "",
        "  Projects",
        "    j k              move selection",
        "    a / r / d        add / rename / delete",
        "    l / enter        open project",
        "",
        "  Todos",
        "    j k              move selection",
        "    a / e            add / edit",
        "    x                toggle done",
        "    p                cycle priority",
        "    J K              reorder",
        "    l / enter        open subtasks (right pane)",
        "    d / h            delete / back to projects",
        "",
        "  Subtasks",
        "    j k              move selection",
        "    a / e            add / edit",
        "    x                toggle done",
        "    J K / d / h      reorder / delete / back",
        "",
        "  Notes",
        "    j k              move selection",
        "    a / e            add note / edit title",
        "    x                pin / unpin",
        "    J K              reorder",
        "    l / enter        open the note (right pane)",
        "    d                delete",
        "",
        "  Note body  (rendered Markdown)",
        "    j k / ^d ^u      scroll",
        "    e / i / enter    edit in the Markdown editor",
        "    editor           esc or ^s to save & close",
        "    syntax           # heading · - list · 1. list · > quote",
        "                     **bold** · *italic* · `code` · ``` fence ``` · ---",
        "",
        "  Schedule",
        "    j k              move selection",
        "    a                add milestone",
        "    x                toggle done (todo or milestone)",
        "    r                reschedule date",
        "    enter / l        jump to todo (or edit milestone)",
        "    d                delete",
        "    0 1 2 3          filter: all / overdue / today / this week",
        "",
        "  Quick-add syntax",
        "    ship it !3 @2026-09-15   →   !1..!3 priority, @date due",
    ];
    let height = (raw.len() as u16 + 2).min(area.height);
    let rect = popup(area, 60u16.min(area.width), height);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(" Keybindings ", Style::new().fg(ACCENT).bold()))
        .padding(Padding::horizontal(2));
    let lines: Vec<Line> = raw
        .iter()
        .map(|l| {
            let heading = l.starts_with("  ") && !l.starts_with("    ");
            let style = if heading {
                Style::new().fg(ACCENT).bold()
            } else {
                Style::new().fg(Color::White)
            };
            Line::from(Span::styled((*l).to_string(), style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

// ---- helpers -------------------------------------------------------

fn panel(title: String, focused: bool) -> Block<'static> {
    let color = if focused { ACCENT } else { SUBTLE };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { BORDER }))
        .title(Span::styled(title, Style::new().fg(color).bold()))
        .padding(Padding::horizontal(1))
}

fn hint(text: &str) -> Paragraph<'static> {
    Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {text}"),
            Style::new().fg(SUBTLE),
        )),
    ])
    .wrap(Wrap { trim: true })
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
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
