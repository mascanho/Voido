//! Application state and Vim-style key handling.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use chrono::{DateTime, Local, NaiveDate};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use tui_textarea::{CursorMove, TextArea};

use crate::config::{Config, StorageChoice};
use crate::github::{GitHubClient, RepoInfo, RepoRef, SyncClient};
use crate::model::{Attachment, Meeting, Milestone, Note, Priority, Project, Store, Subtask, Todo};
use crate::util::truncate;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Projects,
    /// The middle pane: the todo list, notes, timeline or overview, per `Tab`.
    Content,
    /// The right pane: subtasks of the selected todo (Todos tab) or subnotes of
    /// the selected note (Notes tab).
    Detail,
}

/// Screen rects of the three body panes, refreshed every render so mouse clicks
/// can be routed to the right pane. A zero-sized rect means "not shown".
#[derive(Debug, Clone, Copy, Default)]
pub struct PaneRects {
    pub projects: Rect,
    pub content: Rect,
    pub detail: Rect,
}

/// One line in the `^l` activity panel.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub at: DateTime<Local>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

/// A transient corner notification ("sonner") that fades on its own after a few
/// seconds. Used for sync results and other one-off events that don't warrant a
/// modal or a permanent footer badge.
#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    born: Instant,
}

impl Toast {
    /// How long a toast stays on screen.
    const TTL: std::time::Duration = std::time::Duration::from_secs(4);

    fn expired(&self) -> bool {
        self.born.elapsed() >= Self::TTL
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Todos,
    Notes,
    Schedule,
    Meetings,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Overview,
        Tab::Todos,
        Tab::Notes,
        Tab::Schedule,
        Tab::Meetings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Todos => "Todos",
            Tab::Notes => "Notes",
            Tab::Schedule => "Schedule",
            Tab::Meetings => "Meetings",
        }
    }

    /// Clipped title for the tab strip in a narrow pane.
    fn short(self) -> &'static str {
        match self {
            Tab::Overview => "Ovw",
            Tab::Todos => "Todo",
            Tab::Notes => "Note",
            Tab::Schedule => "Sched",
            Tab::Meetings => "Meet",
        }
    }

    /// The label the strip shows for this tab on a pane `width` wide. The full
    /// titles don't fit the middle pane of the three-pane views, and a strip
    /// that runs past the border would hide the last tab entirely.
    pub fn strip_label(self, width: u16) -> &'static str {
        let full: usize = Tab::ALL.iter().map(|t| t.title().chars().count() + 2).sum();
        if full > (width as usize).saturating_sub(2) {
            self.short()
        } else {
            self.title()
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

impl DeadlineFilter {
    /// Cycle All -> Overdue -> Today -> This week -> All.
    fn next(self) -> Self {
        match self {
            DeadlineFilter::All => DeadlineFilter::Overdue,
            DeadlineFilter::Overdue => DeadlineFilter::Today,
            DeadlineFilter::Today => DeadlineFilter::ThisWeek,
            DeadlineFilter::ThisWeek => DeadlineFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DeadlineFilter::All => "all",
            DeadlineFilter::Overdue => "overdue",
            DeadlineFilter::Today => "today",
            DeadlineFilter::ThisWeek => "this week",
        }
    }
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
    /// Add an attachment to this target; returns to the manager afterwards.
    AddAttachment(AttachTarget),
    /// Add one or more (space-separated) tags to this target.
    AddTag(TagTarget),
    AddMilestone,
    EditMilestone(usize),
    AddMeeting,
    EditMeeting(usize),
    RescheduleMeeting(usize),
    RescheduleTodo(usize),
    RescheduleMilestone(usize),
    /// Write the whole store to a JSON file at this path.
    ExportData,
    /// Load a JSON file at this path; on success, confirm before replacing.
    ImportData,
    LinkRepo,
    /// GitHub data-sync setup: repo, then token.
    SyncRepo,
    SyncToken,
    /// Fetch a settings file out of a GitHub repo and adopt it.
    ImportSettings,
}

pub struct InputState {
    pub title: String,
    pub editor: TextArea<'static>,
    pub action: InputAction,
}

impl InputState {
    /// The current single-line contents of the input.
    pub fn value(&self) -> String {
        self.editor.lines().first().cloned().unwrap_or_default()
    }
}

pub enum ConfirmAction {
    DeleteProject(usize),
    DeleteTodo(usize),
    DeleteSubtask(usize),
    DeleteNote(usize),
    DeleteMilestone(usize),
    DeleteMeeting(usize),
    /// `q` — guard against an accidental quit.
    Quit,
    /// Replace the entire store with a dataset loaded from a file.
    ImportData(Box<Store>),
    /// Replace the settings with the ones fetched from a GitHub repo.
    ImportSettings(Box<Config>),
}

pub struct ConfirmState {
    pub prompt: String,
    pub action: ConfirmAction,
}

/// What a Markdown editor session is writing to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    /// The body of the project note at this index.
    NoteBody(usize),
    /// The note attached to the todo at this index.
    TodoNote(usize),
    /// The note attached to the current todo's subtask at this index.
    SubtaskNote(usize),
    /// The agenda / minutes of the meeting at this index.
    MeetingNote(usize),
}

/// The note `^f` blows up to fill the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullNote {
    /// The selected todo's own note.
    Todo,
    /// The note of the current todo's subtask at this index.
    Subtask(usize),
    /// The Markdown body of the selected note in the Notes tab.
    Note,
    /// The agenda / minutes of the selected meeting.
    Meeting,
}

/// Which note the Todos-tab detail pane is rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailNote {
    /// The selected todo's own note (fills the detail pane).
    Todo,
    /// The note of the todo's subtask at this index (section below the list).
    Subtask(usize),
}

/// Markdown editor for a note body or a todo's attached note.
pub struct EditState {
    pub target: EditTarget,
    pub textarea: TextArea<'static>,
}

/// What a set of attachments hangs off: a todo, or one of its subtasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachTarget {
    pub todo_idx: usize,
    /// `Some(i)` when the manager is for subtask `i` of that todo.
    pub sub_idx: Option<usize>,
}

/// The attachment manager overlay.
pub struct AttachState {
    pub target: AttachTarget,
    pub sel: usize,
}

/// What a set of tags hangs off. Indices are captured when the manager opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagTarget {
    Project,
    Todo(usize),
    Subtask { todo: usize, sub: usize },
}

/// The tag manager overlay.
pub struct TagState {
    pub target: TagTarget,
    pub sel: usize,
}

/// The "links in this note" overlay (`L`). Items are `(label, url)`, captured
/// from the markdown that was on screen when it opened.
pub struct LinksState {
    pub items: Vec<(String, String)>,
    pub sel: usize,
}

/// The fuzzy-finder overlay.
pub struct SearchState {
    pub editor: TextArea<'static>,
    pub sel: usize,
}

impl SearchState {
    pub fn query(&self) -> String {
        self.editor.lines().first().cloned().unwrap_or_default()
    }
}

/// Where a fuzzy-search hit points.
#[derive(Debug, Clone, Copy)]
pub enum SearchTarget {
    Project,
    Todo(usize),
    Subtask { todo: usize, sub: usize },
    Note(usize),
}

impl SearchTarget {
    /// Nesting depth: project 0, todo/note 1, subtask 2.
    pub fn depth(self) -> usize {
        match self {
            SearchTarget::Project => 0,
            SearchTarget::Todo(_) | SearchTarget::Note(_) => 1,
            SearchTarget::Subtask { .. } => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub project_idx: usize,
    pub target: SearchTarget,
    pub label: String,
    /// Ancestor names, outermost first — `[]` for a project, `[project]` for a
    /// todo/note, `[project, todo]` for a subtask.
    pub crumbs: Vec<String>,
    /// A dim summary of what the hit contains — `Some("8 todos · 2 overdue")` on
    /// a project, `Some("↳ 2/5")` on a todo that has subtasks, `None` otherwise.
    pub context: Option<String>,
}

/// One-line summary of a project's workload, shown on its fuzzy-search hit.
fn project_search_context(p: &Project) -> String {
    let total = p.todos.len();
    if total == 0 {
        return "no todos".to_string();
    }
    let today = Local::now().date_naive();
    let overdue = p
        .todos
        .iter()
        .filter(|t| !t.done && t.due.is_some_and(|d| d < today))
        .count();
    let mut s = format!("{total} todo{}", if total == 1 { "" } else { "s" });
    if overdue > 0 {
        s.push_str(&format!(" · {overdue} overdue"));
    }
    s
}

/// Theme picker. Moving the cursor previews the theme live; `esc` restores the
/// one that was active when the picker opened.
pub struct ThemeState {
    /// Index into `theme::registry()` — highlighted and previewed.
    pub idx: usize,
    /// Index to fall back to on cancel.
    pub saved: usize,
}

pub enum Mode {
    Normal,
    Input(Box<InputState>),
    Confirm(ConfirmState),
    EditBody(Box<EditState>),
    Help,
    GitHub,
    /// Full current-conditions + 3-day forecast popup (`^w`).
    Weather,
    Theme(ThemeState),
    /// A dismissible message popup — (title, body). Used for sync results.
    Notice(String, String),
    /// Attachment manager for the current todo.
    Attach(AttachState),
    /// Tag manager for the current project / todo / subtask.
    Tags(TagState),
    /// Links found in the note currently on screen (`L`).
    Links(LinksState),
    /// The main menu (`^k`) — a hub for the global actions.
    Menu(MenuState),
    /// The `o` ordering menu for the list in focus.
    Sort(SortState),
    /// Global fuzzy finder.
    Search(Box<SearchState>),
}

/// One entry in the `^k` main menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    SaveNow,
    Sync,
    Export,
    Import,
    Settings,
    ImportSettings,
    Theme,
    Weather,
    Activity,
    Help,
    Quit,
}

impl MenuAction {
    /// The menu, top to bottom: `(action, glyph, label, key hint)`.
    pub const ENTRIES: &'static [(MenuAction, &'static str, &'static str, &'static str)] = &[
        (MenuAction::SaveNow, "\u{f0c7}", "Save now", ""),
        (MenuAction::Sync, "\u{f09b}", "Sync to GitHub", "^s"),
        (MenuAction::Export, "\u{f0ee}", "Export data…", ""),
        (MenuAction::Import, "\u{f019}", "Import data…", ""),
        (MenuAction::Settings, "\u{f013}", "Settings", "^e"),
        (
            MenuAction::ImportSettings,
            "\u{f0ed}",
            "Settings from GitHub…",
            "",
        ),
        (MenuAction::Theme, "\u{f043}", "Theme", "^t"),
        (MenuAction::Weather, "\u{f0c2}", "Weather", "^w"),
        (MenuAction::Activity, "\u{f085}", "Activity log", "^l"),
        (MenuAction::Help, "\u{f059}", "Keybindings", "?"),
        (MenuAction::Quit, "\u{f011}", "Quit", "q"),
    ];
}

pub struct MenuState {
    pub sel: usize,
}

/// Which list an ordering applies to. Each scope offers the orderings that make
/// sense for what it holds, and remembers the last one applied to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortScope {
    Projects,
    Todos,
    Subtasks,
    Notes,
    Meetings,
}

impl SortScope {
    /// Index into `App::sorts`.
    fn idx(self) -> usize {
        match self {
            SortScope::Projects => 0,
            SortScope::Todos => 1,
            SortScope::Subtasks => 2,
            SortScope::Notes => 3,
            SortScope::Meetings => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortScope::Projects => "projects",
            SortScope::Todos => "todos",
            SortScope::Subtasks => "subtasks",
            SortScope::Notes => "notes",
            SortScope::Meetings => "meetings",
        }
    }

    /// The orderings this list offers, in menu order. The first is the default
    /// the menu opens on, so it's the one that fits the list best.
    pub fn keys(self) -> &'static [SortKey] {
        match self {
            SortScope::Projects => &[
                SortKey::Name,
                SortKey::Deadline,
                SortKey::Open,
                SortKey::Progress,
                SortKey::Created,
                SortKey::Tag,
            ],
            SortScope::Todos => &[
                SortKey::Priority,
                SortKey::Due,
                SortKey::Name,
                SortKey::Status,
                SortKey::Progress,
                SortKey::Tag,
            ],
            SortScope::Subtasks => &[
                SortKey::Priority,
                SortKey::Name,
                SortKey::Status,
                SortKey::Tag,
            ],
            SortScope::Notes => &[SortKey::Pinned, SortKey::Name, SortKey::Length],
            SortScope::Meetings => &[
                SortKey::Date,
                SortKey::Name,
                SortKey::Held,
                SortKey::Attendees,
            ],
        }
    }
}

/// One ordering offered by the `o` menu. Not every key applies to every scope —
/// `SortScope::keys` decides which are on offer where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    /// High → low.
    Priority,
    /// A todo's own due date.
    Due,
    /// When a meeting is (or was).
    Date,
    /// Title, case-folded.
    Name,
    /// Open before done.
    Status,
    /// How far along an item is (a todo by its subtasks, a project by its todos).
    Progress,
    /// When a project was created.
    Created,
    /// A project's soonest open milestone or todo due date.
    Deadline,
    /// How many todos a project still has open.
    Open,
    /// Pinned notes first.
    Pinned,
    /// How much body a note has.
    Length,
    /// Grouped by first tag.
    Tag,
    /// Meetings still to come before ones already held.
    Held,
    /// How many people are in a meeting.
    Attendees,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Priority => "Priority",
            SortKey::Due => "Due date",
            SortKey::Date => "Date",
            SortKey::Name => "Name",
            SortKey::Status => "Status",
            SortKey::Progress => "Progress",
            SortKey::Created => "Created",
            SortKey::Deadline => "Next deadline",
            SortKey::Open => "Open todos",
            SortKey::Pinned => "Pinned",
            SortKey::Length => "Note length",
            SortKey::Tag => "Tag",
            SortKey::Held => "Held",
            SortKey::Attendees => "Attendees",
        }
    }

    /// Nerd Font glyph for the menu row.
    pub fn glyph(self) -> &'static str {
        match self {
            SortKey::Priority => "\u{f024}",
            SortKey::Due => "\u{f073}",
            SortKey::Date => "\u{f133}",
            SortKey::Name => "\u{f15d}",
            SortKey::Status => "\u{f046}",
            SortKey::Progress => "\u{f200}",
            SortKey::Created => "\u{f017}",
            SortKey::Deadline => "\u{f252}",
            SortKey::Open => "\u{f03a}",
            SortKey::Pinned => "\u{f005}",
            SortKey::Length => "\u{f036}",
            SortKey::Tag => "\u{f02b}",
            SortKey::Held => "\u{f274}",
            SortKey::Attendees => "\u{f0c0}",
        }
    }

    /// Which way round the ordering runs, spelled out for the menu row and the
    /// status line — `r` in the menu flips it.
    pub fn hint(self, reverse: bool) -> &'static str {
        match (self, reverse) {
            (SortKey::Priority, false) => "high → low",
            (SortKey::Priority, true) => "low → high",
            (SortKey::Due | SortKey::Deadline | SortKey::Date, false) => "soonest first",
            (SortKey::Due | SortKey::Deadline | SortKey::Date, true) => "latest first",
            (SortKey::Name | SortKey::Tag, false) => "A → Z",
            (SortKey::Name | SortKey::Tag, true) => "Z → A",
            (SortKey::Status, false) => "open first",
            (SortKey::Status, true) => "done first",
            (SortKey::Progress, false) => "most done first",
            (SortKey::Progress, true) => "least done first",
            (SortKey::Created, false) => "newest first",
            (SortKey::Created, true) => "oldest first",
            (SortKey::Open, false) => "most open first",
            (SortKey::Open, true) => "fewest open first",
            (SortKey::Pinned, false) => "pinned first",
            (SortKey::Pinned, true) => "pinned last",
            (SortKey::Length, false) => "longest first",
            (SortKey::Length, true) => "shortest first",
            (SortKey::Held, false) => "upcoming first",
            (SortKey::Held, true) => "held first",
            (SortKey::Attendees, false) => "most people first",
            (SortKey::Attendees, true) => "fewest people first",
        }
    }
}

/// An ordering that was applied to a list, kept so the menu can reopen on it and
/// mark it as the one in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortOrder {
    pub key: SortKey,
    pub reverse: bool,
}

