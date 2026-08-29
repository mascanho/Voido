//! Application state and Vim-style key handling.

use chrono::{Local, NaiveDate};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::TextArea;

use crate::github::{GitHubClient, RepoInfo};
use crate::model::{Milestone, Note, Priority, Project, Store, Subtask, Todo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Projects,
    /// The middle pane: the todo list, notes, timeline or overview, per `Tab`.
    Content,
    /// The right pane: subtasks of the selected todo (Todos tab) or subnotes of
    /// the selected note (Notes tab).
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Todos,
    Notes,
    Schedule,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Overview, Tab::Todos, Tab::Notes, Tab::Schedule];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Todos => "Todos",
            Tab::Notes => "Notes",
            Tab::Schedule => "Schedule",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlKind {
    Todo,
    Milestone,
}

/// A row in the merged, date-sorted timeline view.
#[derive(Clone)]
#[allow(dead_code)]
pub struct TimelineEntry {
    pub date: NaiveDate,
    pub label: String,
    pub kind: TlKind,
    pub done: bool,
    /// Index into the project's `milestones` vec, when this row is a milestone.
    pub milestone_idx: Option<usize>,
    /// Index into the project's `todos` vec, when this row is a todo.
    pub todo_idx: Option<usize>,
    pub project_idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadlineFilter {
    All,
    Overdue,
    Today,
    ThisWeek,
}

pub enum InputAction {
    AddProject,
    RenameProject,
    EditDescription,
    AddTodo,
    EditTodo(usize),
    AddSubtask,
    EditSubtask(usize),
    AddNote,
    EditNote(usize),
    AddMilestone,
    EditMilestone(usize),
    RescheduleTodo(usize),
    RescheduleMilestone(usize),
    LinkRepo,
}

pub struct InputState {
    pub title: String,
    pub value: String,
    pub action: InputAction,
}

#[allow(clippy::enum_variant_names)]
pub enum ConfirmAction {
    DeleteProject(usize),
    DeleteTodo(usize),
    DeleteSubtask(usize),
    DeleteNote(usize),
    DeleteMilestone(usize),
}

pub struct ConfirmState {
    pub prompt: String,
    pub action: ConfirmAction,
}

/// Full-screen markdown editor for a note body.
pub struct EditState {
    pub note_idx: usize,
    pub textarea: TextArea<'static>,
}

pub enum Mode {
    Normal,
    Input(InputState),
    Confirm(ConfirmState),
    EditBody(Box<EditState>),
    Help,
    GitHub,
}

pub struct App {
    pub store: Store,
    pub focus: Focus,
    pub tab: Tab,
    pub project_idx: usize,
    pub todo_idx: usize,
    pub subtask_idx: usize,
    pub note_idx: usize,
    pub note_scroll: u16,
    pub note_expanded: bool,
    pub timeline_idx: usize,
    pub deadline_filter: DeadlineFilter,
    pub mode: Mode,
    pub status: String,
    pub should_quit: bool,
    pub dirty: bool,
    pending_g: bool,
    pub gh_client: GitHubClient,
    pub gh_cache: Option<RepoInfo>,
    pub gh_loading: bool,
}

impl App {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            focus: Focus::Projects,
            tab: Tab::Todos,
            project_idx: 0,
            todo_idx: 0,
            subtask_idx: 0,
            note_idx: 0,
            note_scroll: 0,
            note_expanded: false,
            timeline_idx: 0,
            deadline_filter: DeadlineFilter::All,
            mode: Mode::Normal,
            status: String::new(),
            should_quit: false,
            dirty: false,
            pending_g: false,
            gh_client: GitHubClient::new(None),
            gh_cache: None,
            gh_loading: false,
        }
    }

    // ---- accessors -------------------------------------------------------

    pub fn current_project(&self) -> Option<&Project> {
        self.store.projects.get(self.project_idx)
    }

    pub fn current_project_mut(&mut self) -> Option<&mut Project> {
        self.store.projects.get_mut(self.project_idx)
    }

    pub fn current_todo(&self) -> Option<&Todo> {
        self.current_project()?.todos.get(self.todo_idx)
    }

    fn current_todo_mut(&mut self) -> Option<&mut Todo> {
        let i = self.todo_idx;
        self.current_project_mut()?.todos.get_mut(i)
    }

    pub fn current_note(&self) -> Option<&Note> {
        self.current_project()?.notes.get(self.note_idx)
    }

    /// Whether the right-hand detail pane has something to show right now.
    fn detail_available(&self) -> bool {
        match self.tab {
            Tab::Todos => self.current_todo().is_some(),
            Tab::Notes => self.current_note().is_some(),
            _ => false,
        }
    }

    /// Todos with a due date + milestones, sorted by date.
    pub fn timeline(&self) -> Vec<TimelineEntry> {
        let mut rows = Vec::new();
        let today = Local::now().date_naive();
        if let Some(p) = self.current_project() {
            for (ti, t) in p.todos.iter().enumerate() {
                if let Some(d) = t.due {
                    let matches = match self.deadline_filter {
                        DeadlineFilter::All => true,
                        DeadlineFilter::Overdue => !t.done && d < today,
                        DeadlineFilter::Today => d == today,
                        DeadlineFilter::ThisWeek => {
                            let diff = (d - today).num_days();
                            diff >= 0 && diff <= 7
                        }
                    };
                    if matches {
                        rows.push(TimelineEntry {
                            date: d,
                            label: t.title.clone(),
                            kind: TlKind::Todo,
                            done: t.done,
                            milestone_idx: None,
                            todo_idx: Some(ti),
                            project_idx: self.project_idx,
                        });
                    }
                }
            }
            for (mi, m) in p.milestones.iter().enumerate() {
                let matches = match self.deadline_filter {
                    DeadlineFilter::All => true,
                    DeadlineFilter::Overdue => !m.done && m.date < today,
                    DeadlineFilter::Today => m.date == today,
                    DeadlineFilter::ThisWeek => {
                        let diff = (m.date - today).num_days();
                        diff >= 0 && diff <= 7
                    }
                };
                if matches {
                    rows.push(TimelineEntry {
                        date: m.date,
                        label: m.title.clone(),
                        kind: TlKind::Milestone,
                        done: m.done,
                        milestone_idx: Some(mi),
                        todo_idx: None,
                        project_idx: self.project_idx,
                    });
                }
            }
        }
        rows.sort_by(|a, b| a.date.cmp(&b.date));
        rows
    }

    fn selected_milestone_idx(&self) -> Option<usize> {
        self.timeline()
            .get(self.timeline_idx)
            .and_then(|e| e.milestone_idx)
    }

    /// Returns (total, done, overdue, today, this_week) for the current project's deadlines.
    pub fn deadline_stats(&self) -> (usize, usize, usize, usize, usize) {
        let today = Local::now().date_naive();
        let mut total = 0usize;
        let mut done = 0usize;
        let mut overdue = 0usize;
        let mut today_count = 0usize;
        let mut this_week = 0usize;
        if let Some(p) = self.current_project() {
            for t in &p.todos {
                if let Some(d) = t.due {
                    total += 1;
                    if t.done {
                        done += 1;
                    } else if d < today {
                        overdue += 1;
                    }
                    if d == today {
                        today_count += 1;
                    }
                    let diff = (d - today).num_days();
                    if diff >= 0 && diff <= 7 {
                        this_week += 1;
                    }
                }
            }
            for m in &p.milestones {
                total += 1;
                if m.done {
                    done += 1;
                } else if m.date < today {
                    overdue += 1;
                }
                if m.date == today {
                    today_count += 1;
                }
                let diff = (m.date - today).num_days();
                if diff >= 0 && diff <= 7 {
                    this_week += 1;
                }
            }
        }
        (total, done, overdue, today_count, this_week)
    }

    /// The currently highlighted deadline entry, if any.
    pub fn current_deadline(&self) -> Option<TimelineEntry> {
        self.timeline().get(self.timeline_idx).cloned()
    }

    /// Keep every selection index inside bounds. Called once per frame.
    pub fn clamp(&mut self) {
        if self.store.projects.is_empty() {
            self.project_idx = 0;
            self.focus = Focus::Projects;
        } else if self.project_idx >= self.store.projects.len() {
            self.project_idx = self.store.projects.len() - 1;
        }
        let todos = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
        self.todo_idx = self.todo_idx.min(todos.saturating_sub(1));
        let subs = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
        self.subtask_idx = self.subtask_idx.min(subs.saturating_sub(1));
        let notes = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
        self.note_idx = self.note_idx.min(notes.saturating_sub(1));
        let body_lines = self
            .current_note()
            .map(|n| n.body.lines().count() as u16 + 8)
            .unwrap_or(0);
        self.note_scroll = self.note_scroll.min(body_lines.saturating_sub(1));
        let tl = self.timeline().len();
        self.timeline_idx = self.timeline_idx.min(tl.saturating_sub(1));

        if self.focus == Focus::Detail && !self.detail_available() {
            self.focus = Focus::Content;
        }
    }

    // ---- top level dispatch --------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Input(_) => self.handle_input(key),
            Mode::Confirm(_) => self.handle_confirm(key),
            Mode::EditBody(_) => self.handle_edit_body(key),
            Mode::Help | Mode::GitHub => self.mode = Mode::Normal,
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => self.should_quit = true,
                KeyCode::Char('g') => {
                    if self.focus == Focus::Projects {
                        if let Some(p) = self.current_project() {
                            let pre = p.repo.clone().unwrap_or_default();
                            self.begin_input("Link GitHub repo (owner/repo)", pre, InputAction::LinkRepo);
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        let g_pending = std::mem::replace(&mut self.pending_g, false);

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Tab | KeyCode::BackTab => self.toggle_focus(),
            KeyCode::Char('1') => self.focus = Focus::Projects,
            KeyCode::Char('2') => {
                if !self.store.projects.is_empty() {
                    self.focus = Focus::Content;
                }
            }
            KeyCode::Char('3') => {
                if self.detail_available() {
                    self.focus = Focus::Detail;
                }
            }
            KeyCode::Char('t') => {
                if self.focus == Focus::Content {
                    self.select_tab(Tab::Todos);
                } else {
                    self.cycle_tab(true);
                }
            }
            KeyCode::Char('T') => self.cycle_tab(false),
            KeyCode::Char('o') if self.focus == Focus::Content => self.select_tab(Tab::Overview),
            KeyCode::Char('n') if self.focus == Focus::Content => self.select_tab(Tab::Notes),
            KeyCode::Char('s') if self.focus == Focus::Content => self.select_tab(Tab::Schedule),
            KeyCode::Char('g') => {
                if g_pending {
                    self.move_sel(-1_000_000);
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => self.move_sel(1_000_000),
            _ => match self.focus {
                Focus::Projects => self.handle_projects_key(key),
                Focus::Content => match self.tab {
                    Tab::Overview => self.handle_overview_key(key),
                    Tab::Todos => self.handle_todos_key(key),
                    Tab::Notes => self.handle_notes_key(key),
                    Tab::Schedule => self.handle_timeline_key(key),
                },
                Focus::Detail => match self.tab {
                    Tab::Notes => self.handle_note_body_key(key),
                    _ => self.handle_subtasks_key(key),
                },
            },
        }
    }

    // ---- projects panel ----------------------------------------------

    fn handle_projects_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('a') => {
                self.begin_input("New project", String::new(), InputAction::AddProject)
            }
            KeyCode::Char('r') => {
                if let Some(p) = self.current_project() {
                    let name = p.name.clone();
                    self.begin_input("Rename project", name, InputAction::RenameProject);
                }
            }
            KeyCode::Char('d') => {
                if let Some(p) = self.current_project() {
                    let prompt = format!("Delete project \"{}\" and all its items?", p.name);
                    let i = self.project_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteProject(i),
                    });
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(p) = self.current_project() {
                    if let Some(ref repo) = p.repo {
                        let repo = repo.clone();
                        let parts: Vec<&str> = repo.split('/').collect();
                        if parts.len() == 2 {
                            self.fetch_github(parts[0], parts[1]);
                        }
                    } else {
                        self.status = "no repo linked — press g to link".into();
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if !self.store.projects.is_empty() {
                    self.focus = Focus::Content;
                }
            }
            _ => {}
        }
    }

    // ---- todos tab --------------------------------------------------

    fn handle_todos_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if self.current_todo().is_some() {
                    self.subtask_idx = 0;
                    self.focus = Focus::Detail;
                }
            }
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input(
                        "New todo   (!1..!3 priority · @YYYY-MM-DD due)",
                        String::new(),
                        InputAction::AddTodo,
                    );
                } else {
                    self.status = "create a project first (press 1, then a)".into();
                }
            }
            KeyCode::Char('e') => {
                if let Some(t) = self.current_project().and_then(|p| p.todos.get(self.todo_idx)) {
                    let pre = todo_edit_string(t);
                    let i = self.todo_idx;
                    self.begin_input("Edit todo", pre, InputAction::EditTodo(i));
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let i = self.todo_idx;
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.done = !t.done;
                    for s in &mut t.subtasks {
                        s.done = t.done;
                    }
                    self.dirty = true;
                }
            }
            KeyCode::Char('p') => {
                let i = self.todo_idx;
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.priority = t.priority.next();
                    self.dirty = true;
                }
            }
            KeyCode::Char('d') => {
                if let Some(t) = self.current_project().and_then(|p| p.todos.get(self.todo_idx)) {
                    let prompt = format!("Delete todo \"{}\"?", t.title);
                    let i = self.todo_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteTodo(i),
                    });
                }
            }
            KeyCode::Char('J') => self.reorder_todo(1),
            KeyCode::Char('K') => self.reorder_todo(-1),
            _ => {}
        }
    }

    fn reorder_todo(&mut self, delta: i32) {
        let i = self.todo_idx;
        let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
        if len < 2 {
            return;
        }
        let j = (i as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if i == j {
            return;
        }
        if let Some(p) = self.current_project_mut() {
            p.todos.swap(i, j);
        }
        self.todo_idx = j;
        self.dirty = true;
    }

    // ---- subtasks pane ------------------------------------------

    fn handle_subtasks_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Content,
            KeyCode::Char('a') => {
                if self.current_todo().is_some() {
                    self.begin_input("New subtask", String::new(), InputAction::AddSubtask);
                }
            }
            KeyCode::Char('e') => {
                if let Some(s) = self
                    .current_todo()
                    .and_then(|t| t.subtasks.get(self.subtask_idx))
                {
                    let pre = s.title.clone();
                    let i = self.subtask_idx;
                    self.begin_input("Edit subtask", pre, InputAction::EditSubtask(i));
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let i = self.subtask_idx;
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.done = !s.done;
                    self.dirty = true;
                }
            }
            KeyCode::Char('d') => {
                if let Some(s) = self
                    .current_todo()
                    .and_then(|t| t.subtasks.get(self.subtask_idx))
                {
                    let prompt = format!("Delete subtask \"{}\"?", s.title);
                    let i = self.subtask_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteSubtask(i),
                    });
                }
            }
            KeyCode::Char('J') => self.reorder_subtask(1),
            KeyCode::Char('K') => self.reorder_subtask(-1),
            _ => {}
        }
    }

    fn reorder_subtask(&mut self, delta: i32) {
        let i = self.subtask_idx;
        let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
        if len < 2 {
            return;
        }
        let j = (i as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if i == j {
            return;
        }
        if let Some(t) = self.current_todo_mut() {
            t.subtasks.swap(i, j);
        }
        self.subtask_idx = j;
        self.dirty = true;
    }

    // ---- overview tab -------------------------------------------

    fn handle_overview_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('e') | KeyCode::Char('a') => {
                if let Some(p) = self.current_project() {
                    let pre = p.description.clone();
                    self.begin_input("Project description", pre, InputAction::EditDescription);
                }
            }
            KeyCode::Char('r') => {
                if let Some(p) = self.current_project() {
                    let name = p.name.clone();
                    self.begin_input("Rename project", name, InputAction::RenameProject);
                }
            }
            _ => {}
        }
    }

    // ---- notes tab ---------------------------------------------

    fn handle_notes_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if self.current_note().is_some() {
                    self.note_scroll = 0;
                    self.focus = Focus::Detail;
                }
            }
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input("New note", String::new(), InputAction::AddNote);
                } else {
                    self.status = "create a project first".into();
                }
            }
            KeyCode::Char('e') => {
                if let Some(n) = self
                    .current_project()
                    .and_then(|p| p.notes.get(self.note_idx))
                {
                    let pre = n.text.clone();
                    let i = self.note_idx;
                    self.begin_input("Edit note", pre, InputAction::EditNote(i));
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let i = self.note_idx;
                if let Some(n) = self.current_project_mut().and_then(|p| p.notes.get_mut(i)) {
                    n.pinned = !n.pinned;
                    self.dirty = true;
                }
            }
            KeyCode::Char('d') => {
                if let Some(n) = self
                    .current_project()
                    .and_then(|p| p.notes.get(self.note_idx))
                {
                    let prompt = format!("Delete note \"{}\"?", truncate(&n.text, 40));
                    let i = self.note_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteNote(i),
                    });
                }
            }
            KeyCode::Char('J') => self.reorder_note(1),
            KeyCode::Char('K') => self.reorder_note(-1),
            _ => {}
        }
    }

    fn reorder_note(&mut self, delta: i32) {
        let i = self.note_idx;
        let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
        if len < 2 {
            return;
        }
        let j = (i as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if i == j {
            return;
        }
        if let Some(p) = self.current_project_mut() {
            p.notes.swap(i, j);
        }
        self.note_idx = j;
        self.dirty = true;
    }

    // ---- note body pane (rendered markdown, beside a note) ------

    fn handle_note_body_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.note_scroll = self.note_scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.note_scroll = self.note_scroll.saturating_sub(1),
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.note_scroll = self.note_scroll.saturating_add(10);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.note_scroll = self.note_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => self.note_scroll = self.note_scroll.saturating_add(10),
            KeyCode::PageUp => self.note_scroll = self.note_scroll.saturating_sub(10),
            KeyCode::Char('h') | KeyCode::Left => {
                if self.note_expanded {
                    self.note_expanded = false;
                } else {
                    self.focus = Focus::Content;
                }
            }
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if self.note_expanded {
                    self.note_expanded = false;
                } else {
                    self.note_expanded = true;
                }
            }
            KeyCode::Char('e') | KeyCode::Char('i') => self.begin_edit_body(),
            KeyCode::Esc => {
                if self.note_expanded {
                    self.note_expanded = false;
                }
            }
            _ => {}
        }
    }

    fn begin_edit_body(&mut self) {
        let Some(note) = self.current_note() else { return };
        let lines: Vec<String> = if note.body.is_empty() {
            vec![String::new()]
        } else {
            note.body.lines().map(str::to_string).collect()
        };
        let mut textarea = TextArea::new(lines);
        textarea.set_placeholder_text("Write in Markdown…  # heading, - bullet, **bold**, `code`");
        self.mode = Mode::EditBody(Box::new(EditState {
            note_idx: self.note_idx,
            textarea,
        }));
        self.status = "editing note — esc / ^s to save".into();
    }

    fn handle_edit_body(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.commit_edit_body(),
            KeyCode::Char('s') if ctrl => self.commit_edit_body(),
            _ => {
                if let Mode::EditBody(state) = &mut self.mode {
                    state.textarea.input(key);
                }
            }
        }
    }

    fn commit_edit_body(&mut self) {
        let Mode::EditBody(state) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        let body = state.textarea.lines().join("\n");
        let body = body.trim_end().to_string();
        if let Some(n) = self
            .current_project_mut()
            .and_then(|p| p.notes.get_mut(state.note_idx))
        {
            n.body = body;
            self.dirty = true;
            self.status = "note saved".into();
        }
        self.note_scroll = 0;
    }

    // ---- deadlines tab ---------------------------------------------

    fn handle_timeline_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('1') => {
                self.deadline_filter = DeadlineFilter::Overdue;
                self.timeline_idx = 0;
                self.status = "filter: overdue".into();
            }
            KeyCode::Char('2') => {
                self.deadline_filter = DeadlineFilter::Today;
                self.timeline_idx = 0;
                self.status = "filter: today".into();
            }
            KeyCode::Char('3') => {
                self.deadline_filter = DeadlineFilter::ThisWeek;
                self.timeline_idx = 0;
                self.status = "filter: this week".into();
            }
            KeyCode::Char('0') | KeyCode::Char('`') => {
                self.deadline_filter = DeadlineFilter::All;
                self.timeline_idx = 0;
                self.status = "filter: all".into();
            }
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input(
                        "New milestone   (add @YYYY-MM-DD for the date)",
                        String::new(),
                        InputAction::AddMilestone,
                    );
                } else {
                    self.status = "create a project first".into();
                }
            }
            KeyCode::Char('e') => match self.selected_milestone_idx() {
                Some(mi) => {
                    if let Some(m) = self.current_project().and_then(|p| p.milestones.get(mi)) {
                        let pre = milestone_edit_string(m);
                        self.begin_input("Edit milestone", pre, InputAction::EditMilestone(mi));
                    }
                }
                None => {
                    self.status = "pick a milestone (◆) — todos are edited on the Todos tab".into()
                }
            },
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                if let Some(entry) = self.current_deadline() {
                    if let Some(ti) = entry.todo_idx {
                        let done = self
                            .current_project_mut()
                            .and_then(|p| p.todos.get_mut(ti))
                            .map(|t| {
                                t.done = !t.done;
                                for s in &mut t.subtasks {
                                    s.done = t.done;
                                }
                                t.done
                            });
                        if let Some(done) = done {
                            self.dirty = true;
                            self.status = if done { "todo done" } else { "todo reopened" }.into();
                        }
                    } else if let Some(mi) = entry.milestone_idx {
                        if let Some(m) = self
                            .current_project_mut()
                            .and_then(|p| p.milestones.get_mut(mi))
                        {
                            m.done = !m.done;
                            self.dirty = true;
                        }
                    }
                }
            }
            KeyCode::Char('r') => {
                if let Some(entry) = self.current_deadline() {
                    if let Some(ti) = entry.todo_idx {
                        let pre = format!("@{}", entry.date.format("%Y-%m-%d"));
                        self.begin_input(
                            "Reschedule todo   (@YYYY-MM-DD)",
                            pre,
                            InputAction::RescheduleTodo(ti),
                        );
                    } else if let Some(mi) = entry.milestone_idx {
                        let pre = format!("@{}", entry.date.format("%Y-%m-%d"));
                        self.begin_input(
                            "Reschedule milestone   (@YYYY-MM-DD)",
                            pre,
                            InputAction::RescheduleMilestone(mi),
                        );
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                if let Some(entry) = self.current_deadline() {
                    if entry.kind == TlKind::Todo {
                        if let Some(ti) = entry.todo_idx {
                            self.todo_idx = ti;
                            self.tab = Tab::Todos;
                            self.focus = Focus::Content;
                            self.status = "jumped to todo".into();
                        }
                    } else if let Some(mi) = entry.milestone_idx {
                        if let Some(m) = self.current_project().and_then(|p| p.milestones.get(mi)) {
                            let pre = milestone_edit_string(m);
                            self.begin_input("Edit milestone", pre, InputAction::EditMilestone(mi));
                        }
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(entry) = self.current_deadline() {
                    if let Some(mi) = entry.milestone_idx {
                        if let Some(m) = self.current_project().and_then(|p| p.milestones.get(mi)) {
                            let prompt = format!("Delete milestone \"{}\"?", m.title);
                            self.mode = Mode::Confirm(ConfirmState {
                                prompt,
                                action: ConfirmAction::DeleteMilestone(mi),
                            });
                        }
                    } else if let Some(ti) = entry.todo_idx {
                        if let Some(t) = self.current_project().and_then(|p| p.todos.get(ti)) {
                            let prompt = format!("Delete todo \"{}\"?", t.title);
                            self.mode = Mode::Confirm(ConfirmState {
                                prompt,
                                action: ConfirmAction::DeleteTodo(ti),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ---- input / confirm modes ----------------------------------

    fn handle_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "cancelled".into();
            }
            KeyCode::Enter => {
                if let Mode::Input(input) = std::mem::replace(&mut self.mode, Mode::Normal) {
                    self.commit_input(input);
                }
            }
            KeyCode::Backspace => {
                if let Mode::Input(input) = &mut self.mode {
                    input.value.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Mode::Input(input) = &mut self.mode {
                    input.value.push(c);
                }
            }
            _ => {}
        }
    }

    fn handle_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Mode::Confirm(c) = std::mem::replace(&mut self.mode, Mode::Normal) {
                    self.perform_confirm(c.action);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.status = "cancelled".into();
            }
            _ => {}
        }
    }

    fn commit_input(&mut self, input: InputState) {
        match input.action {
            InputAction::AddProject => {
                let name = input.value.trim();
                if name.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                self.store.projects.push(Project::new(name));
                self.project_idx = self.store.projects.len() - 1;
                self.focus = Focus::Content;
                self.tab = Tab::Todos;
                self.reset_content_idx();
                self.dirty = true;
                self.status = format!("added project: {name}");
            }
            InputAction::RenameProject => {
                let name = input.value.trim().to_string();
                if name.is_empty() {
                    return;
                }
                if let Some(p) = self.current_project_mut() {
                    p.name = name.clone();
                }
                self.dirty = true;
                self.status = format!("renamed to: {name}");
            }
            InputAction::EditDescription => {
                let text = input.value.trim().to_string();
                if let Some(p) = self.current_project_mut() {
                    p.description = text;
                    self.dirty = true;
                    self.status = "description updated".into();
                }
            }
            InputAction::AddNote => {
                let text = input.value.trim().to_string();
                if text.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                let mut len = 0;
                if let Some(p) = self.current_project_mut() {
                    p.notes.push(Note::new(text, false));
                    len = p.notes.len();
                }
                self.note_idx = len.saturating_sub(1);
                self.note_scroll = 0;
                self.focus = Focus::Detail;
                self.dirty = true;
                self.begin_edit_body();
            }
            InputAction::EditNote(i) => {
                let text = input.value.trim().to_string();
                if text.is_empty() {
                    return;
                }
                if let Some(n) = self.current_project_mut().and_then(|p| p.notes.get_mut(i)) {
                    n.text = text;
                    self.dirty = true;
                    self.status = "note updated".into();
                }
            }
            InputAction::AddTodo => {
                let (title, priority, due) = parse_todo_input(&input.value);
                if title.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                if let Some(p) = self.current_project_mut() {
                    let mut todo = Todo::new(title);
                    todo.priority = priority;
                    todo.due = due;
                    p.todos.push(todo);
                }
                let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                self.todo_idx = len.saturating_sub(1);
                self.subtask_idx = 0;
                self.dirty = true;
                self.status = "todo added".into();
            }
            InputAction::EditTodo(i) => {
                let (title, priority, due) = parse_todo_input(&input.value);
                if title.is_empty() {
                    return;
                }
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.title = title;
                    t.priority = priority;
                    t.due = due;
                    self.dirty = true;
                    self.status = "todo updated".into();
                }
            }
            InputAction::AddSubtask => {
                let title = input.value.trim().to_string();
                if title.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                let mut len = 0;
                if let Some(t) = self.current_todo_mut() {
                    t.subtasks.push(Subtask::new(title, false));
                    len = t.subtasks.len();
                }
                self.subtask_idx = len.saturating_sub(1);
                self.dirty = true;
                self.status = "subtask added".into();
            }
            InputAction::EditSubtask(i) => {
                let title = input.value.trim().to_string();
                if title.is_empty() {
                    return;
                }
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.title = title;
                    self.dirty = true;
                    self.status = "subtask updated".into();
                }
            }
            InputAction::AddMilestone => {
                let today = Local::now().date_naive();
                match parse_milestone_input(&input.value, today) {
                    Some((title, date)) => {
                        if let Some(p) = self.current_project_mut() {
                            p.milestones.push(Milestone { title, date, done: false });
                        }
                        self.dirty = true;
                        self.status = "milestone added".into();
                    }
                    None => self.status = "nothing added".into(),
                }
            }
            InputAction::EditMilestone(i) => {
                let today = Local::now().date_naive();
                if let Some((title, date)) = parse_milestone_input(&input.value, today)
                    && let Some(m) = self.current_project_mut().and_then(|p| p.milestones.get_mut(i))
                {
                    m.title = title;
                    m.date = date;
                    self.dirty = true;
                    self.status = "milestone updated".into();
                }
            }
            InputAction::RescheduleTodo(i) => {
                let today = Local::now().date_naive();
                if let Some((_, date)) = parse_milestone_input(&input.value, today) {
                    if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                        t.due = Some(date);
                        self.dirty = true;
                        self.status = format!("rescheduled to {}", date.format("%Y-%m-%d")).into();
                    }
                }
            }
            InputAction::RescheduleMilestone(i) => {
                let today = Local::now().date_naive();
                if let Some((_, date)) = parse_milestone_input(&input.value, today) {
                    if let Some(m) = self.current_project_mut().and_then(|p| p.milestones.get_mut(i)) {
                        m.date = date;
                        self.dirty = true;
                        self.status = format!("rescheduled to {}", date.format("%Y-%m-%d")).into();
                    }
                }
            }
            InputAction::LinkRepo => {
                let value = input.value.trim().to_string();
                if value.is_empty() {
                    if let Some(p) = self.current_project_mut() {
                        p.repo = None;
                        self.dirty = true;
                        self.gh_cache = None;
                        self.status = "repo unlinked".into();
                    }
                } else if let Some((owner, repo)) = crate::github::parse_repo_string(&value) {
                    if let Some(p) = self.current_project_mut() {
                        p.repo = Some(format!("{owner}/{repo}"));
                        self.dirty = true;
                        self.status = format!("linked to {owner}/{repo}").into();
                        self.fetch_github(&owner, &repo);
                    }
                } else {
                    self.status = "invalid repo format — use owner/repo".into();
                }
            }
        }
    }

    fn perform_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::DeleteProject(i) => {
                if i < self.store.projects.len() {
                    let name = self.store.projects.remove(i).name;
                    self.project_idx = self
                        .project_idx
                        .min(self.store.projects.len().saturating_sub(1));
                    self.reset_content_idx();
                    self.focus = Focus::Projects;
                    self.dirty = true;
                    self.status = format!("deleted project: {name}");
                }
            }
            ConfirmAction::DeleteTodo(i) => {
                if let Some(p) = self.current_project_mut()
                    && i < p.todos.len()
                {
                    p.todos.remove(i);
                }
                let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                self.todo_idx = self.todo_idx.min(len.saturating_sub(1));
                self.subtask_idx = 0;
                self.dirty = true;
                self.status = "todo deleted".into();
            }
            ConfirmAction::DeleteSubtask(i) => {
                if let Some(t) = self.current_todo_mut()
                    && i < t.subtasks.len()
                {
                    t.subtasks.remove(i);
                }
                let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
                self.subtask_idx = self.subtask_idx.min(len.saturating_sub(1));
                self.dirty = true;
                self.status = "subtask deleted".into();
            }
            ConfirmAction::DeleteNote(i) => {
                if let Some(p) = self.current_project_mut()
                    && i < p.notes.len()
                {
                    p.notes.remove(i);
                }
                let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
                self.note_idx = self.note_idx.min(len.saturating_sub(1));
                self.dirty = true;
                self.status = "note deleted".into();
            }
            ConfirmAction::DeleteMilestone(i) => {
                if let Some(p) = self.current_project_mut()
                    && i < p.milestones.len()
                {
                    p.milestones.remove(i);
                }
                self.timeline_idx = self.timeline_idx.saturating_sub(1);
                self.dirty = true;
                self.status = "milestone deleted".into();
            }
        }
    }

    // ---- helpers --------------------------------------------------

    fn begin_input(&mut self, title: &str, value: String, action: InputAction) {
        self.mode = Mode::Input(InputState {
            title: title.to_string(),
            value,
            action,
        });
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Projects if self.store.projects.is_empty() => Focus::Projects,
            Focus::Projects => Focus::Content,
            Focus::Content if self.detail_available() => Focus::Detail,
            Focus::Content => Focus::Projects,
            Focus::Detail => Focus::Projects,
        };
    }

    fn reset_content_idx(&mut self) {
        self.todo_idx = 0;
        self.subtask_idx = 0;
        self.note_idx = 0;
        self.note_scroll = 0;
        self.timeline_idx = 0;
    }

    fn cycle_tab(&mut self, forward: bool) {
        let cur = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let next = if forward {
            (cur + 1) % Tab::ALL.len()
        } else {
            (cur + Tab::ALL.len() - 1) % Tab::ALL.len()
        };
        self.tab = Tab::ALL[next];
        if self.focus == Focus::Detail && !self.detail_available() {
            self.focus = Focus::Content;
        }
    }

    fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if self.focus == Focus::Detail && !self.detail_available() {
            self.focus = Focus::Content;
        }
    }

    fn fetch_github(&mut self, owner: &str, repo: &str) {
        self.gh_loading = true;
        self.status = format!("fetching {owner}/{repo}...").into();
        match self.gh_client.fetch_repo_info(owner, repo) {
            Ok(info) => {
                self.gh_cache = Some(info);
                self.gh_loading = false;
                self.mode = Mode::GitHub;
                self.status = "github data loaded".into();
            }
            Err(e) => {
                self.gh_loading = false;
                self.status = format!("github error: {e}").into();
            }
        }
    }

    fn move_sel(&mut self, delta: i32) {
        match self.focus {
            Focus::Projects => {
                let len = self.store.projects.len();
                self.project_idx = step(self.project_idx, delta, len);
                self.reset_content_idx();
            }
            Focus::Content => match self.tab {
                Tab::Overview => {}
                Tab::Todos => {
                    let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                    self.todo_idx = step(self.todo_idx, delta, len);
                    self.subtask_idx = 0;
                }
                Tab::Notes => {
                    let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
                    self.note_idx = step(self.note_idx, delta, len);
                    self.note_scroll = 0;
                }
                Tab::Schedule => {
                    let len = self.timeline().len();
                    self.timeline_idx = step(self.timeline_idx, delta, len);
                }
            },
            Focus::Detail => match self.tab {
                Tab::Notes => {
                    self.note_scroll = if delta < 0 {
                        self.note_scroll.saturating_sub((-delta).min(i32::from(u16::MAX)) as u16)
                    } else {
                        self.note_scroll.saturating_add(delta.min(i32::from(u16::MAX)) as u16)
                    };
                }
                _ => {
                    let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
                    self.subtask_idx = step(self.subtask_idx, delta, len);
                }
            },
        }
    }
}