/// The `o` ordering menu.
pub struct SortState {
    pub scope: SortScope,
    pub sel: usize,
    /// Direction the highlighted ordering will be applied in; `r` flips it.
    pub reverse: bool,
    /// The ordering already in force for this scope, marked in the list.
    pub active: Option<SortOrder>,
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
    pub meeting_idx: usize,
    /// Scroll offset of the meeting agenda / minutes pane.
    pub meeting_note_scroll: u16,
    pub note_expanded: bool,
    /// `^f`: the note on screen fills the whole window, panes and all.
    pub note_full: bool,
    /// `n` toggles the selected todo's note into the detail pane; `n` inside the
    /// Subtasks pane toggles the selected subtask's note into a section below the
    /// list. Independent, each with its own `^d` / `^u` scroll offset.
    pub todo_note_open: bool,
    pub sub_note_open: bool,
    /// While a subtask note is open, `l` steps focus into that pane so `j`/`k`
    /// scroll it; `h` / `esc` steps back out to the subtask list.
    pub sub_note_focus: bool,
    pub todo_note_scroll: u16,
    pub sub_note_scroll: u16,
    /// `i` toggles an inline detail panel (tags, dates, counts…) under the
    /// selected row — a separate flag per pane so expanding todos doesn't also
    /// expand the projects rail.
    pub project_info: bool,
    pub todo_info: bool,
    pub subtask_info: bool,
    pub meeting_info: bool,
    /// `m` toggles a stripped-back layout (no hint bar, minimal header).
    pub minimal: bool,
    /// `^l` toggles a bottom panel with two tables: app events and the log of
    /// data changes made this session.
    pub activity_open: bool,
    /// Session event log (status-line messages that weren't data changes).
    pub logs: Vec<LogEntry>,
    /// Session change log (status-line messages that accompanied a save).
    pub changes: Vec<LogEntry>,
    /// Last status text folded into `logs`/`changes`, so each message lands once.
    last_activity_status: String,
    /// Transient corner notification, cleared by `tick_toast` once it's old.
    pub toast: Option<Toast>,
    pub timeline_idx: usize,
    pub deadline_filter: DeadlineFilter,
    /// The ordering last applied to each list, indexed by `SortScope`. Kept so
    /// the `o` menu reopens on the order in force and can mark it.
    pub sorts: [Option<SortOrder>; 5],
    pub mode: Mode,
    pub status: String,
    pub should_quit: bool,
    pub dirty: bool,
    /// Set by `^e`; `main`'s loop drops out of the TUI to run `$EDITOR` on the
    /// settings file, then reloads it.
    pub open_settings: bool,
    pending_g: bool,
    pub gh_client: GitHubClient,
    pub gh_cache: Option<RepoInfo>,
    pub gh_loading: bool,
    /// Receiver for an in-flight background GitHub fetch, if any.
    gh_rx: Option<Receiver<Result<RepoInfo, String>>>,
    /// Persisted settings. `github_repo` is written back (as `owner/repo`) after
    /// a successful setup; `main` also persists on exit.
    pub config: Config,
    /// Token resolved at startup (config value, `gh` CLI, or environment).
    pub sync_token: Option<String>,
    /// Blob SHA of the last successfully pulled/pushed sync file.
    pub sync_sha: Option<String>,
    pub sync_in_flight: bool,
    /// Data edits made since the last successful sync this session — drives the
    /// footer's "N unsynced" hint. Zeroed on a successful push.
    pub sync_pending: usize,
    /// When the last successful sync completed this session.
    pub last_sync: Option<DateTime<Local>>,
    /// Repo name captured from the first `^s` prompt, pending a token step.
    pending_sync_repo: Option<String>,
    sync_rx: Option<Receiver<Result<SyncOk, String>>>,
    /// Receiver for an in-flight settings import (menu → "Settings from GitHub").
    settings_rx: Option<Receiver<Result<ImportedSettings, String>>>,
    settings_in_flight: bool,
    /// Receiver for an in-flight data import from a repo — "Import data" given a
    /// GitHub link instead of a file path.
    data_rx: Option<Receiver<Result<ImportedData, String>>>,
    data_in_flight: bool,
    /// Current-conditions snapshot for the header / Overview, when `weather` is
    /// configured. `None` until the first fetch lands.
    pub weather: Option<crate::weather::Weather>,
    weather_rx: Option<Receiver<Result<crate::weather::Weather, String>>>,
    weather_in_flight: bool,
    /// When the last weather fetch was *started* (drives the refresh interval).
    weather_last_try: Option<Instant>,
    /// Memoized Markdown render of the note body currently on screen,
    /// keyed by (content hash, pane width).
    md_cache: RefCell<Option<(u64, u16, Vec<Line<'static>>)>>,
    /// Body-pane rects from the last render, for routing mouse clicks.
    pub pane_rects: RefCell<PaneRects>,
}

/// The outcome of a successful sync push: the resolved `owner/repo` and the new
/// blob SHA.
pub struct SyncOk {
    repo: String,
    sha: String,
}

/// A settings file fetched from a repo, waiting for the user to confirm it.
pub struct ImportedSettings {
    config: Config,
    /// `owner/repo/path@ref` — echoed in the confirmation and the log.
    source: String,
}

/// A dataset fetched from a repo, waiting for the user to confirm it.
pub struct ImportedData {
    store: Store,
    /// `owner/repo/path@ref` — echoed in the confirmation and the log.
    source: String,
}

impl App {
    pub fn new(
        mut store: Store,
        config: Config,
        sync_sha: Option<String>,
        sync_token: Option<String>,
    ) -> Self {
        // Tidy whatever we loaded (local DB, legacy import or GitHub sync):
        // enforce "todo with subtasks is done iff they all are", and normalise
        // any hand-edited tags.
        heal_store(&mut store);
        let gh_client = GitHubClient::new(sync_token.clone());
        Self {
            store,
            focus: Focus::Projects,
            tab: Tab::Todos,
            project_idx: 0,
            todo_idx: 0,
            subtask_idx: 0,
            note_idx: 0,
            note_scroll: 0,
            meeting_idx: 0,
            meeting_note_scroll: 0,
            todo_note_open: false,
            sub_note_open: false,
            sub_note_focus: false,
            todo_note_scroll: 0,
            sub_note_scroll: 0,
            note_expanded: false,
            note_full: false,
            project_info: false,
            todo_info: false,
            subtask_info: false,
            meeting_info: false,
            minimal: false,
            activity_open: false,
            logs: Vec::new(),
            changes: Vec::new(),
            last_activity_status: String::new(),
            toast: None,
            timeline_idx: 0,
            deadline_filter: DeadlineFilter::All,
            sorts: [None; 5],
            mode: Mode::Normal,
            status: String::new(),
            should_quit: false,
            dirty: false,
            open_settings: false,
            pending_g: false,
            gh_client,
            gh_cache: None,
            gh_loading: false,
            gh_rx: None,
            config,
            sync_token,
            sync_sha,
            sync_in_flight: false,
            sync_pending: 0,
            last_sync: None,
            pending_sync_repo: None,
            sync_rx: None,
            settings_rx: None,
            settings_in_flight: false,
            data_rx: None,
            data_in_flight: false,
            weather: None,
            weather_rx: None,
            weather_in_flight: false,
            weather_last_try: None,
            md_cache: RefCell::new(None),
            pane_rects: RefCell::new(PaneRects::default()),
        }
    }

    /// GitHub sync is on, has a repo, and we have a token to use.
    pub fn sync_ready(&self) -> bool {
        self.config.sync_configured() && self.sync_token.is_some()
    }

    /// Adopt a settings file re-read from disk (after the user edited it via
    /// `^e`). Re-resolves the token and rebuilds the GitHub client; a changed
    /// repo/file means the next push re-establishes the blob SHA.
    pub fn reload_config(&mut self, config: Config) {
        let repo_changed = config.github_repo != self.config.github_repo
            || config.github_file != self.config.github_file;
        let weather_changed = config.weather != self.config.weather
            || config.weather_unit != self.config.weather_unit;
        self.config = config;
        self.sync_token = crate::github::resolve_token(self.config.github_token.as_deref());
        self.gh_client = GitHubClient::new(self.sync_token.clone());
        if repo_changed {
            self.sync_sha = None;
        }
        // Rebuild the theme list so themes added to the file show up in `^t`
        // right away, then re-apply the selected slug over the new registry.
        for warning in crate::theme::install_custom(&self.config.themes) {
            self.push_log(warning);
        }
        match self.config.theme.as_deref() {
            Some(slug) => crate::theme::set_slug(Some(slug)),
            // The key was removed (or the imported file never had one) — that
            // means "the default", not "keep whatever is on screen".
            None => crate::theme::set_index(0),
        }
        self.clear_render_cache();
        if weather_changed {
            self.weather = None;
            self.weather_last_try = None; // refetch against the new location
        }
    }

    /// Adopt an imported settings file: keep the local token, write the result
    /// to `config.toml`, and apply it to the running app.
    fn adopt_settings(&mut self, incoming: Config) {
        let mut config = self.config.clone();
        config.apply_import(incoming);
        if let Err(e) = config.save() {
            self.push_log(format!("settings import failed: {e}"));
            self.toast(ToastKind::Error, format!("Could not save settings: {e}"));
            return;
        }
        let repo_before = self.config.github_repo.clone();
        self.reload_config(config);
        self.push_log("imported settings from GitHub");
        self.toast(ToastKind::Success, "Settings imported");
        // A new sync repo only takes effect for *data* on the next start, which
        // is when voido pulls it — say so rather than let the exit push surprise
        // anyone.
        self.status = match &self.config.github_repo {
            Some(repo) if Some(repo) != repo_before.as_ref() && self.config.sync_configured() => {
                format!("settings imported — restart to pull data from {repo}")
            }
            _ => "settings imported".into(),
        };
    }

    /// Record that the local store diverged from what's on GitHub. Called once
    /// per edit cycle (after the local DB save), so the footer can show how many
    /// edits are waiting for the next `^s` / exit push.
    pub fn note_unsynced_edit(&mut self) {
        if self.sync_ready() {
            self.sync_pending = self.sync_pending.saturating_add(1);
        }
    }

    /// Fold this tick's status message into the `^l` activity panel. `saved` is
    /// set when a data change was just persisted, which routes the line to the
    /// **Changes** table instead of **Logs**. Call once per event-loop pass.
    pub fn record_activity(&mut self, saved: bool) {
        if saved {
            // Exactly one persisted edit this tick — always its own row, even if
            // the status text repeats a previous action.
            let text = if self.status.is_empty() {
                "edited".to_string()
            } else {
                self.status.clone()
            };
            push_capped(&mut self.changes, text);
            // The Changes table is the record now; keep it out of the footer
            // unless it's an error the user must see.
            if !self.status.starts_with("save error") {
                self.status.clear();
            }
        } else if !self.status.is_empty() && self.status != self.last_activity_status {
            push_capped(&mut self.logs, self.status.clone());
        }
        self.last_activity_status = self.status.clone();
    }

    /// Append a line to the **Logs** table directly (startup / shutdown events
    /// that never touch the status line).
    pub fn push_log(&mut self, text: impl Into<String>) {
        push_capped(&mut self.logs, text.into());
    }

    /// Raise a transient corner notification.
    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toast = Some(Toast {
            kind,
            text: text.into(),
            born: Instant::now(),
        });
    }

    /// Drop the toast once it's outlived its TTL. Returns `true` if it changed.
    pub fn tick_toast(&mut self) -> bool {
        if self.toast.as_ref().is_some_and(Toast::expired) {
            self.toast = None;
            true
        } else {
            false
        }
    }

    /// Markdown-render a note body, reusing the last result when the content and
    /// pane width are unchanged (avoids re-parsing on every scroll keypress).
    pub fn note_body_lines(&self, body: &str, width: u16) -> Vec<Line<'static>> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        body.hash(&mut hasher);
        let key = hasher.finish();

        let mut cache = self.md_cache.borrow_mut();
        if let Some((k, w, lines)) = cache.as_ref()
            && *k == key
            && *w == width
        {
            return lines.clone();
        }
        let lines = crate::md::render(body, width);
        *cache = Some((key, width, lines.clone()));
        lines
    }

    /// Poll in-flight background work (GitHub activity fetches and data syncs).
    /// Returns `true` when app state changed and the UI should redraw.
    pub fn poll_background(&mut self) -> bool {
        let mut changed = false;

        if let Some(rx) = &self.gh_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.gh_rx = None;
                    self.gh_loading = false;
                    match result {
                        Ok(info) => {
                            self.gh_cache = Some(info);
                            if matches!(self.mode, Mode::Normal) {
                                self.mode = Mode::GitHub;
                            }
                            self.status = "github data loaded".into();
                        }
                        Err(e) => self.status = format!("github error: {e}"),
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.gh_rx = None;
                    self.gh_loading = false;
                    self.status = "github fetch failed".into();
                    changed = true;
                }
            }
        }

        if let Some(rx) = &self.sync_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.sync_rx = None;
                    self.sync_in_flight = false;
                    self.status.clear(); // wipe the lingering "syncing…" line
                    match result {
                        Ok(ok) => {
                            self.sync_sha = Some(ok.sha);
                            self.sync_pending = 0;
                            self.last_sync = Some(Local::now());
                            if self.config.github_repo.as_deref() != Some(ok.repo.as_str()) {
                                self.config.github_repo = Some(ok.repo);
                                let _ = self.config.save();
                            }
                            self.push_log("synced with GitHub");
                            self.toast(ToastKind::Success, "Synced with GitHub");
                        }
                        Err(e) => {
                            self.push_log(format!("sync failed: {e}"));
                            let repo = self.config.github_repo.clone().unwrap_or_default();
                            self.mode = Mode::Notice(
                                "GitHub sync failed".into(),
                                format!("{repo}\n\n{e}\n\nPress ^s to re-run the setup."),
                            );
                        }
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.sync_rx = None;
                    self.sync_in_flight = false;
                    self.push_log("sync failed");
                    self.toast(ToastKind::Error, "GitHub sync failed");
                    changed = true;
                }
            }
        }

        if let Some(rx) = &self.settings_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.settings_rx = None;
                    self.settings_in_flight = false;
                    self.status.clear(); // wipe the lingering "fetching…" line
                    match result {
                        Ok(imported) => self.offer_settings_import(imported),
                        Err(e) => {
                            self.push_log(format!("settings import failed: {e}"));
                            self.mode = Mode::Notice("Settings import failed".into(), e);
                        }
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.settings_rx = None;
                    self.settings_in_flight = false;
                    self.push_log("settings import failed");
                    self.toast(ToastKind::Error, "Settings import failed");
                    changed = true;
                }
            }
        }

        if let Some(rx) = &self.data_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.data_rx = None;
                    self.data_in_flight = false;
                    self.status.clear(); // wipe the lingering "fetching…" line
                    match result {
                        Ok(imported) => self.offer_data_import(imported),
                        Err(e) => {
                            self.push_log(format!("data import failed: {e}"));
                            self.mode = Mode::Notice("Data import failed".into(), e);
                        }
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.data_rx = None;
                    self.data_in_flight = false;
                    self.push_log("data import failed");
                    self.toast(ToastKind::Error, "Data import failed");
                    changed = true;
                }
            }
        }

        if let Some(rx) = &self.weather_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.weather_rx = None;
                    self.weather_in_flight = false;
                    match result {
                        Ok(w) => {
                            self.push_log(format!(
                                "weather · {} · {} {}°{}",
                                w.place,
                                w.label(),
                                w.temp_i(),
                                w.deg()
                            ));
                            self.weather = Some(w);
                        }
                        Err(e) => self.push_log(format!("weather fetch failed: {e}")),
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.weather_rx = None;
                    self.weather_in_flight = false;
                }
            }
        }

        changed
    }

    /// Kick off a weather fetch on a worker thread when `weather` is configured
    /// and the last attempt is stale (or there hasn't been one). Cheap to call
    /// every event-loop pass.
    pub fn maybe_refresh_weather(&mut self) {
        let Some(loc) = self.config.weather.clone().filter(|s| !s.trim().is_empty()) else {
            return;
        };
        if self.weather_in_flight {
            return;
        }
        let due = self
            .weather_last_try
            .is_none_or(|t| t.elapsed() >= crate::weather::REFRESH);
        if !due {
            return;
        }

        let unit = crate::weather::Unit::parse(self.config.weather_unit.as_deref());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::weather::fetch(&loc, unit));
        });
        self.weather_rx = Some(rx);
        self.weather_in_flight = true;
        self.weather_last_try = Some(Instant::now());
    }

    /// `^s`: push now if sync is ready, otherwise start setup. When a token is
    /// already available (config / `gh` / env) the only prompt is the repo.
    fn sync_action(&mut self) {
        if self.sync_ready() {
            self.spawn_sync(false);
            return;
        }
        let pre = self
            .config
            .github_repo
            .clone()
            .unwrap_or_else(|| "voido-data".into());
        let title = if self.sync_token.is_some() {
            "GitHub sync — repo  (name, or owner/repo; Enter = voido-data)"
        } else {
            "GitHub sync — repo  (owner/repo)"
        };
        self.begin_input(title, pre, InputAction::SyncRepo);
    }

    /// Menu → "Settings from GitHub": ask which repo (and, optionally, which
    /// file) holds the settings to adopt.
    fn import_settings_prompt(&mut self) {
        if self.settings_in_flight {
            self.status = "already fetching settings…".into();
            return;
        }
        let pre = self.config.github_repo.clone().unwrap_or_default();
        self.begin_input(
            "Import settings — owner/repo[/path], or a GitHub URL",
            pre,
            InputAction::ImportSettings,
        );
    }

    /// Fetch the settings file on a worker thread. Nothing is changed until the
    /// result lands and the user confirms the diff.
    fn spawn_settings_fetch(&mut self, spec: &str) {
        if self.settings_in_flight {
            return;
        }
        let Some(target) = crate::github::parse_repo_ref(spec) else {
            self.status = "invalid repo — use owner/repo, or a GitHub URL".into();
            return;
        };
        let label = target.label();
        let token = self.sync_token.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_settings(token.as_deref(), target));
        });
        self.settings_rx = Some(rx);
        self.settings_in_flight = true;
        self.status = format!("fetching settings from {label}…");
    }

    /// Show what the fetched file would change and ask before touching anything.
    fn offer_settings_import(&mut self, imported: ImportedSettings) {
        let diff = self.config.import_diff(&imported.config);
        if diff.is_empty() {
            self.push_log(format!(
                "settings from {} match the current ones",
                imported.source
            ));
            self.toast(ToastKind::Info, "Settings already match");
            return;
        }
        self.mode = Mode::Confirm(ConfirmState {
            prompt: format!(
                "Adopt these settings from {}?\n\n{}",
                imported.source,
                diff.join("\n")
            ),
            action: ConfirmAction::ImportSettings(Box::new(imported.config)),
        });
    }

    /// Kick off setup with the repo + token now in hand: save config, then push
    /// on a worker thread (creating the repo if needed).
    fn finish_sync_setup(&mut self) {
        let Some(repo) = self.pending_sync_repo.take() else {
            return;
        };
        if self.sync_token.is_none() {
            self.status = "sync setup needs a token".into();
            return;
        }
        self.config.storage = StorageChoice::GitHub;
        self.config.github_repo = Some(repo);
        let _ = self.config.save();
        self.status = "setting up GitHub sync…".into();
        self.spawn_sync(true);
    }

    /// Serialize the store and push it on a worker thread. With `setup`, first
    /// resolve a bare repo name to `owner/repo` and create the repo if missing.
    fn spawn_sync(&mut self, setup: bool) {
        if self.sync_in_flight {
            return;
        }
        let (Some(repo), Some(token)) = (self.config.github_repo.clone(), self.sync_token.clone())
        else {
            self.status = "GitHub sync is not configured — press ^s".into();
            return;
        };
        let json = match serde_json::to_string_pretty(&self.store) {
            Ok(j) => j,
            Err(e) => {
                self.status = format!("sync: {e}");
                return;
            }
        };
        let sha = self.sync_sha.clone();
        let file = self.config.sync_file();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_sync(&token, &repo, &file, &json, sha.as_deref(), setup));
        });
        self.sync_rx = Some(rx);
        self.sync_in_flight = true;
        self.status = "syncing with GitHub…".into();
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

    /// After a subtask edit, pull the parent todo's `done` back in line with its
    /// subtasks (all done -> todo done; any open -> todo open).
    fn sync_parent_done(&mut self) {
        if let Some(t) = self.current_todo_mut()
            && t.recompute_done()
        {
            let (title, done) = (t.title.clone(), t.done);
            self.dirty = true;
            if done {
                self.status = format!("“{title}” auto-completed");
            }
        }
    }

    pub fn current_note(&self) -> Option<&Note> {
        self.current_project()?.notes.get(self.note_idx)
    }

    pub fn current_meeting(&self) -> Option<&Meeting> {
        self.current_project()?.meetings.get(self.meeting_idx)
    }

    fn current_meeting_mut(&mut self) -> Option<&mut Meeting> {
        let i = self.meeting_idx;
        self.current_project_mut()?.meetings.get_mut(i)
    }

    /// `n` at todo level toggled the selected todo's note into the detail pane
    /// (3rd pane), and it has a non-empty note to show.
    pub fn showing_todo_note(&self) -> bool {
        self.tab == Tab::Todos
            && self.focus != Focus::Detail
            && self.todo_note_open
            && self
                .current_todo()
                .is_some_and(|t| !t.note.trim().is_empty())
    }

    /// `n` inside the Subtasks pane toggled the selected subtask's note into a
    /// section below the subtask list (4th pane), and it has one to show.
    pub fn showing_sub_note(&self) -> bool {
        self.tab == Tab::Todos
            && self.focus == Focus::Detail
            && self.sub_note_open
            && self
                .current_todo()
                .and_then(|t| t.subtasks.get(self.subtask_idx))
                .is_some_and(|s| !s.note.trim().is_empty())
    }

    /// Does the middle pane read as the live one? While a todo's note fills the
    /// detail pane it's the note that's lit, so the list steps back — it still
    /// takes the keys, but only one pane wears the accent at a time.
    pub fn content_lit(&self) -> bool {
        self.focus == Focus::Content && !self.showing_todo_note()
    }

    /// The note on screen that `^f` can expand, if there is one.
    pub fn active_note(&self) -> Option<FullNote> {
        if self.tab == Tab::Notes {
            return self
                .current_note()
                .filter(|n| !n.body.trim().is_empty())
                .map(|_| FullNote::Note);
        }
        if self.tab == Tab::Meetings {
            return self
                .current_meeting()
                .filter(|m| !m.note.trim().is_empty())
                .map(|_| FullNote::Meeting);
        }
        if self.showing_sub_note() {
            return Some(FullNote::Subtask(self.subtask_idx));
        }
        if self.showing_todo_note() {
            return Some(FullNote::Todo);
        }
        None
    }

    /// Title, body and scroll offset of the note filling the screen — what the
    /// renderer needs, resolved in one place.
    pub fn full_note_view(&self) -> Option<(String, &str, u16)> {
        match self.active_note()? {
            FullNote::Note => {
                let note = self.current_note()?;
                Some((
                    format!(" Note · {} ", note.text),
                    note.body.as_str(),
                    self.note_scroll,
                ))
            }
            FullNote::Todo => {
                let todo = self.current_todo()?;
                Some((
                    format!(" Note · {} ", todo.title),
                    todo.note.as_str(),
                    self.todo_note_scroll,
                ))
            }
            FullNote::Subtask(i) => {
                let sub = self.current_todo()?.subtasks.get(i)?;
                Some((
                    format!(" ↳ Note · {} ", sub.title),
                    sub.note.as_str(),
                    self.sub_note_scroll,
                ))
            }
            FullNote::Meeting => {
                let m = self.current_meeting()?;
                Some((
                    format!(" Meeting · {} · {} ", m.title, m.when()),
                    m.note.as_str(),
                    self.meeting_note_scroll,
                ))
            }
        }
    }

    /// `^f`: blow the note on screen up to fill the window, or drop back to the
    /// panes if it already does.
    fn toggle_note_full(&mut self) {
        if self.note_full {
            self.note_full = false;
            self.status = "note collapsed".into();
            return;
        }
        match self.active_note() {
            Some(_) => {
                self.note_full = true;
                self.status = "full-screen note — ^f or esc to close".into();
            }
            None => {
                self.status =
                    "no note on screen — press n on a todo or subtask, or l on a note".into();
            }
        }
    }

    /// Scroll whichever note `^f` is showing (or would show).
    fn scroll_active_note(&mut self, delta: i32) {
        let target = match self.active_note() {
            Some(FullNote::Note) => &mut self.note_scroll,
            Some(FullNote::Todo) => &mut self.todo_note_scroll,
            Some(FullNote::Subtask(_)) => &mut self.sub_note_scroll,
            Some(FullNote::Meeting) => &mut self.meeting_note_scroll,
            None => return,
        };
        *target = if delta < 0 {
            target.saturating_sub((-delta) as u16)
        } else {
            target.saturating_add(delta as u16)
        };
    }

    /// Keys while a note fills the window: scroll it, or step back out. The
    /// global keys (`?`, `/`, `L`, `q`, the `^` bindings) are handled before this.
    fn handle_full_note_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_active_note(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_active_note(-1),
            KeyCode::PageDown => self.scroll_active_note(10),
            KeyCode::PageUp => self.scroll_active_note(-10),
            KeyCode::Char('g') | KeyCode::Home => self.scroll_active_note(-30_000),
            KeyCode::Char('G') | KeyCode::End => self.scroll_active_note(30_000),
            KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left => {
                self.note_full = false;
                self.status = "note collapsed".into();
            }
            _ => {}
        }
    }

    /// `n` on a todo: flip its note in/out of the detail pane.
    fn toggle_todo_note(&mut self) {
        match self.current_todo() {
            Some(t) if !t.note.trim().is_empty() => {
                self.todo_note_open = !self.todo_note_open;
                self.todo_note_scroll = 0;
            }
            Some(_) => self.status = "no note yet — press N to write one".into(),
            None => {}
        }
    }

    /// `n` on a subtask: flip its note in/out of the pane below the list.
    fn toggle_sub_note(&mut self) {
        match self
            .current_todo()
            .and_then(|t| t.subtasks.get(self.subtask_idx))
        {
            Some(s) if !s.note.trim().is_empty() => {
                self.sub_note_open = !self.sub_note_open;
                self.sub_note_scroll = 0;
                // Opening jumps straight into the note so it can be scrolled;
                // closing drops the focus flag with it.
                self.sub_note_focus = self.sub_note_open;
            }
            Some(_) => self.status = "no note yet — press N to write one".into(),
            None => {}
        }
    }

    /// Scroll whichever detail note is currently on screen.
    fn scroll_detail_note(&mut self, delta: i32) {
        let target = if self.showing_sub_note() {
            &mut self.sub_note_scroll
        } else {
            &mut self.todo_note_scroll
        };
        *target = if delta < 0 {
            target.saturating_sub((-delta) as u16)
        } else {
            target.saturating_add(delta as u16)
        };
    }

    /// Whether the right-hand detail pane has something to show right now.
    fn detail_available(&self) -> bool {
        match self.tab {
            Tab::Todos => self.current_todo().is_some(),
            Tab::Notes => self.current_note().is_some(),
            Tab::Meetings => self.current_meeting().is_some(),
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
                            (0..=7).contains(&diff)
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
                        (0..=7).contains(&diff)
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
                    if (0..=7).contains(&diff) {
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
                if (0..=7).contains(&diff) {
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
        let todo_note_lines = self
            .current_todo()
            .map(|t| t.note.lines().count() as u16)
            .unwrap_or(0);
        self.todo_note_scroll = self.todo_note_scroll.min(todo_note_lines.saturating_add(4));
        let sub_note_lines = self
            .current_todo()
            .and_then(|t| t.subtasks.get(self.subtask_idx))
            .map(|s| s.note.lines().count() as u16)
            .unwrap_or(0);
        self.sub_note_scroll = self.sub_note_scroll.min(sub_note_lines.saturating_add(4));
        let tl = self.timeline().len();
        self.timeline_idx = self.timeline_idx.min(tl.saturating_sub(1));
        let meetings = self
            .current_project()
            .map(|p| p.meetings.len())
            .unwrap_or(0);
        self.meeting_idx = self.meeting_idx.min(meetings.saturating_sub(1));
        let meeting_note_lines = self
            .current_meeting()
            .map(|m| m.note.lines().count() as u16 + 8)
            .unwrap_or(0);
        self.meeting_note_scroll = self
            .meeting_note_scroll
            .min(meeting_note_lines.saturating_sub(1));

        if self.focus == Focus::Detail && !self.detail_available() {
            self.focus = Focus::Content;
        }
        // The subtask-note pane can't hold focus once it's no longer on screen.
        if self.sub_note_focus && !self.showing_sub_note() {
            self.sub_note_focus = false;
        }
        // Nothing left to blow up (note deleted, selection moved off it).
        if self.note_full && self.active_note().is_none() {
            self.note_full = false;
        }
    }

    // ---- top level dispatch --------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        match self.mode {
            Mode::Normal => self.handle_normal(key),
            Mode::Input(_) => self.handle_input(key),
            Mode::Confirm(_) => self.handle_confirm(key),
            Mode::EditBody(_) => self.handle_edit_body(key),
            Mode::Theme(_) => self.handle_theme(key),
            Mode::Attach(_) => self.handle_attach(key),
            Mode::Tags(_) => self.handle_tags(key),
            Mode::Links(_) => self.handle_links(key),
            Mode::Menu(_) => self.handle_menu(key),
            Mode::Sort(_) => self.handle_sort(key),
            Mode::Search(_) => self.handle_search(key),
            Mode::Help | Mode::GitHub | Mode::Weather | Mode::Notice(..) => {
                self.mode = Mode::Normal
            }
        }
    }

    /// Route a mouse event. Left-click focuses the pane under the pointer (and
    /// selects the row it landed on); the wheel scrolls the focused pane.
    /// Overlays own the screen, so mouse input is ignored while one is open.
    /// Returns `true` when something changed and the UI should redraw.
    pub fn handle_mouse(&mut self, me: MouseEvent) -> bool {
        if !matches!(self.mode, Mode::Normal) {
            return false;
        }
        let pos = Position::new(me.column, me.row);
        match me.kind {
            MouseEventKind::Down(MouseButton::Left) => self.click_pane(pos),
            MouseEventKind::ScrollDown => {
                self.wheel_scroll(pos, 3);
                true
            }
            MouseEventKind::ScrollUp => {
                self.wheel_scroll(pos, -3);
                true
            }
            _ => false,
        }
    }

    /// Route a wheel tick: scroll a note pane when the pointer is over one that's
    /// showing (or the sub-note pane has focus), otherwise move the selection.
    fn wheel_scroll(&mut self, pos: Position, delta: i32) {
        if self.note_full {
            self.scroll_active_note(delta);
            return;
        }
        let over_detail = {
            let r = self.pane_rects.borrow();
            r.detail.area() > 0 && r.detail.contains(pos)
        };
        let scroll_note = (self.sub_note_focus && self.showing_sub_note())
            || (over_detail && (self.showing_todo_note() || self.showing_sub_note()));
        if scroll_note {
            self.scroll_detail_note(delta);
        } else {
            self.move_sel(delta);
        }
    }

    fn click_pane(&mut self, pos: Position) -> bool {
        let r = *self.pane_rects.borrow();

        // The tab strip lives on the content pane's top border row.
        if r.content.area() > 0
            && pos.y == r.content.y
            && let Some(t) = tab_at_x(r.content, pos.x)
        {
            self.goto_tab(t);
            return true;
        }

        if r.projects.contains(pos) {
            self.focus = Focus::Projects;
            if let Some(i) = row_at(r.projects, pos.y, self.store.projects.len()) {
                self.select_project_to(i);
            }
            return true;
        }

        // The detail pane, when one is on screen.
        if r.detail.area() > 0 && r.detail.contains(pos) && self.detail_available() {
            // The todo's note is filling this pane — a click there must not
            // dismiss it (it drops out the moment focus moves to Detail). Leave
            // everything alone so the text stays put to be selected / copied.
            if self.showing_todo_note() {
                return false;
            }
            self.focus = Focus::Detail;
            if self.tab == Tab::Todos {
                let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
                if let Some(i) = row_at(r.detail, pos.y, len) {
                    self.subtask_idx = i;
                    self.sub_note_scroll = 0;
                }
            }
            return true;
        }

        if r.content.area() > 0 && r.content.contains(pos) && !self.store.projects.is_empty() {
            self.focus = Focus::Content;
            match self.tab {
                Tab::Todos => {
                    let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                    if let Some(i) = row_at(r.content, pos.y, len) {
                        self.todo_idx = i;
                        self.subtask_idx = 0;
                        self.todo_note_scroll = 0;
                        self.sub_note_scroll = 0;
                    }
                }
                Tab::Notes => {
                    let len = self.current_project().map(|p| p.notes.len()).unwrap_or(0);
                    if let Some(i) = row_at(r.content, pos.y, len) {
                        self.note_idx = i;
                        self.note_scroll = 0;
                    }
                }
                Tab::Schedule => {
                    let len = self.timeline().len();
                    if let Some(i) = row_at(r.content, pos.y, len) {
                        self.timeline_idx = i;
                    }
                }
                Tab::Meetings => {
                    let len = self
                        .current_project()
                        .map(|p| p.meetings.len())
                        .unwrap_or(0);
                    if let Some(i) = row_at(r.content, pos.y, len) {
                        self.meeting_idx = i;
                        self.meeting_note_scroll = 0;
                    }
                }
                Tab::Overview => {}
            }
            return true;
        }

        false
    }

    fn select_project_to(&mut self, i: usize) {
        if i != self.project_idx {
            self.project_idx = i;
            self.reset_content_idx();
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => self.should_quit = true,
                KeyCode::Char('g') => self.link_repo_prompt(),
                KeyCode::Char('s') => self.sync_action(),
                KeyCode::Char('e') => self.open_settings = true,
                KeyCode::Char('t') => self.open_theme(),
                KeyCode::Char('k') => self.mode = Mode::Menu(MenuState { sel: 0 }),
                KeyCode::Char('w') => self.open_weather(),
                KeyCode::Char('f') => self.toggle_note_full(),
                KeyCode::Char('l') => {
                    self.activity_open = !self.activity_open;
                    self.status = if self.activity_open {
                        "activity panel — ^l to hide".into()
                    } else {
                        String::new()
                    };
                }
                // page scroll in the note body
                KeyCode::Char('d') if self.note_full => self.scroll_active_note(10),
                KeyCode::Char('u') if self.note_full => self.scroll_active_note(-10),
                KeyCode::Char('d') if self.focus == Focus::Detail && self.tab == Tab::Notes => {
                    self.note_scroll = self.note_scroll.saturating_add(10);
                }
                KeyCode::Char('u') if self.focus == Focus::Detail && self.tab == Tab::Notes => {
                    self.note_scroll = self.note_scroll.saturating_sub(10);
                }
                KeyCode::Char('d') if self.focus == Focus::Detail && self.tab == Tab::Meetings => {
                    self.scroll_meeting_note(10);
                }
                KeyCode::Char('u') if self.focus == Focus::Detail && self.tab == Tab::Meetings => {
                    self.scroll_meeting_note(-10);
                }
                // scroll the note(s) shown under the subtask list
                KeyCode::Char('d') if self.showing_todo_note() || self.showing_sub_note() => {
                    self.scroll_detail_note(6);
                }
                KeyCode::Char('u') if self.showing_todo_note() || self.showing_sub_note() => {
                    self.scroll_detail_note(-6);
                }
                _ => {}
            }
            return;
        }

        let g_pending = std::mem::replace(&mut self.pending_g, false);

        // A full-screen note swallows the navigation keys — there are no panes
        // to move between — but leaves the global ones below it alone.
        if self.note_full
            && !matches!(
                key.code,
                KeyCode::Char('q' | '?' | '/' | 'L' | 'm' | '1' | '2' | '3' | '4' | '5')
                    | KeyCode::Tab
                    | KeyCode::BackTab
            )
        {
            self.handle_full_note_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('q') => {
                self.mode = Mode::Confirm(ConfirmState {
                    prompt: "Quit voido?".into(),
                    action: ConfirmAction::Quit,
                });
            }
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('/') => self.open_search(),
            KeyCode::Char('L') => self.open_links(),
            KeyCode::Char('m') => {
                self.minimal = !self.minimal;
                self.status = if self.minimal {
                    "minimal view — m to restore".into()
                } else {
                    "full view".into()
                };
            }
            // Tab switches the content view (Overview / Todos / Notes / Schedule).
            KeyCode::Tab => self.cycle_tab(true),
            KeyCode::BackTab => self.cycle_tab(false),
            KeyCode::Char('1') => self.goto_tab(Tab::Overview),
            KeyCode::Char('2') => self.goto_tab(Tab::Todos),
            KeyCode::Char('3') => self.goto_tab(Tab::Notes),
            KeyCode::Char('4') => self.goto_tab(Tab::Schedule),
            KeyCode::Char('5') => self.goto_tab(Tab::Meetings),
            // Switch project from anywhere, without leaving the current view.
            KeyCode::Char('w') => self.select_project(-1),
            KeyCode::Char('s') => self.select_project(1),
            KeyCode::Esc => {
                if self.sub_note_focus {
                    self.sub_note_focus = false;
                } else if self.tab == Tab::Notes
                    && self.focus == Focus::Detail
                    && self.note_expanded
                {
                    self.note_expanded = false;
                } else {
                    self.focus = match self.focus {
                        Focus::Detail => Focus::Content,
                        _ => Focus::Projects,
                    };
                }
            }
            KeyCode::Char('g') => {
                if self.sub_note_focus {
                    self.sub_note_scroll = 0;
                } else if g_pending {
                    self.move_sel(-1_000_000);
                } else {
                    self.pending_g = true;
                }
            }
            KeyCode::Char('G') => {
                if self.sub_note_focus {
                    self.scroll_detail_note(30_000);
                } else {
                    self.move_sel(1_000_000);
                }
            }
            _ => match self.focus {
                Focus::Projects => self.handle_projects_key(key),
                Focus::Content => match self.tab {
                    Tab::Overview => self.handle_overview_key(key),
                    Tab::Todos => self.handle_todos_key(key),
                    Tab::Notes => self.handle_notes_key(key),
                    Tab::Schedule => self.handle_timeline_key(key),
                    Tab::Meetings => self.handle_meetings_key(key),
                },
                Focus::Detail if self.sub_note_focus && self.showing_sub_note() => {
                    self.handle_sub_note_key(key)
                }
                Focus::Detail => match self.tab {
                    Tab::Notes => self.handle_note_body_key(key),
                    Tab::Meetings => self.handle_meeting_note_key(key),
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
            KeyCode::Char('a') => self.begin_input(
                "New project   (#tag for tags)",
                String::new(),
                InputAction::AddProject,
            ),
            KeyCode::Char('r') => {
                if let Some(p) = self.current_project() {
                    let pre = project_edit_string(p);
                    self.begin_input("Rename project   (#tag)", pre, InputAction::RenameProject);
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
            KeyCode::Char('o') => self.open_sort(SortScope::Projects),
            KeyCode::Char('R') => self.show_github(),
            KeyCode::Char('t') => self.open_tags(),
            KeyCode::Char('i') => self.project_info = !self.project_info,
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
                    self.sub_note_scroll = 0;
                    self.focus = Focus::Detail;
                }
            }
            KeyCode::Char('n') => self.toggle_todo_note(),
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input(
                        "New todo   (!1..!3 · @YYYY-MM-DD · #tag)",
                        String::new(),
                        InputAction::AddTodo,
                    );
                } else {
                    self.status = "create a project first (press 1, then a)".into();
                }
            }
            KeyCode::Char('e') => {
                if let Some(t) = self
                    .current_project()
                    .and_then(|p| p.todos.get(self.todo_idx))
                {
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
                    let (title, done) = (t.title.clone(), t.done);
                    self.dirty = true;
                    self.status = format!("{} “{title}”", if done { "done" } else { "reopened" });
                }
            }
            KeyCode::Char('p') => {
                let i = self.todo_idx;
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.priority = t.priority.next();
                    let (title, prio) = (t.title.clone(), t.priority.label());
                    self.dirty = true;
                    self.status = format!("“{title}” priority: {prio}");
                }
            }
            KeyCode::Char('d') => {
                if let Some(t) = self
                    .current_project()
                    .and_then(|p| p.todos.get(self.todo_idx))
                {
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
            KeyCode::Char('o') => self.open_sort(SortScope::Todos),
            KeyCode::Char('N') => self.begin_edit_todo_note(),
            KeyCode::Char('A') => self.open_attachments(),
            KeyCode::Char('t') => self.open_tags(),
            KeyCode::Char('i') => self.todo_info = !self.todo_info,
            _ => {}
        }
    }

    /// `o` — open the ordering menu for the list in focus, sitting on whatever
    /// order that list is already in.
    fn open_sort(&mut self, scope: SortScope) {
        let active = self.sorts[scope.idx()];
        let sel = active
            .and_then(|o| scope.keys().iter().position(|k| *k == o.key))
            .unwrap_or(0);
        let reverse = active.map(|o| o.reverse).unwrap_or(false);
        self.mode = Mode::Sort(SortState {
            scope,
            sel,
            reverse,
            active,
        });
    }

    fn handle_sort(&mut self, key: KeyEvent) {
        let Mode::Sort(state) = &mut self.mode else {
            return;
        };
        let len = state.scope.keys().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('o') => self.mode = Mode::Normal,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char('j') | KeyCode::Down => state.sel = step(state.sel, 1, len),
            KeyCode::Char('k') | KeyCode::Up => state.sel = step(state.sel, -1, len),
            KeyCode::Char('g') | KeyCode::Home => state.sel = 0,
            KeyCode::Char('G') | KeyCode::End => state.sel = len - 1,
            // Flip the direction the highlighted ordering runs in.
            KeyCode::Char('r') | KeyCode::Tab => state.reverse = !state.reverse,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char(' ') => {
                let (scope, sort_key, reverse) =
                    (state.scope, state.scope.keys()[state.sel], state.reverse);
                self.mode = Mode::Normal;
                self.apply_sort(scope, sort_key, reverse);
            }
            _ => {}
        }
    }

    /// Reorder the list `scope` names, remember the choice, and keep the
    /// selection on the row the user was sitting on. Sorts are stable, so items
    /// that tie hold the hand-ordered positions `J`/`K` gave them.
    fn apply_sort(&mut self, scope: SortScope, key: SortKey, reverse: bool) {
        let sorted = match scope {
            SortScope::Projects => {
                let sel = self.current_project().map(|p| p.name.clone());
                let long_enough = self.store.projects.len() > 1;
                if long_enough {
                    self.store
                        .projects
                        .sort_by(|a, b| cmp_project(a, b, key, reverse));
                    if let Some(name) = sel
                        && let Some(i) = self.store.projects.iter().position(|p| p.name == name)
                    {
                        self.project_idx = i;
                    }
                }
                long_enough
            }
            SortScope::Todos => {
                let sel = self.current_todo().map(|t| t.title.clone());
                let mut long_enough = false;
                if let Some(p) = self.current_project_mut()
                    && p.todos.len() > 1
                {
                    p.todos.sort_by(|a, b| cmp_todo(a, b, key, reverse));
                    long_enough = true;
                }
                if long_enough {
                    if let Some(title) = sel
                        && let Some(p) = self.current_project()
                        && let Some(i) = p.todos.iter().position(|t| t.title == title)
                    {
                        self.todo_idx = i;
                    }
                    self.subtask_idx = 0;
                }
                long_enough
            }
            SortScope::Subtasks => {
                let sel = self
                    .current_todo()
                    .and_then(|t| t.subtasks.get(self.subtask_idx))
                    .map(|s| s.title.clone());
                let mut long_enough = false;
                if let Some(t) = self.current_todo_mut()
                    && t.subtasks.len() > 1
                {
                    t.subtasks.sort_by(|a, b| cmp_subtask(a, b, key, reverse));
                    long_enough = true;
                }
                if long_enough
                    && let Some(title) = sel
                    && let Some(t) = self.current_todo()
                    && let Some(i) = t.subtasks.iter().position(|s| s.title == title)
                {
                    self.subtask_idx = i;
                }
                long_enough
            }
            SortScope::Meetings => {
                let sel = self.current_meeting().map(|m| m.title.clone());
                let mut long_enough = false;
                if let Some(p) = self.current_project_mut()
                    && p.meetings.len() > 1
                {
                    p.meetings.sort_by(|a, b| cmp_meeting(a, b, key, reverse));
                    long_enough = true;
                }
                if long_enough
                    && let Some(title) = sel
                    && let Some(p) = self.current_project()
                    && let Some(i) = p.meetings.iter().position(|m| m.title == title)
                {
                    self.meeting_idx = i;
                    self.meeting_note_scroll = 0;
                }
                long_enough
            }
            SortScope::Notes => {
                let sel = self.current_note().map(|n| n.text.clone());
                let mut long_enough = false;
                if let Some(p) = self.current_project_mut()
                    && p.notes.len() > 1
                {
                    p.notes.sort_by(|a, b| cmp_note(a, b, key, reverse));
                    long_enough = true;
                }
                if long_enough
                    && let Some(text) = sel
                    && let Some(p) = self.current_project()
                    && let Some(i) = p.notes.iter().position(|n| n.text == text)
                {
                    self.note_idx = i;
                    self.note_scroll = 0;
                }
                long_enough
            }
        };
        self.sorts[scope.idx()] = Some(SortOrder { key, reverse });
        if sorted {
            self.dirty = true;
            self.status = format!(
                "{} sorted by {} — {}",
                scope.label(),
                key.label().to_lowercase(),
                key.hint(reverse)
            );
        } else {
            self.status = format!("nothing to sort — fewer than two {}", scope.label());
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
        self.status = "moved todo".into();
    }

    // ---- subtasks pane ------------------------------------------

    fn handle_subtasks_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Content,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if self.showing_sub_note() {
                    self.sub_note_focus = true;
                } else {
                    self.toggle_sub_note();
                }
            }
            KeyCode::Char('n') => self.toggle_sub_note(),
            KeyCode::Char('a') => {
                if self.current_todo().is_some() {
                    self.begin_input(
                        "New subtask   (!1..!3 · #tag)",
                        String::new(),
                        InputAction::AddSubtask,
                    );
                }
            }
            KeyCode::Char('e') => {
                if let Some(s) = self
                    .current_todo()
                    .and_then(|t| t.subtasks.get(self.subtask_idx))
                {
                    let pre = subtask_edit_string(s);
                    let i = self.subtask_idx;
                    self.begin_input("Edit subtask", pre, InputAction::EditSubtask(i));
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                let i = self.subtask_idx;
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.done = !s.done;
                    let (title, done) = (s.title.clone(), s.done);
                    self.dirty = true;
                    self.status = format!(
                        "subtask {} “{title}”",
                        if done { "done" } else { "reopened" }
                    );
                }
                self.sync_parent_done();
            }
            KeyCode::Char('p') => {
                let i = self.subtask_idx;
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.priority = s.priority.next();
                    let (title, prio) = (s.title.clone(), s.priority.label());
                    self.dirty = true;
                    self.status = format!("subtask “{title}” priority: {prio}");
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
            KeyCode::Char('o') => self.open_sort(SortScope::Subtasks),
            KeyCode::Char('N') => self.begin_edit_subtask_note(),
            KeyCode::Char('A') => self.open_subtask_attachments(),
            KeyCode::Char('t') => self.open_tags(),
            KeyCode::Char('i') => self.subtask_info = !self.subtask_info,
            _ => {}
        }
    }

    /// Keys while focus sits in the subtask-note pane (opened with `n` / `l`):
    /// scroll the note, edit it, or step back out to the subtask list.
    fn handle_sub_note_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_detail_note(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_detail_note(-1),
            KeyCode::PageDown => self.scroll_detail_note(10),
            KeyCode::PageUp => self.scroll_detail_note(-10),
            KeyCode::Char('N') | KeyCode::Char('e') => self.begin_edit_subtask_note(),
            KeyCode::Char('h') | KeyCode::Left => self.sub_note_focus = false,
            KeyCode::Char('n') => {
                self.sub_note_open = false;
                self.sub_note_focus = false;
            }
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
        self.status = "moved subtask".into();
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
                    let pre = project_edit_string(p);
                    self.begin_input("Rename project   (#tag)", pre, InputAction::RenameProject);
                }
            }
            KeyCode::Char('t') => self.open_tags(),
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
                    let pinned = n.pinned;
                    self.dirty = true;
                    self.status = if pinned {
                        "note pinned"
                    } else {
                        "note unpinned"
                    }
                    .into();
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
            KeyCode::Char('o') => self.open_sort(SortScope::Notes),
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
        self.status = "moved note".into();
    }

    // ---- meetings tab ------------------------------------------

    fn handle_meetings_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                if self.current_meeting().is_some() {
                    self.meeting_note_scroll = 0;
                    self.focus = Focus::Detail;
                }
            }
            KeyCode::Char('a') => {
                if self.current_project().is_some() {
                    self.begin_input(MEETING_PROMPT, String::new(), InputAction::AddMeeting);
                } else {
                    self.status = "create a project first".into();
                }
            }
            KeyCode::Char('e') => {
                if let Some(m) = self.current_meeting() {
                    let pre = meeting_edit_string(m);
                    let i = self.meeting_idx;
                    self.begin_input("Edit meeting", pre, InputAction::EditMeeting(i));
                }
            }
            KeyCode::Char('r') => {
                if let Some(m) = self.current_meeting() {
                    let pre = match &m.time {
                        Some(t) => format!("@{} {t}", m.date.format("%Y-%m-%d")),
                        None => format!("@{}", m.date.format("%Y-%m-%d")),
                    };
                    let i = self.meeting_idx;
                    self.begin_input(
                        "Reschedule meeting   (@YYYY-MM-DD · 14:30)",
                        pre,
                        InputAction::RescheduleMeeting(i),
                    );
                }
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => {
                if let Some(m) = self.current_meeting_mut() {
                    m.held = !m.held;
                    let (title, held) = (m.title.clone(), m.held);
                    self.dirty = true;
                    self.status = format!(
                        "“{title}” {}",
                        if held { "held" } else { "back on the calendar" }
                    );
                }
            }
            KeyCode::Char('d') => {
                if let Some(m) = self.current_meeting() {
                    let prompt = format!("Delete meeting \"{}\"?", m.title);
                    let i = self.meeting_idx;
                    self.mode = Mode::Confirm(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteMeeting(i),
                    });
                }
            }
            KeyCode::Char('J') => self.reorder_meeting(1),
            KeyCode::Char('K') => self.reorder_meeting(-1),
            KeyCode::Char('o') => self.open_sort(SortScope::Meetings),
            KeyCode::Char('N') => self.begin_edit_meeting_note(),
            KeyCode::Char('i') => self.meeting_info = !self.meeting_info,
            _ => {}
        }
    }

    fn reorder_meeting(&mut self, delta: i32) {
        let i = self.meeting_idx;
        let len = self
            .current_project()
            .map(|p| p.meetings.len())
            .unwrap_or(0);
        if len < 2 {
            return;
        }
        let j = (i as i32 + delta).clamp(0, len as i32 - 1) as usize;
        if i == j {
            return;
        }
        if let Some(p) = self.current_project_mut() {
            p.meetings.swap(i, j);
        }
        self.meeting_idx = j;
        self.dirty = true;
        self.status = "moved meeting".into();
    }

    /// Keys while focus sits in the agenda / minutes pane beside the meeting list.
    fn handle_meeting_note_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_meeting_note(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_meeting_note(-1),
            KeyCode::PageDown => self.scroll_meeting_note(10),
            KeyCode::PageUp => self.scroll_meeting_note(-10),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Content,
            KeyCode::Char('e') | KeyCode::Char('N') | KeyCode::Enter => {
                self.begin_edit_meeting_note()
            }
            _ => {}
        }
    }

    fn scroll_meeting_note(&mut self, delta: i32) {
        self.meeting_note_scroll = if delta < 0 {
            self.meeting_note_scroll
                .saturating_sub((-delta).min(i32::from(u16::MAX)) as u16)
        } else {
            self.meeting_note_scroll
                .saturating_add(delta.min(i32::from(u16::MAX)) as u16)
        };
    }

    /// Open the Markdown editor on the selected meeting's agenda / minutes.
    fn begin_edit_meeting_note(&mut self) {
        let Some(m) = self.current_meeting() else {
            self.status = "pick a meeting first".into();
            return;
        };
        let note = m.note.clone();
        self.begin_edit(EditTarget::MeetingNote(self.meeting_idx), &note);
        self.status = "editing meeting notes — esc / ^s to save".into();
    }

    // ---- note body pane (rendered markdown, beside a note) ------

    fn handle_note_body_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.note_scroll = self.note_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.note_scroll = self.note_scroll.saturating_sub(1)
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
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
                self.note_expanded = !self.note_expanded;
            }
            KeyCode::Char('e') | KeyCode::Char('i') => self.begin_edit_body(),
            _ => {}
        }
    }

    fn begin_edit_body(&mut self) {
        let Some(note) = self.current_note() else {
            return;
        };
        let body = note.body.clone();
        self.begin_edit(EditTarget::NoteBody(self.note_idx), &body);
        self.status = "editing note — esc / ^s to save".into();
    }

    /// Open the Markdown editor on the current todo's attached note.
    fn begin_edit_todo_note(&mut self) {
        let Some(todo) = self.current_todo() else {
            self.status = "pick a todo first".into();
            return;
        };
        let note = todo.note.clone();
        self.begin_edit(EditTarget::TodoNote(self.todo_idx), &note);
        self.status = "editing todo note — esc / ^s to save".into();
    }

    /// Open the Markdown editor on the selected subtask's attached note.
    fn begin_edit_subtask_note(&mut self) {
        let Some(sub) = self
            .current_todo()
            .and_then(|t| t.subtasks.get(self.subtask_idx))
        else {
            self.status = "pick a subtask first".into();
            return;
        };
        let note = sub.note.clone();
        self.begin_edit(EditTarget::SubtaskNote(self.subtask_idx), &note);
        self.status = "editing subtask note — esc / ^s to save".into();
    }

    fn begin_edit(&mut self, target: EditTarget, initial: &str) {
        let lines: Vec<String> = if initial.is_empty() {
            vec![String::new()]
        } else {
            initial.lines().map(str::to_string).collect()
        };
        let mut textarea = TextArea::new(lines);
        textarea.move_cursor(CursorMove::Bottom);
        textarea.move_cursor(CursorMove::End);
        textarea.set_placeholder_text("Write in Markdown…  # heading, - bullet, **bold**, `code`");
        self.mode = Mode::EditBody(Box::new(EditState { target, textarea }));
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
        let body = state.textarea.lines().join("\n").trim_end().to_string();
        match state.target {
            EditTarget::NoteBody(i) => {
                if let Some(n) = self.current_project_mut().and_then(|p| p.notes.get_mut(i)) {
                    n.body = body;
                    self.dirty = true;
                    self.status = "note saved".into();
                }
                self.note_scroll = 0;
            }
            EditTarget::TodoNote(i) => {
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.note = body;
                    self.dirty = true;
                    self.status = "todo note saved".into();
                }
            }
            EditTarget::MeetingNote(i) => {
                if let Some(m) = self
                    .current_project_mut()
                    .and_then(|p| p.meetings.get_mut(i))
                {
                    m.note = body;
                    self.dirty = true;
                    self.status = "meeting notes saved".into();
                }
                self.meeting_note_scroll = 0;
            }
            EditTarget::SubtaskNote(i) => {
                let ti = self.todo_idx;
                if let Some(s) = self
                    .current_project_mut()
                    .and_then(|p| p.todos.get_mut(ti))
                    .and_then(|t| t.subtasks.get_mut(i))
                {
                    s.note = body;
                    self.dirty = true;
                    self.status = "subtask note saved".into();
                }
            }
        }
    }

    // ---- attachments -------------------------------------------

    /// Open the manager for the todo selected in the Todos tab.
    fn open_attachments(&mut self) {
        if self.current_todo().is_none() {
            self.status = "pick a todo first".into();
            return;
        }
        self.open_attachments_for(AttachTarget {
            todo_idx: self.todo_idx,
            sub_idx: None,
        });
    }

    /// Open the manager for the subtask selected in the Subtasks pane.
    fn open_subtask_attachments(&mut self) {
        let Some(t) = self.current_todo() else {
            self.status = "pick a todo first".into();
            return;
        };
        if self.subtask_idx >= t.subtasks.len() {
            self.status = "pick a subtask first".into();
            return;
        }
        self.open_attachments_for(AttachTarget {
            todo_idx: self.todo_idx,
            sub_idx: Some(self.subtask_idx),
        });
    }

    fn open_attachments_for(&mut self, target: AttachTarget) {
        self.mode = Mode::Attach(AttachState { target, sel: 0 });
    }

    /// The attachment list a target points at (read-only).
    pub fn attachments_at(&self, t: AttachTarget) -> Option<&Vec<Attachment>> {
        let todo = self.current_project()?.todos.get(t.todo_idx)?;
        match t.sub_idx {
            None => Some(&todo.attachments),
            Some(i) => todo.subtasks.get(i).map(|s| &s.attachments),
        }
    }

    fn attachments_at_mut(&mut self, t: AttachTarget) -> Option<&mut Vec<Attachment>> {
        let todo = self.current_project_mut()?.todos.get_mut(t.todo_idx)?;
        match t.sub_idx {
            None => Some(&mut todo.attachments),
            Some(i) => todo.subtasks.get_mut(i).map(|s| &mut s.attachments),
        }
    }

    fn handle_attach(&mut self, key: KeyEvent) {
        let Mode::Attach(state) = &self.mode else {
            return;
        };
        let (target, sel) = (state.target, state.sel);
        let len = self.attachments_at(target).map(Vec::len).unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                if let Mode::Attach(s) = &mut self.mode {
                    s.sel = step(sel, 1, len);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Mode::Attach(s) = &mut self.mode {
                    s.sel = step(sel, -1, len);
                }
            }
            KeyCode::Char('a') => self.begin_input(
                "Attachment — URL or path   (append  | label  to name it)",
                String::new(),
                InputAction::AddAttachment(target),
            ),
            KeyCode::Char('d') => {
                if let Some(list) = self.attachments_at_mut(target)
                    && sel < list.len()
                {
                    list.remove(sel);
                    self.dirty = true;
                    self.status = "attachment removed".into();
                }
                if let Mode::Attach(s) = &mut self.mode {
                    s.sel = s.sel.min(len.saturating_sub(2));
                }
            }
            KeyCode::Char('o') | KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(a) = self
                    .attachments_at(target)
                    .and_then(|list| list.get(sel))
                    .map(|a| a.value.clone())
                {
                    self.open_external(&a);
                }
            }
            _ => {}
        }
    }

    /// Hand a URL or path to the OS opener (`open` / `xdg-open` / `explorer`).
    fn open_external(&mut self, target: &str) {
        let target = target.trim();
        if target.is_empty() {
            return;
        }
        let program = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "explorer"
        } else {
            "xdg-open"
        };
        match std::process::Command::new(program)
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.status = format!("opening {}", truncate(target, 40)),
            Err(e) => self.status = format!("could not open: {e}"),
        }
    }

    // ---- tags -------------------------------------------------

    /// `t` opens the tag manager for whatever's under the cursor.
    fn open_tags(&mut self) {
        let target = match self.focus {
            Focus::Projects => Some(TagTarget::Project),
            Focus::Content if self.tab == Tab::Overview => Some(TagTarget::Project),
            Focus::Content if self.tab == Tab::Todos && self.current_todo().is_some() => {
                Some(TagTarget::Todo(self.todo_idx))
            }
            Focus::Detail
                if self.tab == Tab::Todos
                    && self
                        .current_todo()
                        .is_some_and(|t| self.subtask_idx < t.subtasks.len()) =>
            {
                Some(TagTarget::Subtask {
                    todo: self.todo_idx,
                    sub: self.subtask_idx,
                })
            }
            _ => None,
        };
        match target {
            Some(t) => self.mode = Mode::Tags(TagState { target: t, sel: 0 }),
            None => self.status = "nothing here to tag".into(),
        }
    }

    pub fn tags_at(&self, t: TagTarget) -> Option<&Vec<String>> {
        let p = self.current_project()?;
        match t {
            TagTarget::Project => Some(&p.tags),
            TagTarget::Todo(i) => p.todos.get(i).map(|t| &t.tags),
            TagTarget::Subtask { todo, sub } => p
                .todos
                .get(todo)
                .and_then(|t| t.subtasks.get(sub))
                .map(|s| &s.tags),
        }
    }

    fn tags_at_mut(&mut self, t: TagTarget) -> Option<&mut Vec<String>> {
        let p = self.current_project_mut()?;
        match t {
            TagTarget::Project => Some(&mut p.tags),
            TagTarget::Todo(i) => p.todos.get_mut(i).map(|t| &mut t.tags),
            TagTarget::Subtask { todo, sub } => p
                .todos
                .get_mut(todo)
                .and_then(|t| t.subtasks.get_mut(sub))
                .map(|s| &mut s.tags),
        }
    }

    fn handle_tags(&mut self, key: KeyEvent) {
        let Mode::Tags(state) = &self.mode else {
            return;
        };
        let (target, sel) = (state.target, state.sel);
        let len = self.tags_at(target).map(Vec::len).unwrap_or(0);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                if let Mode::Tags(s) = &mut self.mode {
                    s.sel = step(sel, 1, len);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Mode::Tags(s) = &mut self.mode {
                    s.sel = step(sel, -1, len);
                }
            }
            KeyCode::Char('a') => self.begin_input(
                "Add tags   (space-separated · # optional)",
                String::new(),
                InputAction::AddTag(target),
            ),
            KeyCode::Char('d') => {
                if let Some(list) = self.tags_at_mut(target)
                    && sel < list.len()
                {
                    let removed = list.remove(sel);
                    self.dirty = true;
                    self.status = format!("removed #{removed}");
                }
                if let Mode::Tags(s) = &mut self.mode {
                    s.sel = s.sel.min(len.saturating_sub(2));
                }
            }
            _ => {}
        }
    }

    // ---- links in a note -------------------------------------

    /// The markdown text currently visible in a note pane, if any.
    fn current_markdown(&self) -> Option<&str> {
        match self.tab {
            Tab::Notes => self.current_note().map(|n| n.body.as_str()),
            Tab::Todos if self.showing_sub_note() => self
                .current_todo()
                .and_then(|t| t.subtasks.get(self.subtask_idx))
                .map(|s| s.note.as_str()),
            Tab::Todos if self.showing_todo_note() => self.current_todo().map(|t| t.note.as_str()),
            Tab::Meetings => self.current_meeting().map(|m| m.note.as_str()),
            _ => None,
        }
    }

    /// `L` — pull every link out of the note on screen so it can be opened or
    /// copied.
    fn open_links(&mut self) {
        let Some(md) = self.current_markdown() else {
            self.status =
                "no note on screen — press n on a todo/subtask, or open the Notes tab".into();
            return;
        };
        let items = crate::md::extract_links(md);
        if items.is_empty() {
            self.status = "no links in this note".into();
            return;
        }
        self.mode = Mode::Links(LinksState { items, sel: 0 });
    }

    fn handle_links(&mut self, key: KeyEvent) {
        let Mode::Links(state) = &self.mode else {
            return;
        };
        let (sel, len) = (state.sel, state.items.len());
        let url = |app: &Self| -> Option<String> {
            match &app.mode {
                Mode::Links(s) => s.items.get(s.sel).map(|(_, u)| u.clone()),
                _ => None,
            }
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') => self.mode = Mode::Normal,
            KeyCode::Char('j') | KeyCode::Down => {
                if let Mode::Links(s) = &mut self.mode {
                    s.sel = step(sel, 1, len);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Mode::Links(s) = &mut self.mode {
                    s.sel = step(sel, -1, len);
                }
            }
            KeyCode::Char('y') | KeyCode::Char('c') => {
                if let Some(u) = url(self) {
                    self.copy_to_clipboard(&u);
                }
            }
            KeyCode::Char('o') | KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(u) = url(self) {
                    self.open_external(&u);
                }
            }
            _ => {}
        }
    }

    /// Pipe `text` to the platform clipboard tool. Best-effort; reports via the
    /// status line.
    fn copy_to_clipboard(&mut self, text: &str) {
        use std::io::Write;
        let attempts: Vec<Vec<&str>> = if cfg!(target_os = "macos") {
            vec![vec!["pbcopy"]]
        } else if cfg!(target_os = "windows") {
            vec![vec!["clip"]]
        } else {
            vec![
                vec!["wl-copy"],
                vec!["xclip", "-selection", "clipboard"],
                vec!["xsel", "--clipboard", "--input"],
            ]
        };
        for cmd in &attempts {
            let (prog, args) = cmd.split_first().expect("non-empty command");
            let spawned = std::process::Command::new(prog)
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            if let Ok(mut child) = spawned {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
                self.status = format!("copied {}", truncate(text, 44));
                return;
            }
        }
        self.status = "no clipboard tool found (pbcopy / wl-copy / xclip / xsel)".into();
    }

    // ---- main menu (`^k`) ------------------------------------

    /// Open the weather modal, or explain why it's empty. Shared by `^w` and the
    /// menu.
    fn open_weather(&mut self) {
        if self.weather.is_some() {
            self.mode = Mode::Weather;
        } else if self
            .config
            .weather
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
        {
            self.status = "weather — still loading…".into();
        } else {
            self.status = "set `weather` in config (^e) to enable it".into();
        }
    }

    fn handle_menu(&mut self, key: KeyEvent) {
        let Mode::Menu(state) = &mut self.mode else {
            return;
        };
        let len = MenuAction::ENTRIES.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Normal,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.mode = Mode::Normal
            }
            KeyCode::Char('j') | KeyCode::Down => state.sel = step(state.sel, 1, len),
            KeyCode::Char('k') | KeyCode::Up => state.sel = step(state.sel, -1, len),
            KeyCode::Char('g') | KeyCode::Home => state.sel = 0,
            KeyCode::Char('G') | KeyCode::End => state.sel = len - 1,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char(' ') => {
                let action = MenuAction::ENTRIES[state.sel].0;
                self.mode = Mode::Normal;
                self.run_menu(action);
            }
            _ => {}
        }
    }

    fn run_menu(&mut self, action: MenuAction) {
        match action {
            MenuAction::SaveNow => {
                self.dirty = true;
                self.status = "saved".into();
            }
            MenuAction::Sync => self.sync_action(),
            MenuAction::Export => {
                let path = default_export_path();
                self.begin_input("Export data — file path", path, InputAction::ExportData);
            }
            MenuAction::Import => self.begin_input(
                "Import data — file path, or a GitHub link (replaces everything)",
                self.import_prefill(),
                InputAction::ImportData,
            ),
            MenuAction::Settings => self.open_settings = true,
            MenuAction::ImportSettings => self.import_settings_prompt(),
            MenuAction::Theme => self.open_theme(),
            MenuAction::Weather => self.open_weather(),
            MenuAction::Activity => {
                self.activity_open = !self.activity_open;
                self.status = if self.activity_open {
                    "activity panel — ^l to hide".into()
                } else {
                    String::new()
                };
            }
            MenuAction::Help => self.mode = Mode::Help,
            MenuAction::Quit => {
                self.mode = Mode::Confirm(ConfirmState {
                    prompt: "Quit voido?".into(),
                    action: ConfirmAction::Quit,
                });
            }
        }
    }

    fn do_export(&mut self, raw_path: &str) {
        let path = expand_tilde(raw_path);
        let json = match serde_json::to_string_pretty(&self.store) {
            Ok(j) => j,
            Err(e) => {
                self.toast(ToastKind::Error, format!("export failed: {e}"));
                return;
            }
        };
        match std::fs::write(&path, json) {
            Ok(()) => {
                self.push_log(format!("exported data to {}", path.display()));
                self.toast(
                    ToastKind::Success,
                    format!("Exported to {}", truncate(&path.to_string_lossy(), 48)),
                );
            }
            Err(e) => self.toast(ToastKind::Error, format!("export failed: {e}")),
        }
    }

    fn do_import(&mut self, raw_path: &str) {
        let path = expand_tilde(raw_path);
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => {
                self.toast(ToastKind::Error, format!("can't read that file: {e}"));
                return;
            }
        };
        match serde_json::from_str::<Store>(&raw) {
            Ok(store) => {
                self.mode = Mode::Confirm(ConfirmState {
                    prompt: format!(
                        "Replace ALL data with\n{}\nfrom this file?",
                        store_summary(&store)
                    ),
                    action: ConfirmAction::ImportData(Box::new(store)),
                });
            }
            Err(e) => self.toast(ToastKind::Error, format!("not valid voido data: {e}")),
        }
    }

    /// What the "Import data" prompt starts with: the sync repo, when one is set
    /// up, since that's the dataset people most often want to pull in.
    fn import_prefill(&self) -> String {
        if !self.config.sync_configured() {
            return String::new();
        }
        self.config.github_repo.clone().unwrap_or_default()
    }

    /// Paths tried when a repo is given with no file: whatever this install
    /// syncs under first, then the usual spots.
    fn data_candidates(&self) -> Vec<String> {
        let mut out = vec![self.config.sync_file()];
        for path in crate::config::DATA_CANDIDATES {
            if !out.iter().any(|c| c == path) {
                out.push((*path).to_string());
            }
        }
        out
    }

    /// Pull a dataset out of a repo on a worker thread. As with a file import,
    /// nothing is replaced until the result lands and the user confirms.
    fn spawn_data_fetch(&mut self, spec: &str) {
        if self.data_in_flight {
            self.status = "already fetching data…".into();
            return;
        }
        let Some(target) = crate::github::parse_repo_ref(spec) else {
            self.status = "invalid link — use owner/repo, or a GitHub URL".into();
            return;
        };
        let label = target.label();
        let token = self.sync_token.clone();
        let candidates = self.data_candidates();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_data(token.as_deref(), target, candidates));
        });
        self.data_rx = Some(rx);
        self.data_in_flight = true;
        self.status = format!("fetching data from {label}…");
    }

    /// Ask before replacing the store with what came back.
    fn offer_data_import(&mut self, imported: ImportedData) {
        self.mode = Mode::Confirm(ConfirmState {
            prompt: format!(
                "Replace ALL local data with\n{}\nfrom {}?",
                store_summary(&imported.store),
                imported.source
            ),
            action: ConfirmAction::ImportData(Box::new(imported.store)),
        });
    }

    // ---- fuzzy finder -----------------------------------------

    fn open_search(&mut self) {
        let mut editor = TextArea::new(vec![String::new()]);
        editor.set_placeholder_text("Fuzzy-find a project, todo or note…");
        self.mode = Mode::Search(Box::new(SearchState { editor, sel: 0 }));
    }

    /// Score every project / todo / note against the current query. Empty query
    /// lists everything (projects first, then todos, then notes, per project).
    pub fn search_results(&self) -> Vec<SearchHit> {
        let q = match &self.mode {
            Mode::Search(s) => s.query().trim().to_lowercase(),
            _ => return Vec::new(),
        };
        let mut scored: Vec<(i32, usize, SearchHit)> = Vec::new();
        for (pi, p) in self.store.projects.iter().enumerate() {
            // `hay` is matched against; `label` is what the result shows; `crumbs`
            // are the ancestor names. Tags fold into the haystack.
            let mut consider = |label: &str,
                                hay: &str,
                                target: SearchTarget,
                                crumbs: Vec<String>,
                                context: Option<String>| {
                let score = if q.is_empty() {
                    Some(0)
                } else {
                    fuzzy_score(&q, &hay.to_lowercase())
                };
                if let Some(score) = score {
                    let order = scored.len();
                    scored.push((
                        score,
                        order,
                        SearchHit {
                            project_idx: pi,
                            target,
                            label: label.to_string(),
                            crumbs,
                            context,
                        },
                    ));
                }
            };
            let with_tags = |base: &str, tags: &[String]| {
                if tags.is_empty() {
                    base.to_string()
                } else {
                    format!("{base} {}", tags.join(" "))
                }
            };
            consider(
                &p.name,
                &with_tags(&p.name, &p.tags),
                SearchTarget::Project,
                vec![],
                Some(project_search_context(p)),
            );
            for (ti, t) in p.todos.iter().enumerate() {
                let (sd, st) = t.subtask_progress();
                let todo_ctx = (st > 0).then(|| format!("↳ {sd}/{st}"));
                consider(
                    &t.title,
                    &with_tags(&t.title, &t.tags),
                    SearchTarget::Todo(ti),
                    vec![p.name.clone()],
                    todo_ctx,
                );
                if !q.is_empty() {
                    for (si, s) in t.subtasks.iter().enumerate() {
                        consider(
                            &s.title,
                            &with_tags(&s.title, &s.tags),
                            SearchTarget::Subtask { todo: ti, sub: si },
                            vec![p.name.clone(), t.title.clone()],
                            None,
                        );
                    }
                }
            }
            for (ni, n) in p.notes.iter().enumerate() {
                consider(
                    &n.text,
                    &n.text,
                    SearchTarget::Note(ni),
                    vec![p.name.clone()],
                    None,
                );
            }
        }
        // Highest score first; stable on insertion order for ties / empty query.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(40);
        scored.into_iter().map(|(_, _, h)| h).collect()
    }

    fn move_search_sel(&mut self, delta: i32) {
        let len = self.search_results().len();
        if let Mode::Search(s) = &mut self.mode {
            s.sel = step(s.sel, delta, len);
        }
    }

    fn commit_search(&mut self) {
        let sel = match &self.mode {
            Mode::Search(s) => s.sel,
            _ => return,
        };
        let Some(hit) = self.search_results().get(sel).cloned() else {
            self.mode = Mode::Normal;
            return;
        };
        self.project_idx = hit.project_idx;
        self.reset_content_idx();
        match hit.target {
            SearchTarget::Project => self.focus = Focus::Projects,
            SearchTarget::Todo(i) => {
                self.tab = Tab::Todos;
                self.todo_idx = i;
                self.subtask_idx = 0;
                self.focus = Focus::Content;
            }
            SearchTarget::Subtask { todo, sub } => {
                self.tab = Tab::Todos;
                self.todo_idx = todo;
                self.subtask_idx = sub;
                self.focus = Focus::Detail;
            }
            SearchTarget::Note(i) => {
                self.tab = Tab::Notes;
                self.note_idx = i;
                self.note_scroll = 0;
                self.focus = Focus::Content;
            }
        }
        self.mode = Mode::Normal;
        self.status = format!("jumped to {}", truncate(&hit.label, 40));
    }

    fn handle_search(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => self.commit_search(),
            KeyCode::Down | KeyCode::Tab => self.move_search_sel(1),
            KeyCode::Up | KeyCode::BackTab => self.move_search_sel(-1),
            KeyCode::Char('n') if ctrl => self.move_search_sel(1),
            KeyCode::Char('p') if ctrl => self.move_search_sel(-1),
            _ => {
                if let Mode::Search(s) = &mut self.mode {
                    s.editor.input(key);
                    s.sel = 0;
                }
            }
        }
    }

    // ---- theme picker -------------------------------------------

    /// Drop the memoised note-body render so it re-runs with the new palette.
    fn clear_render_cache(&self) {
        *self.md_cache.borrow_mut() = None;
    }

    fn open_theme(&mut self) {
        let idx = crate::theme::current_index();
        self.mode = Mode::Theme(ThemeState { idx, saved: idx });
    }

    fn preview_theme(&mut self, idx: usize) {
        crate::theme::set_index(idx);
        self.clear_render_cache();
    }

    fn handle_theme(&mut self, key: KeyEvent) {
        let Mode::Theme(state) = &self.mode else {
            return;
        };
        let (idx, saved, n) = (state.idx, state.saved, crate::theme::registry().len());
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let next = (idx + 1) % n;
                if let Mode::Theme(s) = &mut self.mode {
                    s.idx = next;
                }
                self.preview_theme(next);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let prev = (idx + n - 1) % n;
                if let Mode::Theme(s) = &mut self.mode {
                    s.idx = prev;
                }
                self.preview_theme(prev);
            }
            KeyCode::Enter | KeyCode::Char('l') => {
                self.preview_theme(idx);
                self.config.theme = Some(crate::theme::current().slug.clone());
                let _ = self.config.save();
                self.mode = Mode::Normal;
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.preview_theme(saved);
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    // ---- deadlines tab ---------------------------------------------

    fn handle_timeline_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_sel(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('h') | KeyCode::Left => self.focus = Focus::Projects,
            KeyCode::Char('f') => {
                self.deadline_filter = self.deadline_filter.next();
                self.timeline_idx = 0;
                self.status = format!("filter: {}", self.deadline_filter.label());
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
                    } else if let Some(mi) = entry.milestone_idx
                        && let Some(m) = self
                            .current_project_mut()
                            .and_then(|p| p.milestones.get_mut(mi))
                    {
                        m.done = !m.done;
                        self.dirty = true;
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
                    } else if let Some(mi) = entry.milestone_idx
                        && let Some(m) = self.current_project().and_then(|p| p.milestones.get(mi))
                    {
                        let pre = milestone_edit_string(m);
                        self.begin_input("Edit milestone", pre, InputAction::EditMilestone(mi));
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
                    } else if let Some(ti) = entry.todo_idx
                        && let Some(t) = self.current_project().and_then(|p| p.todos.get(ti))
                    {
                        let prompt = format!("Delete todo \"{}\"?", t.title);
                        self.mode = Mode::Confirm(ConfirmState {
                            prompt,
                            action: ConfirmAction::DeleteTodo(ti),
                        });
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
                    self.commit_input(*input);
                }
            }
            _ => {
                if let Mode::Input(input) = &mut self.mode {
                    // Single-line field: swallow newlines, accept everything else
                    // (cursor movement, word-delete, paste) via tui-textarea.
                    input.editor.input(key);
                }
            }
        }
    }

    fn handle_confirm(&mut self, key: KeyEvent) {
        // A quit prompt also takes a second `q` as "yes", so the muscle-memory
        // `qq` still works while a stray single `q` doesn't.
        let quit = matches!(
            self.mode,
            Mode::Confirm(ConfirmState {
                action: ConfirmAction::Quit,
                ..
            })
        );
        let accept = matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter)
            || (quit && matches!(key.code, KeyCode::Char('q' | 'Q')));

        if accept {
            if let Mode::Confirm(c) = std::mem::replace(&mut self.mode, Mode::Normal) {
                self.perform_confirm(c.action);
            }
        } else if matches!(key.code, KeyCode::Char('n' | 'N') | KeyCode::Esc) {
            self.mode = Mode::Normal;
            self.status = "cancelled".into();
        }
    }

    fn commit_input(&mut self, input: InputState) {
        let value = input.value();
        match input.action {
            InputAction::AddProject => {
                let (name, tags) = parse_tagged_name(&value);
                if name.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                let mut project = Project::new(&name);
                project.tags = tags;
                self.store.projects.push(project);
                self.project_idx = self.store.projects.len() - 1;
                self.focus = Focus::Content;
                self.tab = Tab::Todos;
                self.reset_content_idx();
                self.dirty = true;
                self.status = format!("added project: {name}");
            }
            InputAction::RenameProject => {
                let (name, tags) = parse_tagged_name(&value);
                if name.is_empty() {
                    return;
                }
                if let Some(p) = self.current_project_mut() {
                    p.name = name.clone();
                    p.tags = tags;
                }
                self.dirty = true;
                self.status = format!("renamed to: {name}");
            }
            InputAction::EditDescription => {
                let text = value.trim().to_string();
                if let Some(p) = self.current_project_mut() {
                    p.description = text;
                    self.dirty = true;
                    self.status = "description updated".into();
                }
            }
            InputAction::AddNote => {
                let text = value.trim().to_string();
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
                let text = value.trim().to_string();
                if text.is_empty() {
                    return;
                }
                if let Some(n) = self.current_project_mut().and_then(|p| p.notes.get_mut(i)) {
                    n.text = text;
                    self.dirty = true;
                    self.status = "note updated".into();
                }
            }
            InputAction::AddAttachment(target) => {
                let raw = value.trim();
                if raw.is_empty() {
                    self.status = "nothing added".into();
                } else {
                    let (val, label) = match raw.split_once('|') {
                        Some((v, l)) => (v.trim().to_string(), l.trim().to_string()),
                        None => (raw.to_string(), String::new()),
                    };
                    if let Some(list) = self.attachments_at_mut(target) {
                        list.push(Attachment::new(val, label));
                        self.dirty = true;
                        self.status = "attachment added".into();
                    }
                }
                let sel = self
                    .attachments_at(target)
                    .map(|list| list.len().saturating_sub(1))
                    .unwrap_or(0);
                self.mode = Mode::Attach(AttachState { target, sel });
            }
            InputAction::AddTag(target) => {
                let new = crate::model::normalize_tags(value.split_whitespace());
                if new.is_empty() {
                    self.status = "no tags added".into();
                } else if let Some(list) = self.tags_at_mut(target) {
                    let before = list.len();
                    for t in new {
                        if !list.contains(&t) {
                            list.push(t);
                        }
                    }
                    let added = list.len() - before;
                    self.dirty = true;
                    self.status = match added {
                        0 => "already tagged".into(),
                        1 => "tag added".into(),
                        n => format!("{n} tags added"),
                    };
                }
                let sel = self
                    .tags_at(target)
                    .map(|list| list.len().saturating_sub(1))
                    .unwrap_or(0);
                self.mode = Mode::Tags(TagState { target, sel });
            }
            InputAction::AddTodo => {
                let (title, priority, due, tags) = parse_todo_input(&value);
                if title.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                if let Some(p) = self.current_project_mut() {
                    let mut todo = Todo::new(title);
                    todo.priority = priority;
                    todo.due = due;
                    todo.tags = tags;
                    p.todos.push(todo);
                }
                let len = self.current_project().map(|p| p.todos.len()).unwrap_or(0);
                self.todo_idx = len.saturating_sub(1);
                self.subtask_idx = 0;
                self.dirty = true;
                self.status = "todo added".into();
            }
            InputAction::EditTodo(i) => {
                let (title, priority, due, tags) = parse_todo_input(&value);
                if title.is_empty() {
                    return;
                }
                if let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i)) {
                    t.title = title;
                    t.priority = priority;
                    t.due = due;
                    t.tags = tags;
                    self.dirty = true;
                    self.status = "todo updated".into();
                }
            }
            InputAction::AddSubtask => {
                let (title, priority, tags) = parse_priority_input(&value);
                if title.is_empty() {
                    self.status = "nothing added".into();
                    return;
                }
                let mut len = 0;
                if let Some(t) = self.current_todo_mut() {
                    let mut sub = Subtask::new(title, false);
                    sub.priority = priority;
                    sub.tags = tags;
                    t.subtasks.push(sub);
                    len = t.subtasks.len();
                }
                self.subtask_idx = len.saturating_sub(1);
                self.dirty = true;
                self.status = "subtask added".into();
                self.sync_parent_done();
            }
            InputAction::EditSubtask(i) => {
                let (title, priority, tags) = parse_priority_input(&value);
                if title.is_empty() {
                    return;
                }
                if let Some(s) = self.current_todo_mut().and_then(|t| t.subtasks.get_mut(i)) {
                    s.title = title;
                    s.priority = priority;
                    s.tags = tags;
                    self.dirty = true;
                    self.status = "subtask updated".into();
                }
            }
            InputAction::AddMilestone => {
                let today = Local::now().date_naive();
                match parse_milestone_input(&value, today) {
                    Some((title, date)) => {
                        if let Some(p) = self.current_project_mut() {
                            p.milestones.push(Milestone {
                                title,
                                date,
                                done: false,
                            });
                        }
                        self.dirty = true;
                        self.status = "milestone added".into();
                    }
                    None => self.status = "nothing added".into(),
                }
            }
            InputAction::EditMilestone(i) => {
                let today = Local::now().date_naive();
                if let Some((title, date)) = parse_milestone_input(&value, today)
                    && let Some(m) = self
                        .current_project_mut()
                        .and_then(|p| p.milestones.get_mut(i))
                {
                    m.title = title;
                    m.date = date;
                    self.dirty = true;
                    self.status = "milestone updated".into();
                }
            }
            InputAction::AddMeeting => {
                let today = Local::now().date_naive();
                match parse_meeting_input(&value, today) {
                    Some((title, date, time, attendees)) => {
                        if let Some(p) = self.current_project_mut() {
                            let mut m = Meeting::new(title, date);
                            m.time = time;
                            m.attendees = attendees;
                            p.meetings.push(m);
                            self.meeting_idx = p.meetings.len() - 1;
                        }
                        self.dirty = true;
                        self.status = "meeting added".into();
                    }
                    None => self.status = "nothing added".into(),
                }
            }
            InputAction::EditMeeting(i) => {
                let today = Local::now().date_naive();
                if let Some((title, date, time, attendees)) = parse_meeting_input(&value, today)
                    && let Some(m) = self
                        .current_project_mut()
                        .and_then(|p| p.meetings.get_mut(i))
                {
                    m.title = title;
                    m.date = date;
                    m.time = time;
                    m.attendees = attendees;
                    self.dirty = true;
                    self.status = "meeting updated".into();
                }
            }
            InputAction::RescheduleMeeting(i) => {
                let today = Local::now().date_naive();
                let (date, time) = parse_meeting_when(&value, today);
                if let Some(m) = self
                    .current_project_mut()
                    .and_then(|p| p.meetings.get_mut(i))
                {
                    m.date = date;
                    m.time = time;
                    self.dirty = true;
                    self.status = format!("moved to {}", date.format("%Y-%m-%d"));
                }
            }
            InputAction::RescheduleTodo(i) => {
                let today = Local::now().date_naive();
                if let Some((_, date)) = parse_milestone_input(&value, today)
                    && let Some(t) = self.current_project_mut().and_then(|p| p.todos.get_mut(i))
                {
                    t.due = Some(date);
                    self.dirty = true;
                    self.status = format!("rescheduled to {}", date.format("%Y-%m-%d"));
                }
            }
            InputAction::RescheduleMilestone(i) => {
                let today = Local::now().date_naive();
                if let Some((_, date)) = parse_milestone_input(&value, today)
                    && let Some(m) = self
                        .current_project_mut()
                        .and_then(|p| p.milestones.get_mut(i))
                {
                    m.date = date;
                    self.dirty = true;
                    self.status = format!("rescheduled to {}", date.format("%Y-%m-%d"));
                }
            }
            InputAction::ExportData => {
                if value.trim().is_empty() {
                    self.status = "export cancelled".into();
                } else {
                    self.do_export(value.trim());
                }
            }
            InputAction::ImportData => {
                let v = value.trim();
                if v.is_empty() {
                    self.status = "import cancelled".into();
                } else if looks_like_github(v) {
                    self.spawn_data_fetch(v);
                } else {
                    self.do_import(v);
                }
            }
            InputAction::LinkRepo => {
                let value = value.trim().to_string();
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
                        self.status = format!("linked to {owner}/{repo}");
                        self.fetch_github(&owner, &repo);
                    }
                } else {
                    self.status = "invalid repo format — use owner/repo".into();
                }
            }
            InputAction::SyncRepo => {
                let v = value.trim().trim_end_matches('/');
                let v = if v.is_empty() { "voido-data" } else { v };
                // Accept "owner/repo" or a bare name (owner resolved on push).
                let looks_ok = crate::github::parse_repo_string(v).is_some()
                    || !v.contains('/') && !v.contains(char::is_whitespace);
                if !looks_ok {
                    self.status = "invalid repo — use a name or owner/repo".into();
                    return;
                }
                self.pending_sync_repo = Some(v.to_string());
                if self.sync_token.is_some() {
                    self.finish_sync_setup();
                } else {
                    self.begin_input(
                        "GitHub sync — token  (classic: repo scope)",
                        self.config.github_token.clone().unwrap_or_default(),
                        InputAction::SyncToken,
                    );
                }
            }
            InputAction::ImportSettings => {
                let v = value.trim();
                if v.is_empty() {
                    self.status = "settings import cancelled".into();
                } else {
                    self.spawn_settings_fetch(v);
                }
            }
            InputAction::SyncToken => {
                let token = value.trim().to_string();
                if token.is_empty() {
                    self.pending_sync_repo = None;
                    self.status = "sync setup cancelled — no token given".into();
                    return;
                }
                self.config.github_token = Some(token.clone());
                self.sync_token = Some(token.clone());
                self.gh_client = GitHubClient::new(Some(token));
                self.sync_sha = None;
                self.finish_sync_setup();
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
                // Removing the last open subtask can complete the parent.
                self.sync_parent_done();
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
            ConfirmAction::DeleteMeeting(i) => {
                if let Some(p) = self.current_project_mut()
                    && i < p.meetings.len()
                {
                    p.meetings.remove(i);
                }
                self.meeting_idx = self.meeting_idx.saturating_sub(1);
                self.meeting_note_scroll = 0;
                self.dirty = true;
                self.status = "meeting deleted".into();
            }
            ConfirmAction::Quit => self.should_quit = true,
            ConfirmAction::ImportSettings(config) => self.adopt_settings(*config),
            ConfirmAction::ImportData(store) => {
                let mut store = *store;
                let n = store.projects.len();
                heal_store(&mut store);
                self.store = store;
                self.project_idx = 0;
                self.todo_idx = 0;
                self.subtask_idx = 0;
                self.note_idx = 0;
                self.reset_content_idx();
                self.focus = Focus::Projects;
                self.tab = Tab::Todos;
                self.dirty = true;
                self.push_log(format!("imported {n} project(s)"));
                self.toast(
                    ToastKind::Success,
                    format!("Imported {n} project{}", if n == 1 { "" } else { "s" }),
                );
            }
        }
    }

    // ---- helpers --------------------------------------------------

    fn begin_input(&mut self, title: &str, value: String, action: InputAction) {
        let mut editor = TextArea::new(vec![value]);
        editor.move_cursor(CursorMove::End);
        self.mode = Mode::Input(Box::new(InputState {
            title: title.to_string(),
            editor,
            action,
        }));
    }

    fn reset_content_idx(&mut self) {
        self.todo_idx = 0;
        self.subtask_idx = 0;
        self.note_idx = 0;
        self.note_scroll = 0;
        self.todo_note_open = false;
        self.sub_note_open = false;
        self.sub_note_focus = false;
        self.todo_note_scroll = 0;
        self.sub_note_scroll = 0;
        self.timeline_idx = 0;
        self.meeting_idx = 0;
        self.meeting_note_scroll = 0;
    }

    fn cycle_tab(&mut self, forward: bool) {
        let cur = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        let next = if forward {
            (cur + 1) % Tab::ALL.len()
        } else {
            (cur + Tab::ALL.len() - 1) % Tab::ALL.len()
        };
        self.goto_tab(Tab::ALL[next]);
    }

    /// Switch the content view and move focus into it.
    fn goto_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if !self.store.projects.is_empty() {
            self.focus = Focus::Content;
        }
    }

    /// Move the project selection without changing which pane or view is focused.
    fn select_project(&mut self, delta: i32) {
        let new = step(self.project_idx, delta, self.store.projects.len());
        if new != self.project_idx {
            self.project_idx = new;
            self.reset_content_idx();
        }
    }

    /// Prompt to link (or, with an empty answer, unlink) a GitHub repo for the
    /// selected project.
    fn link_repo_prompt(&mut self) {
        if let Some(p) = self.current_project() {
            let pre = p.repo.clone().unwrap_or_default();
            self.begin_input("Link GitHub repo (owner/repo)", pre, InputAction::LinkRepo);
        }
    }

    /// Open the GitHub activity view for the selected project's linked repo.
    fn show_github(&mut self) {
        match self.current_project().and_then(|p| p.repo.clone()) {
            Some(repo) => match crate::github::parse_repo_string(&repo) {
                Some((owner, name)) => self.fetch_github(&owner, &name),
                None => self.status = "linked repo is malformed — re-link with ^g".into(),
            },
            None => self.status = "no repo linked — press ^g to link one".into(),
        }
    }

    fn fetch_github(&mut self, owner: &str, repo: &str) {
        if self.gh_loading {
            return;
        }
        self.gh_loading = true;
        self.status = format!("fetching {owner}/{repo}…");

        let (tx, rx) = mpsc::channel();
        let client = self.gh_client.clone();
        let owner = owner.to_string();
        let repo = repo.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(client.fetch_repo_info(&owner, &repo));
        });
        self.gh_rx = Some(rx);
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
                    self.todo_note_scroll = 0;
                    self.sub_note_scroll = 0;
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
                Tab::Meetings => {
                    let len = self
                        .current_project()
                        .map(|p| p.meetings.len())
                        .unwrap_or(0);
                    self.meeting_idx = step(self.meeting_idx, delta, len);
                    self.meeting_note_scroll = 0;
                }
            },
            Focus::Detail => match self.tab {
                Tab::Notes => {
                    self.note_scroll = if delta < 0 {
                        self.note_scroll
                            .saturating_sub((-delta).min(i32::from(u16::MAX)) as u16)
                    } else {
                        self.note_scroll
                            .saturating_add(delta.min(i32::from(u16::MAX)) as u16)
                    };
                }
                Tab::Meetings => self.scroll_meeting_note(delta),
                _ => {
                    let len = self.current_todo().map(|t| t.subtasks.len()).unwrap_or(0);
                    self.subtask_idx = step(self.subtask_idx, delta, len);
                    self.sub_note_scroll = 0;
                }
            },
        }
    }
}