fn step(idx: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len as i32 - 1;
    (idx as i32 + delta).clamp(0, max) as usize
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// Parse `buy milk !3 @2026-09-01` into (title, priority, due).
fn parse_todo_input(raw: &str) -> (String, Priority, Option<NaiveDate>) {
    let mut priority = Priority::Medium;
    let mut due = None;
    let mut words = Vec::new();

    for tok in raw.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('@')
            && let Ok(d) = NaiveDate::parse_from_str(rest, "%Y-%m-%d")
        {
            due = Some(d);
            continue;
        }
        match tok {
            "!1" | "!low" => {
                priority = Priority::Low;
                continue;
            }
            "!2" | "!med" | "!medium" => {
                priority = Priority::Medium;
                continue;
            }
            "!3" | "!!" | "!high" => {
                priority = Priority::High;
                continue;
            }
            _ => {}
        }
        words.push(tok);
    }

    (words.join(" ").trim().to_string(), priority, due)
}

/// Parse `Launch v1 @2026-09-15` into (title, date), defaulting to `fallback`.
fn parse_milestone_input(raw: &str, fallback: NaiveDate) -> Option<(String, NaiveDate)> {
    let mut date = fallback;
    let mut words = Vec::new();

    for tok in raw.split_whitespace() {
        if tok == "|" {
            continue;
        }
        if let Some(rest) = tok.strip_prefix('@')
            && let Ok(d) = NaiveDate::parse_from_str(rest, "%Y-%m-%d")
        {
            date = d;
            continue;
        }
        words.push(tok);
    }

    let title = words.join(" ").trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some((title, date))
    }
}

fn todo_edit_string(t: &Todo) -> String {
    let mut s = t.title.clone();
    match t.priority {
        Priority::Low => s.push_str(" !1"),
        Priority::High => s.push_str(" !3"),
        Priority::Medium => {}
    }
    if let Some(d) = t.due {
        s.push_str(&format!(" @{}", d.format("%Y-%m-%d")));
    }
    s
}

fn milestone_edit_string(m: &Milestone) -> String {
    format!("{} @{}", m.title, m.date.format("%Y-%m-%d"))
}