/// Which tab (if any) sits under `x` on the content pane's top border. Mirrors
/// the strip drawn in `ui::content_block`: one ` {title} ` cell per tab, starting
/// one column in from the rounded corner.
fn tab_at_x(content: Rect, x: u16) -> Option<Tab> {
    let mut cx = content.x + 1;
    for t in Tab::ALL {
        let w = t.strip_label(content.width).chars().count() as u16 + 2;
        if x >= cx && x < cx + w {
            return Some(t);
        }
        cx += w;
    }
    None
}

/// Map a click's y-coordinate to a list row inside `pane`. Row 0 sits just below
/// the pane's top border. Returns `None` for clicks on the border or past the
/// last item (so the click still focuses the pane without moving the selection).
fn row_at(pane: Rect, y: u16, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let first = pane.y.saturating_add(1);
    if y < first {
        return None;
    }
    let idx = (y - first) as usize;
    (idx < len).then_some(idx)
}

/// Subsequence fuzzy match. `needle` and `haystack` must already be lowercased.
/// `None` when `needle` isn't a subsequence of `haystack`; otherwise a score
/// where higher is better (contiguous runs and early matches score more).
fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = haystack.chars().collect();
    let mut hi = 0usize;
    let mut score = 0i32;
    let mut prev_match = false;
    let mut first: Option<usize> = None;

    for nc in needle.chars() {
        let mut found = false;
        while hi < hay.len() {
            let hc = hay[hi];
            hi += 1;
            if hc == nc {
                found = true;
                first.get_or_insert(hi - 1);
                score += 1;
                if prev_match {
                    score += 3;
                }
                prev_match = true;
                break;
            }
            prev_match = false;
        }
        if !found {
            return None;
        }
    }

    if let Some(f) = first {
        score += 10 - f.min(10) as i32;
    }
    score -= haystack.chars().count() as i32 / 20;
    Some(score)
}

/// Compare two values in the requested direction.
fn cmp_val<T: Ord>(a: T, b: T, reverse: bool) -> Ordering {
    if reverse { b.cmp(&a) } else { a.cmp(&b) }
}

/// Compare two values that an item may not have (a due date, a tag). Present
/// values order as asked; anything missing sits at the bottom either way, so
/// reversing doesn't float the blanks to the top.
fn cmp_opt<T: Ord>(a: Option<T>, b: Option<T>, reverse: bool) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => cmp_val(x, y, reverse),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Case-folded title, so `Name` orders the way a reader reads.
fn name_key(s: &str) -> String {
    s.trim().to_lowercase()
}

/// The tag `Tag` groups an item under — its first, since that's the one the
/// lists show first.
fn tag_key(tags: &[String]) -> Option<String> {
    tags.first().map(|t| t.to_lowercase())
}

/// How far along a todo is, 0-100: its subtask completion, or its own done flag
/// when it has no subtasks.
fn todo_progress(t: &Todo) -> u32 {
    let (done, total) = t.subtask_progress();
    if total == 0 {
        if t.done { 100 } else { 0 }
    } else {
        done as u32 * 100 / total as u32
    }
}

/// How far along a project is, 0-100: the mean progress of its todos.
fn project_progress(p: &Project) -> u32 {
    if p.todos.is_empty() {
        return 0;
    }
    p.todos.iter().map(todo_progress).sum::<u32>() / p.todos.len() as u32
}

/// The soonest date a project owes something: its next open milestone, or the
/// earliest due date among its open todos.
fn project_deadline(p: &Project) -> Option<NaiveDate> {
    p.milestones
        .iter()
        .filter(|m| !m.done)
        .map(|m| m.date)
        .chain(p.todos.iter().filter(|t| !t.done).filter_map(|t| t.due))
        .min()
}

/// `Progress`, `Created`, `Open`, `Length` and `Pinned` read best largest-first,
/// so their natural direction is the reverse of an ascending compare.
fn cmp_desc<T: Ord>(a: T, b: T, reverse: bool) -> Ordering {
    cmp_val(a, b, !reverse)
}

fn cmp_project(a: &Project, b: &Project, key: SortKey, reverse: bool) -> Ordering {
    match key {
        SortKey::Name => cmp_val(name_key(&a.name), name_key(&b.name), reverse),
        SortKey::Deadline => cmp_opt(project_deadline(a), project_deadline(b), reverse),
        SortKey::Open => cmp_desc(a.open_todos(), b.open_todos(), reverse),
        SortKey::Progress => cmp_desc(project_progress(a), project_progress(b), reverse),
        SortKey::Created => cmp_desc(a.created, b.created, reverse),
        SortKey::Tag => cmp_opt(tag_key(&a.tags), tag_key(&b.tags), reverse),
        _ => Ordering::Equal,
    }
}

fn cmp_todo(a: &Todo, b: &Todo, key: SortKey, reverse: bool) -> Ordering {
    match key {
        SortKey::Priority => cmp_val(a.priority.rank(), b.priority.rank(), reverse),
        SortKey::Due => cmp_opt(a.due, b.due, reverse),
        SortKey::Name => cmp_val(name_key(&a.title), name_key(&b.title), reverse),
        SortKey::Status => cmp_val(a.done, b.done, reverse),
        SortKey::Progress => cmp_desc(todo_progress(a), todo_progress(b), reverse),
        SortKey::Tag => cmp_opt(tag_key(&a.tags), tag_key(&b.tags), reverse),
        _ => Ordering::Equal,
    }
}

fn cmp_subtask(a: &Subtask, b: &Subtask, key: SortKey, reverse: bool) -> Ordering {
    match key {
        SortKey::Priority => cmp_val(a.priority.rank(), b.priority.rank(), reverse),
        SortKey::Name => cmp_val(name_key(&a.title), name_key(&b.title), reverse),
        SortKey::Status => cmp_val(a.done, b.done, reverse),
        SortKey::Tag => cmp_opt(tag_key(&a.tags), tag_key(&b.tags), reverse),
        _ => Ordering::Equal,
    }
}

fn cmp_meeting(a: &Meeting, b: &Meeting, key: SortKey, reverse: bool) -> Ordering {
    match key {
        // Same day, earlier slot first — an unscheduled time sorts after the
        // timed meetings on that day.
        SortKey::Date => cmp_val(
            (a.date, a.time.is_none(), a.time.clone()),
            (b.date, b.time.is_none(), b.time.clone()),
            reverse,
        ),
        SortKey::Name => cmp_val(name_key(&a.title), name_key(&b.title), reverse),
        SortKey::Held => cmp_val(a.held, b.held, reverse),
        SortKey::Attendees => cmp_desc(a.attendees.len(), b.attendees.len(), reverse),
        _ => Ordering::Equal,
    }
}

fn cmp_note(a: &Note, b: &Note, key: SortKey, reverse: bool) -> Ordering {
    match key {
        SortKey::Pinned => cmp_desc(a.pinned, b.pinned, reverse),
        SortKey::Name => cmp_val(name_key(&a.text), name_key(&b.text), reverse),
        SortKey::Length => cmp_desc(a.body.trim().len(), b.body.trim().len(), reverse),
        _ => Ordering::Equal,
    }
}

/// Append an activity line, trimming the oldest once the buffer is full.
fn push_capped(buf: &mut Vec<LogEntry>, text: String) {
    const CAP: usize = 400;
    buf.push(LogEntry {
        at: Local::now(),
        text,
    });
    if buf.len() > CAP {
        buf.remove(0);
    }
}

fn step(idx: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = len as i32 - 1;
    (idx as i32 + delta).clamp(0, max) as usize
}

/// Normalise a freshly loaded store: keep every todo's `done` in lockstep with
/// its subtasks and clean any hand-edited tags. Run on startup and after import.
fn heal_store(store: &mut Store) {
    for p in &mut store.projects {
        p.tags = crate::model::normalize_tags(p.tags.drain(..));
        for t in &mut p.todos {
            t.recompute_done();
            t.tags = crate::model::normalize_tags(t.tags.drain(..));
            for s in &mut t.subtasks {
                s.tags = crate::model::normalize_tags(s.tags.drain(..));
            }
        }
    }
}

/// Expand a leading `~` to the home directory.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    std::path::PathBuf::from(path)
}

/// `~/voido-export-YYYY-MM-DD.json`, or a bare filename if there's no home dir.
fn default_export_path() -> String {
    let name = format!("voido-export-{}.json", Local::now().format("%Y-%m-%d"));
    match dirs::home_dir() {
        Some(home) => home.join(name).to_string_lossy().into_owned(),
        None => name,
    }
}

/// A `#tag` token: `#` followed by a letter (so issue refs like `#42` stay in
/// the title). The tag body is cleaned later by `model::normalize_tag`.
fn is_tag_token(tok: &str) -> bool {
    let mut c = tok.chars();
    c.next() == Some('#') && c.next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// Parse `buy milk !3 @2026-09-01 #chores` into (title, priority, due, tags).
fn parse_todo_input(raw: &str) -> (String, Priority, Option<NaiveDate>, Vec<String>) {
    let mut priority = Priority::Medium;
    let mut due = None;
    let mut tags: Vec<&str> = Vec::new();
    let mut words = Vec::new();

    for tok in raw.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('@')
            && let Ok(d) = NaiveDate::parse_from_str(rest, "%Y-%m-%d")
        {
            due = Some(d);
            continue;
        }
        if is_tag_token(tok) {
            tags.push(tok);
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

    (
        words.join(" ").trim().to_string(),
        priority,
        due,
        crate::model::normalize_tags(tags),
    )
}

/// Prompt text for a new meeting — the one place the input syntax is spelled out.
const MEETING_PROMPT: &str = "New meeting   (@YYYY-MM-DD · 14:30 · +person)";

/// Pull the date and start time out of a meeting input: `@YYYY-MM-DD` sets the
/// date (`fallback` when absent), a bare `14:30` (or `9:00`) sets the time.
fn parse_meeting_when(raw: &str, fallback: NaiveDate) -> (NaiveDate, Option<String>) {
    let mut date = fallback;
    let mut time = None;
    for tok in raw.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('@')
            && let Ok(d) = NaiveDate::parse_from_str(rest, "%Y-%m-%d")
        {
            date = d;
        } else if let Some(t) = parse_clock(tok) {
            time = Some(t);
        }
    }
    (date, time)
}

/// `14:30` / `9:05` -> `Some("14:30")`. Anything else -> `None`.
fn parse_clock(tok: &str) -> Option<String> {
    let (h, m) = tok.split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

/// Parse `Design review @2026-09-05 14:30 +ana +you` into
/// (title, date, time, attendees). `@date` and a bare `HH:MM` set when it is;
/// `+name` tokens are attendees; everything else is the title. `None` when no
/// title is left.
fn parse_meeting_input(
    raw: &str,
    fallback: NaiveDate,
) -> Option<(String, NaiveDate, Option<String>, Vec<String>)> {
    let (date, time) = parse_meeting_when(raw, fallback);
    let mut attendees: Vec<String> = Vec::new();
    let mut words = Vec::new();

    for tok in raw.split_whitespace() {
        if tok.starts_with('@') && tok.len() > 1 {
            continue;
        }
        if parse_clock(tok).is_some() {
            continue;
        }
        if let Some(name) = tok.strip_prefix('+') {
            let name = name.trim();
            // Same person twice (however they were capitalised) counts once.
            if !name.is_empty() && !attendees.iter().any(|a| a.eq_ignore_ascii_case(name)) {
                attendees.push(name.to_string());
            }
            continue;
        }
        words.push(tok);
    }

    let title = words.join(" ").trim().to_string();
    (!title.is_empty()).then_some((title, date, time, attendees))
}

/// The editable form of a meeting — what `e` puts in the input.
fn meeting_edit_string(m: &Meeting) -> String {
    let mut out = format!("{} @{}", m.title, m.date.format("%Y-%m-%d"));
    if let Some(t) = &m.time {
        out.push(' ');
        out.push_str(t);
    }
    for a in &m.attendees {
        out.push_str(" +");
        out.push_str(a);
    }
    out
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

/// Parse `refactor auth !3 #api` into (title, priority, tags). `!1`/`!2`/`!3`
/// (also `!low`/`!med`/`!high`, `!!`) set the priority; `#tag` tokens are tags;
/// both drop out of the title.
fn parse_priority_input(raw: &str) -> (String, Priority, Vec<String>) {
    let mut priority = Priority::Medium;
    let mut tags: Vec<&str> = Vec::new();
    let mut words = Vec::new();
    for tok in raw.split_whitespace() {
        if is_tag_token(tok) {
            tags.push(tok);
            continue;
        }
        match tok {
            "!1" | "!low" => priority = Priority::Low,
            "!2" | "!med" | "!medium" => priority = Priority::Medium,
            "!3" | "!!" | "!high" => priority = Priority::High,
            _ => words.push(tok),
        }
    }
    (
        words.join(" ").trim().to_string(),
        priority,
        crate::model::normalize_tags(tags),
    )
}

/// Parse a project name line, pulling out `#tag` tokens: `Website #web #api`.
fn parse_tagged_name(raw: &str) -> (String, Vec<String>) {
    let mut tags: Vec<&str> = Vec::new();
    let mut words = Vec::new();
    for tok in raw.split_whitespace() {
        if is_tag_token(tok) {
            tags.push(tok);
        } else {
            words.push(tok);
        }
    }
    (
        words.join(" ").trim().to_string(),
        crate::model::normalize_tags(tags),
    )
}

/// `!1` for low, `!3` for high — the suffix `parse_priority_input` understands.
fn priority_suffix(p: Priority) -> &'static str {
    match p {
        Priority::Low => " !1",
        Priority::High => " !3",
        Priority::Medium => "",
    }
}

/// ` #tag #tag2` — the suffix the input parsers round-trip.
fn tags_suffix(tags: &[String]) -> String {
    tags.iter().map(|t| format!(" #{t}")).collect()
}

fn subtask_edit_string(s: &Subtask) -> String {
    format!(
        "{}{}{}",
        s.title,
        priority_suffix(s.priority),
        tags_suffix(&s.tags)
    )
}

fn todo_edit_string(t: &Todo) -> String {
    let mut s = t.title.clone();
    s.push_str(priority_suffix(t.priority));
    if let Some(d) = t.due {
        s.push_str(&format!(" @{}", d.format("%Y-%m-%d")));
    }
    s.push_str(&tags_suffix(&t.tags));
    s
}

fn project_edit_string(p: &Project) -> String {
    format!("{}{}", p.name, tags_suffix(&p.tags))
}

fn milestone_edit_string(m: &Milestone) -> String {
    format!("{} @{}", m.title, m.date.format("%Y-%m-%d"))
}

/// Worker-thread body for a data import: read the dataset out of the repo and
/// parse it. Nothing is replaced here; the caller confirms first.
fn fetch_data(
    token: Option<&str>,
    target: RepoRef,
    candidates: Vec<String>,
) -> Result<ImportedData, String> {
    let (store, source) = fetch_repo_file(token, target, candidates, "data", |text| {
        serde_json::from_str::<Store>(text).map_err(|e| format!("not valid voido data: {e}"))
    })?;
    Ok(ImportedData { store, source })
}

/// Does an import target name a GitHub repo rather than a local file? URLs and
/// `owner/repo` forms are remote; anything that exists on disk, or that reads as
/// a path, stays local — so a relative `notes/data.json` still means the file.
fn looks_like_github(spec: &str) -> bool {
    let s = spec.trim();
    const URLS: &[&str] = &[
        "https://",
        "http://",
        "github.com/",
        "raw.githubusercontent.com/",
        "git@github.com:",
    ];
    if URLS.iter().any(|p| s.starts_with(p)) {
        return true;
    }
    // Absolute, home-relative, dot-relative or a Windows drive: a path.
    if s.starts_with(['/', '~', '.', '\\']) || s.contains(':') {
        return false;
    }
    // A file that's actually there always wins over a repo of the same shape.
    if expand_tilde(s).exists() {
        return false;
    }
    crate::github::parse_repo_ref(s).is_some()
}

/// "4 projects · 27 todos · 9 notes" — what an import is about to bring in.
fn store_summary(store: &Store) -> String {
    let projects = store.projects.len();
    let todos: usize = store.projects.iter().map(|p| p.todos.len()).sum();
    let notes: usize = store.projects.iter().map(|p| p.notes.len()).sum();
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    format!(
        "{} · {} · {}",
        plural(projects, "project"),
        plural(todos, "todo"),
        plural(notes, "note")
    )
}

/// Worker-thread body for a settings import: read the named file out of the
/// repo — or, when no path was given, the first of the usual locations that
/// exists — and parse it. Nothing is written here; the caller confirms first.
fn fetch_settings(token: Option<&str>, target: RepoRef) -> Result<ImportedSettings, String> {
    let candidates = crate::config::SETTINGS_CANDIDATES
        .iter()
        .map(|p| (*p).to_string())
        .collect();
    let (config, source) = fetch_repo_file(token, target, candidates, "settings", Config::parse)?;
    Ok(ImportedSettings { config, source })
}

/// Read the file `target` names out of the repo, or — when it names no file —
/// the first of `fallbacks` that exists, and parse it with `parse`. Returns the
/// parsed value and the `owner/repo/path@ref` it actually came from.
fn fetch_repo_file<T>(
    token: Option<&str>,
    target: RepoRef,
    fallbacks: Vec<String>,
    what: &str,
    parse: impl Fn(&str) -> Result<T, String>,
) -> Result<(T, String), String> {
    let candidates = match &target.path {
        Some(path) => vec![path.clone()],
        None => fallbacks,
    };

    for path in &candidates {
        let fetched = crate::github::fetch_file(
            token,
            &target.owner,
            &target.repo,
            path,
            target.git_ref.as_deref(),
        )?;
        let Some(text) = fetched else { continue };
        let value = parse(&text).map_err(|e| format!("{path}: {e}"))?;
        let source = RepoRef {
            path: Some(path.clone()),
            ..target
        }
        .label();
        return Ok((value, source));
    }

    // Every candidate 404'd. That is also how a missing repo and a private one
    // look from here, so spend one more request telling those apart.
    if !crate::github::repo_visible(token, &target.owner, &target.repo).unwrap_or(true) {
        return Err(format!(
            "{}/{} not found — check the name, and note that a private repo needs a token \
             (config, `gh auth login`, or $GITHUB_TOKEN)",
            target.owner, target.repo
        ));
    }
    let repo = match &target.git_ref {
        Some(r) => format!("{}/{}@{r}", target.owner, target.repo),
        None => format!("{}/{}", target.owner, target.repo),
    };
    Err(match &target.path {
        Some(path) => format!(
            "{repo} has no {path} — check the path and the branch, and that the repo is visible \
             to your token"
        ),
        None => format!(
            "no {what} file in {repo} — tried {}. Give the path directly if it lives somewhere \
             else, and check the repo is visible to your token.",
            candidates.join(", ")
        ),
    })
}

/// Worker-thread body for a sync push. Resolves a bare repo name against the
/// token's account, optionally creates the repo, then writes the data file
/// (retrying once with a fresh SHA on a conflict).
fn run_sync(
    token: &str,
    repo: &str,
    file: &str,
    json: &str,
    sha: Option<&str>,
    setup: bool,
) -> Result<SyncOk, String> {
    let (owner, name) = match crate::github::parse_repo_string(repo) {
        Some(pair) => pair,
        None => {
            let name = repo.trim().trim_matches('/').to_string();
            if name.is_empty() || name.contains('/') {
                return Err("invalid repo name".into());
            }
            (crate::github::authed_login(token)?, name)
        }
    };

    let client = SyncClient::new(token, &owner, &name, file)?;
    if setup {
        client.ensure_repo()?;
    }

    let new_sha = client.push(json, sha).or_else(|e| {
        if sha.is_none() || e.contains("409") || e.contains("422") || e.contains("conflict") {
            let latest = client.pull()?.map(|r| r.sha);
            client.push(json, latest.as_deref())
        } else {
            Err(e)
        }
    })?;

    Ok(SyncOk {
        repo: format!("{owner}/{name}"),
        sha: new_sha,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app holding a single empty project — a base for the sorting tests.
    fn test_app() -> App {
        let mut store = Store::default();
        store.projects.push(Project::new("P"));
        App::new(store, Config::default(), None, None)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn step_clamps_within_bounds() {
        assert_eq!(step(0, -1, 5), 0);
        assert_eq!(step(4, 1, 5), 4);
        assert_eq!(step(2, 1, 5), 3);
        assert_eq!(step(2, -1_000_000, 5), 0);
        assert_eq!(step(2, 1_000_000, 5), 4);
        assert_eq!(step(3, 1, 0), 0); // empty list
    }

    #[test]
    fn parse_todo_extracts_priority_and_due() {
        let (title, prio, due, tags) = parse_todo_input("ship the release !3 @2026-09-15 #rel");
        assert_eq!(title, "ship the release");
        assert_eq!(prio, Priority::High);
        assert_eq!(due, NaiveDate::from_ymd_opt(2026, 9, 15));
        assert_eq!(tags, vec!["rel"]);
    }

    #[test]
    fn parse_todo_defaults_and_aliases() {
        let (title, prio, due, tags) = parse_todo_input("plain task");
        assert_eq!(title, "plain task");
        assert_eq!(prio, Priority::Medium);
        assert_eq!(due, None);
        assert!(tags.is_empty());

        let (.., prio, _, _) = parse_todo_input("do it !low");
        assert_eq!(prio, Priority::Low);
        let (.., prio, _, _) = parse_todo_input("do it !!");
        assert_eq!(prio, Priority::High);
    }

    #[test]
    fn parse_todo_keeps_invalid_date_token() {
        let (title, _, due, _) = parse_todo_input("mail @someone about it");
        assert_eq!(title, "mail @someone about it");
        assert_eq!(due, None);
    }

    #[test]
    fn parse_todo_tags_vs_issue_refs() {
        let (title, _, _, tags) = parse_todo_input("fix login #42 #auth-bug");
        assert_eq!(title, "fix login #42", "#42 is not a tag");
        assert_eq!(tags, vec!["auth-bug"]);
    }

    #[test]
    fn subtask_priority_parses_and_roundtrips() {
        let (title, prio, _) = parse_priority_input("wire up the API !3");
        assert_eq!(title, "wire up the API");
        assert_eq!(prio, Priority::High);

        let (title, prio, tags) = parse_priority_input("just a note #x #y #x");
        assert_eq!(title, "just a note");
        assert_eq!(prio, Priority::Medium);
        assert_eq!(tags, vec!["x", "y"], "cleaned + de-duped");

        let mut s = Subtask::new("polish copy", false);
        s.priority = Priority::Low;
        s.tags = vec!["ui".into()];
        let (title, prio, tags) = parse_priority_input(&subtask_edit_string(&s));
        assert_eq!(title, "polish copy");
        assert_eq!(prio, Priority::Low);
        assert_eq!(tags, vec!["ui"]);
    }

    #[test]
    fn parse_milestone_uses_fallback_when_no_date() {
        let fallback = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let (title, date) = parse_milestone_input("Launch v1", fallback).unwrap();
        assert_eq!(title, "Launch v1");
        assert_eq!(date, fallback);
    }

    #[test]
    fn parse_milestone_reads_explicit_date_and_rejects_empty() {
        let fallback = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let (title, date) = parse_milestone_input("Beta @2026-06-30", fallback).unwrap();
        assert_eq!(title, "Beta");
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        assert!(parse_milestone_input("@2026-06-30", fallback).is_none());
    }

    #[test]
    fn fuzzy_score_matches_subsequence_and_ranks() {
        assert!(fuzzy_score("abc", "xyz").is_none());
        assert!(fuzzy_score("abc", "aXbXc").is_some());
        // Contiguous run outranks a scattered match.
        let contiguous = fuzzy_score("side", "sidebar").unwrap();
        let scattered = fuzzy_score("side", "s i d e where").unwrap();
        assert!(contiguous > scattered);
        // Empty needle always matches.
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn todo_edit_string_roundtrips_through_parser() {
        let mut t = Todo::new("write docs");
        t.priority = Priority::High;
        t.due = NaiveDate::from_ymd_opt(2026, 3, 4);
        t.tags = vec!["docs".into(), "q3".into()];
        let (title, prio, due, tags) = parse_todo_input(&todo_edit_string(&t));
        assert_eq!(title, "write docs");
        assert_eq!(prio, Priority::High);
        assert_eq!(due, t.due);
        assert_eq!(tags, t.tags);
    }

    #[test]
    fn parse_meeting_input_splits_when_who_and_title() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        let (title, date, time, who) =
            parse_meeting_input("Design review @2026-09-05 14:30 +ana +You +ANA", today).unwrap();
        assert_eq!(title, "Design review");
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 9, 5).unwrap());
        assert_eq!(time.as_deref(), Some("14:30"));
        // The same person twice, however capitalised, lands once.
        assert_eq!(who, vec!["ana", "You"]);

        // No date given — today. No time — none. Single-digit hours pad.
        let (title, date, time, who) = parse_meeting_input("Standup 9:05", today).unwrap();
        assert_eq!((title.as_str(), date), ("Standup", today));
        assert_eq!(time.as_deref(), Some("09:05"));
        assert!(who.is_empty());

        // A title is required; a bare date isn't a meeting.
        assert!(parse_meeting_input("@2026-09-05 +ana", today).is_none());
        // Nonsense clocks stay part of the title.
        let (title, _, time, _) = parse_meeting_input("Sync 99:99", today).unwrap();
        assert_eq!(title, "Sync 99:99");
        assert!(time.is_none());
    }

    #[test]
    fn meeting_edit_string_roundtrips_through_the_parser() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 2).unwrap();
        let mut m = Meeting::new("Kickoff", NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        m.time = Some("08:30".into());
        m.attendees = vec!["Ana".into(), "Sam".into()];
        let (title, date, time, who) =
            parse_meeting_input(&meeting_edit_string(&m), today).unwrap();
        assert_eq!(title, m.title);
        assert_eq!(date, m.date);
        assert_eq!(time, m.time);
        assert_eq!(who, m.attendees);
    }

    #[test]
    fn sorting_meetings_puts_the_earliest_slot_first() {
        let mut app = test_app();
        app.tab = Tab::Meetings;
        app.focus = Focus::Content;
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 9, d).unwrap();
        let p = app.current_project_mut().unwrap();
        let mut afternoon = Meeting::new("afternoon", day(4));
        afternoon.time = Some("15:00".into());
        let mut morning = Meeting::new("morning", day(4));
        morning.time = Some("09:00".into());
        let untimed = Meeting::new("sometime", day(4));
        let mut earlier = Meeting::new("earlier day", day(3));
        earlier.held = true;
        p.meetings = vec![untimed, afternoon, earlier, morning];
        app.meeting_idx = 1; // "afternoon"

        app.apply_sort(SortScope::Meetings, SortKey::Date, false);
        let order: Vec<&str> = app
            .current_project()
            .unwrap()
            .meetings
            .iter()
            .map(|m| m.title.as_str())
            .collect();
        // Earlier day first, then that day's timed slots, then the untimed one.
        assert_eq!(order, ["earlier day", "morning", "afternoon", "sometime"]);
        assert_eq!(app.current_meeting().unwrap().title, "afternoon");

        app.apply_sort(SortScope::Meetings, SortKey::Held, false);
        assert_eq!(
            app.current_project()
                .unwrap()
                .meetings
                .last()
                .unwrap()
                .title,
            "earlier day"
        );
    }

    #[test]
    fn the_meetings_tab_edits_the_selected_meeting() {
        let mut app = test_app();
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 9, d).unwrap();
        app.current_project_mut().unwrap().meetings = vec![
            Meeting::new("standup", day(3)),
            Meeting::new("retro", day(9)),
        ];

        // `5` opens the tab, `j` moves onto the second meeting.
        app.handle_key(key(KeyCode::Char('5')));
        assert_eq!(app.tab, Tab::Meetings);
        assert_eq!(app.focus, Focus::Content);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.current_meeting().unwrap().title, "retro");

        // `x` marks it held, `l` steps into the notes pane, `N` opens the editor.
        app.handle_key(key(KeyCode::Char('x')));
        assert!(app.current_meeting().unwrap().held);
        app.handle_key(key(KeyCode::Char('l')));
        assert_eq!(app.focus, Focus::Detail);
        app.handle_key(key(KeyCode::Char('N')));
        assert!(matches!(
            app.mode,
            Mode::EditBody(ref e) if e.target == EditTarget::MeetingNote(1)
        ));

        // Typing in the editor and saving writes the meeting's notes.
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Char('i')));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.current_project().unwrap().meetings[1].note, "hi");
        assert!(matches!(app.mode, Mode::Normal));

        // `d` asks before deleting, and the confirmation removes it.
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Char('d')));
        assert!(matches!(
            app.mode,
            Mode::Confirm(ConfirmState {
                action: ConfirmAction::DeleteMeeting(1),
                ..
            })
        ));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let titles: Vec<&str> = app
            .current_project()
            .unwrap()
            .meetings
            .iter()
            .map(|m| m.title.as_str())
            .collect();
        assert_eq!(titles, ["standup"]);
    }

    #[test]
    fn sort_menu_opens_on_the_order_already_in_force() {
        let mut app = test_app();
        app.focus = Focus::Content;
        app.tab = Tab::Todos;
        // First open sits on the scope's default ordering, forwards.
        app.handle_key(key(KeyCode::Char('o')));
        let Mode::Sort(s) = &app.mode else {
            panic!("o should open the sort menu")
        };
        assert_eq!(s.scope, SortScope::Todos);
        assert_eq!(s.scope.keys()[s.sel], SortKey::Priority);
        assert!(!s.reverse);
        assert!(s.active.is_none());

        // Pick "Name", reversed.
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('r')));
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.mode, Mode::Normal));
        assert_eq!(
            app.sorts[SortScope::Todos.idx()],
            Some(SortOrder {
                key: SortKey::Name,
                reverse: true
            })
        );

        // Reopening lands back on it, still reversed, and marks it as active.
        app.handle_key(key(KeyCode::Char('o')));
        let Mode::Sort(s) = &app.mode else {
            panic!("expected the sort menu")
        };
        assert_eq!(s.scope.keys()[s.sel], SortKey::Name);
        assert!(s.reverse);
        assert_eq!(s.active.map(|o| o.key), Some(SortKey::Name));
    }

    #[test]
    fn sorting_todos_reorders_and_keeps_the_selection() {
        let mut app = test_app();
        let today = Local::now().date_naive();
        let p = app.current_project_mut().unwrap();
        p.todos = vec![Todo::new("beta"), Todo::new("alpha"), Todo::new("gamma")];
        p.todos[0].priority = Priority::Low;
        p.todos[1].priority = Priority::High;
        p.todos[2].priority = Priority::Medium;
        p.todos[0].due = Some(today + chrono::Duration::days(5));
        p.todos[2].due = Some(today);
        app.todo_idx = 0; // "beta"

        app.apply_sort(SortScope::Todos, SortKey::Priority, false);
        let titles: Vec<&str> = app
            .current_project()
            .unwrap()
            .todos
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["alpha", "gamma", "beta"]);
        assert_eq!(app.current_todo().unwrap().title, "beta");

        app.apply_sort(SortScope::Todos, SortKey::Name, true);
        let titles: Vec<&str> = app
            .current_project()
            .unwrap()
            .todos
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["gamma", "beta", "alpha"]);
        assert_eq!(app.current_todo().unwrap().title, "beta");

        // Undated todos stay at the bottom in both directions.
        app.apply_sort(SortScope::Todos, SortKey::Due, false);
        let titles: Vec<&str> = app
            .current_project()
            .unwrap()
            .todos
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["gamma", "beta", "alpha"]);
        app.apply_sort(SortScope::Todos, SortKey::Due, true);
        let titles: Vec<&str> = app
            .current_project()
            .unwrap()
            .todos
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["beta", "gamma", "alpha"]);
    }

    #[test]
    fn sorting_projects_and_notes_uses_their_own_keys() {
        let mut app = test_app();
        app.store.projects = vec![Project::new("zeta"), Project::new("alpha")];
        app.store.projects[0].todos = vec![Todo::new("a"), Todo::new("b")];
        app.project_idx = 0; // "zeta"

        app.apply_sort(SortScope::Projects, SortKey::Name, false);
        assert_eq!(app.store.projects[0].name, "alpha");
        assert_eq!(app.current_project().unwrap().name, "zeta");

        // Most open todos first.
        app.apply_sort(SortScope::Projects, SortKey::Open, false);
        assert_eq!(app.store.projects[0].name, "zeta");

        let p = app.current_project_mut().unwrap();
        p.notes = vec![
            Note::new("plain", false),
            Note::new("starred", true).with_body("body"),
        ];
        app.note_idx = 0;
        app.apply_sort(SortScope::Notes, SortKey::Pinned, false);
        let texts: Vec<&str> = app
            .current_project()
            .unwrap()
            .notes
            .iter()
            .map(|n| n.text.as_str())
            .collect();
        assert_eq!(texts, ["starred", "plain"]);
        assert_eq!(app.current_note().unwrap().text, "plain");
    }

    #[test]
    fn project_search_context_counts_todos_and_overdue() {
        let today = Local::now().date_naive();
        let mut p = Project::new("Juriba");
        assert_eq!(project_search_context(&p), "no todos");

        p.todos.push(Todo::new("a"));
        let mut b = Todo::new("b");
        b.due = Some(today - chrono::Duration::days(2));
        p.todos.push(b);
        let mut c = Todo::new("c");
        c.due = Some(today - chrono::Duration::days(1));
        c.done = true; // overdue but done -> not counted
        p.todos.push(c);
        assert_eq!(project_search_context(&p), "3 todos · 1 overdue");
    }

    #[test]
    fn search_target_depth() {
        assert_eq!(SearchTarget::Project.depth(), 0);
        assert_eq!(SearchTarget::Todo(0).depth(), 1);
        assert_eq!(SearchTarget::Note(0).depth(), 1);
        assert_eq!(SearchTarget::Subtask { todo: 0, sub: 0 }.depth(), 2);
    }

    #[test]
    fn parse_tagged_name_splits_project_tags() {
        let (name, tags) = parse_tagged_name("Website Redesign #web #Frontend");
        assert_eq!(name, "Website Redesign");
        assert_eq!(tags, vec!["web", "frontend"]);
    }
}

#[cfg(test)]
mod import_data_tests {
    use super::*;

    fn app() -> App {
        App::new(Store::default(), Config::default(), None, None)
    }

    #[test]
    fn links_are_remote_and_paths_are_local() {
        for remote in [
            "me/notes",
            "me/notes/voido-data.json",
            "me/notes@backup",
            "https://github.com/me/notes/blob/main/voido-data.json",
            "github.com/me/notes",
            "https://raw.githubusercontent.com/me/notes/main/data.json",
        ] {
            assert!(looks_like_github(remote), "{remote} should be a link");
        }
        for local in [
            "/tmp/voido-export.json",
            "~/voido-export.json",
            "./data.json",
            "../backups/data.json",
            "voido-export.json",
            "C:\\Users\\me\\data.json",
        ] {
            assert!(!looks_like_github(local), "{local} should be a path");
        }
    }

    #[test]
    fn an_existing_file_wins_over_a_repo_of_the_same_shape() {
        let dir = std::env::temp_dir().join("voido-import-test/repo-shaped");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.json");
        std::fs::write(&file, "{}").unwrap();
        let spec = file.to_string_lossy().replace(':', ""); // drop any drive colon
        if !spec.starts_with('/') {
            return; // Windows-style temp dir — the path rules already cover it
        }
        assert!(!looks_like_github(&spec));
    }

    #[test]
    fn a_bad_link_never_starts_a_fetch() {
        let mut a = app();
        a.spawn_data_fetch("not a repo");
        assert!(a.status.contains("invalid link"), "{}", a.status);
        assert!(!a.data_in_flight);
        assert!(a.data_rx.is_none());
    }

    #[test]
    fn the_configured_sync_file_is_tried_first() {
        let mut a = app();
        a.config.github_file = Some("archive/2026.json".into());
        let c = a.data_candidates();
        assert_eq!(c[0], "archive/2026.json");
        assert!(c.contains(&crate::config::DEFAULT_SYNC_FILE.to_string()));
        assert_eq!(
            c.iter().collect::<std::collections::HashSet<_>>().len(),
            c.len(),
            "no duplicates: {c:?}"
        );
    }

    #[test]
    fn a_fetched_dataset_asks_before_replacing_anything() {
        let mut a = app();
        let before = a.store.projects.len();
        let mut store = Store::default();
        let mut p = Project::new("Imported");
        p.todos.push(Todo::new("one"));
        p.todos.push(Todo::new("two"));
        store.projects.push(p);

        a.offer_data_import(ImportedData {
            store,
            source: "me/notes/voido-data.json".into(),
        });
        match &a.mode {
            Mode::Confirm(c) => {
                assert!(matches!(c.action, ConfirmAction::ImportData(_)));
                assert!(
                    c.prompt.contains("1 project · 2 todos · 0 notes"),
                    "{}",
                    c.prompt
                );
                assert!(c.prompt.contains("me/notes/voido-data.json"));
            }
            _ => panic!("expected a confirmation"),
        }
        assert_eq!(
            a.store.projects.len(),
            before,
            "data untouched until confirmed"
        );
    }
}

#[cfg(test)]
mod import_settings_tests {
    use super::*;

    fn app() -> App {
        App::new(Store::default(), Config::default(), None, None)
    }

    #[test]
    fn menu_entry_opens_the_prompt() {
        let mut a = app();
        a.run_menu(MenuAction::ImportSettings);
        match &a.mode {
            Mode::Input(input) => {
                assert!(matches!(input.action, InputAction::ImportSettings));
                assert!(input.title.contains("owner/repo"));
            }
            _ => panic!("expected an input prompt"),
        }
    }

    #[test]
    fn a_bad_spec_never_starts_a_fetch() {
        let mut a = app();
        a.spawn_settings_fetch("not a repo");
        assert!(a.status.contains("invalid repo"), "{}", a.status);
        assert!(!a.settings_in_flight);
        assert!(a.settings_rx.is_none());
    }

    #[test]
    fn identical_settings_change_nothing() {
        let mut a = app();
        a.offer_settings_import(ImportedSettings {
            config: Config::default(),
            source: "me/dotfiles/config.toml".into(),
        });
        assert!(
            matches!(a.mode, Mode::Normal),
            "no confirmation when there's nothing to change"
        );
    }

    #[test]
    fn a_difference_asks_before_changing_anything() {
        let mut a = app();
        let incoming = Config {
            theme: Some("dracula".into()),
            ..Config::default()
        };
        a.offer_settings_import(ImportedSettings {
            config: incoming,
            source: "me/dotfiles/config.toml".into(),
        });
        match &a.mode {
            Mode::Confirm(c) => {
                assert!(matches!(c.action, ConfirmAction::ImportSettings(_)));
                assert!(c.prompt.contains("me/dotfiles/config.toml"));
                assert!(c.prompt.contains("dracula"), "{}", c.prompt);
            }
            _ => panic!("expected a confirmation"),
        }
        assert!(
            a.config.theme.is_none(),
            "settings untouched until confirmed"
        );
    }
}
