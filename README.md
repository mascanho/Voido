# voido

A keyboard-first terminal app for **projects, todos, subtasks, notes and
timelines**, with Vim-style navigation. Built with Rust + [ratatui](https://ratatui.rs).

```
cargo run           # or: cargo build --release && ./target/release/voido
```

Data is stored in a SQLite database at `~/Library/Application Support/voido/voido.db`
(macOS) / `~/.local/share/voido/voido.db` (Linux) and saved automatically after
every change (each write is a single transaction). A legacy `data.json` from an
older version is imported automatically on first run.

### GitHub sync

voido can keep a copy of your data in a GitHub repo — it pulls the latest on
startup and pushes on exit (and on demand with `^s`).

**If you have the [`gh` CLI](https://cli.github.com) set up** (`gh auth login`),
it's zero config: press `^s`, hit Enter to accept `voido-data`, and voido
creates the private repo and syncs. Same deal if `GITHUB_TOKEN` / `GH_TOKEN` is
in your environment.

**Otherwise** `^s` asks for a repo (`owner/repo` or just a name) and a token —
classic PAT with **`repo`** scope, or fine-grained with **Contents: read and
write**. voido creates the repo if it's under your account and doesn't exist yet.

- Token precedence: the one you enter in `^s` → `gh auth token` → environment.
  Only a token you type is written to config.
- Conflicts are last-write-wins (whichever machine exits last).
- A failed sync pops a dialog with GitHub's exact error.

#### Importing data from a repo

You don't have to set sync up to pull a dataset in — **`^k` → Import data…**
takes a link as happily as a file path:

```
me/notes                                                # looks for the usual data files
me/notes/backups/voido-data.json                        # exact path
me/notes@last-week                                      # a branch, tag or SHA
https://github.com/me/notes/blob/main/voido-data.json   # a URL you copied
```

With a repo and no path, voido tries your own `github_file` name first, then
`voido-data.json`, `data/voido-data.json`, `voido/voido-data.json`, `voido.json`
and `data.json`. Anything that names a file on disk is still read as a file, so
`./data.json` and `~/voido-export.json` behave exactly as before.

The fetch runs in the background; when it lands voido shows what it found —
*"Replace ALL local data with 4 projects · 27 todos · 9 notes from
me/notes/voido-data.json?"* — and **replaces your data only after you press
`y`**. It's the same JSON that Export writes and sync stores, so any of those
files work. Public repos need no token; private ones use the usual one.

This is a one-off copy, not a subscription: it doesn't turn sync on or change
where you sync to. Press `^s` for that.

#### Settings file

Hand-editable TOML at `~/Library/Application Support/voido/config.toml` (macOS) /
`~/.config/voido/config.toml` (Linux). Every key is optional — omit one for the
default. The file is written with a comment block documenting every key, so `^e`
is usually all the reference you need. An older `config.json` is converted to
TOML automatically on first run (kept as `config.json.bak`).

```toml
storage = "github"
github_repo = "me/my-notes"
github_file = "notes.json"
# github_token = "…"   # usually omitted
```

| key | default | meaning |
| --- | --- | --- |
| `storage` | `"local"` | `"local"` or `"github"` (turns sync on) |
| `github_repo` | – | `owner/repo`, or a bare name (owner = the token's account) |
| `github_file` | `voido-data.json` | name of the data file inside the repo; subpaths like `data/notes.json` work |
| `github_token` | – | omit it to use `gh` / `$GITHUB_TOKEN` |
| `weather` | – | place name (`"Lisbon"`), `"lat,lon"`, or `"auto"` (IP-based). Empty = off, no network call |
| `weather_unit` | `"c"` | `"c"` or `"f"` |
| `theme` | – | colour theme slug; pick live with `^t` |

`^s` writes `storage` and `github_repo` for you; set `github_file` yourself if you
want the synced file called something else. Changing it just starts a fresh file
under the new name (the old one is left in the repo). If the file is malformed,
voido refuses to start rather than overwrite your edits.

Press **`^e`** in the app to open this file in `$VISUAL` / `$EDITOR` (or `vi`);
it's reloaded when you close the editor — a theme added or changed there shows up
in `^t` straight away, no restart.

#### Importing settings from a repo

To carry your setup to another machine, keep the settings file in a repo and pull
it in: **`^k` → Settings from GitHub**. Enter any of

```
me/dotfiles                                             # looks in the usual places
me/dotfiles/voido/config.toml                           # exact path
me/dotfiles@work/config.toml                            # a branch, tag or SHA
https://github.com/me/dotfiles/blob/main/config.toml    # a URL you copied
```

Given a repo with no path, voido tries `config.toml`, `voido.toml`,
`voido/config.toml`, `.config/voido/config.toml`, `config/voido/config.toml` and
`config.json`, in that order.

voido shows exactly which keys would change and asks before writing anything.
Everything else then applies immediately — themes, weather, sync repo — except
data from a newly imported sync repo, which arrives on the next start.

- **Your `github_token` is never imported.** A token in the fetched file is
  ignored (and called out in the confirmation); the local one stays put.
- Public repos work with no token at all. A private one needs the usual token —
  from the settings file, `gh auth login`, or `$GITHUB_TOKEN`.
- The file can be TOML or the older JSON shape.

### Themes

Press **`^t`** for the theme picker — `j`/`k` previews live, `enter` keeps it,
`esc` reverts. The choice is saved as `theme` in the settings file, so you can
also set it there directly. 25 are bundled: Catppuccin (Mocha / Macchiato /
Frappé / Latte), Tokyo Night, Kanagawa, Dracula, Nord, Rosé Pine (+ Moon /
Dawn), Gruvbox (Dark / Material / Light), Everforest (Dark / Light), One Dark,
Monokai, Ayu (Dark / Mirage), GitHub (Dark / Light), Zenburn, Solarized
(Dark / Light).

Add your own in the settings file — they show up in the picker alongside the
built-ins:

```toml
[[themes]]
name = "My Neon"
accent = "#ff00ff"
green  = "#00ff88"
red    = "#ff3355"
yellow = "#ffee00"
blue   = "#22ddff"
text   = "#e8e8ff"
subtle = "#7a7a99"
border = "#333355"
sel_bg = "#222244"
bg     = "#0a0a14"
```

All slots take `#rrggbb`. `on_accent` (text on a colour fill) is optional and
defaults to `bg`. The name is slugified for the `theme` key (`My Neon` →
`my-neon`); reuse a built-in's slug to override it. A theme that fails to parse
is skipped with a message on startup.

### Weather

Set `weather` in the settings file to a place name, a `lat,lon` pair, or `auto`
(IP-based) and voido shows current conditions — a compact glyph + temperature in
the header, a fuller line on the Overview tab, and **`^w`** for the full modal
(feels-like, humidity, wind + gusts + direction, pressure, cloud cover, plus a
3-day outlook with highs/lows, sunrise/sunset, UV and rain chance). Data is from
[Open-Meteo](https://open-meteo.com) (no API key); fetched on a background thread
at startup and refreshed every 30 minutes. Fetch errors go to the `^l` Logs panel
and are otherwise silent. Leave `weather` unset for no network call at all.

### GitHub activity

The resolved token also lifts the anonymous rate limit on the repo-activity view
(`^g` to link a code repo, `R` in the projects rail to view).

Each project gets its own icon in the rail (picked from its name, so it's stable
between runs). The icon's colour is the status: dim = empty, accent = open
todos, green = everything done — and a finished project's name is struck through.
Each row shows the todo tally right-aligned (`done/total`, `✓` when complete,
`·` when empty), dropped when the pane is too narrow.

Every list works the same way: a mark, the title (which flexes to fill the
width and truncates with `…`), then metadata in **fixed right-hand columns** —
priority (`↑`/`↓`), subtask progress (`⊞`), note/attachment marks (`¶` `📎`),
due date. Each column lines up vertically whether or not a given row has a value
for it, and columns drop out as the pane narrows, so the metadata never gets
shoved off the edge by a long title and rows never look ragged.

Press **`i`** in the Projects rail, the Todos pane, or the Subtasks pane to open
a detail panel beneath the selected row — its **tags**, dates, and counts. Each
pane keeps its own `i` toggle, so expanding todos doesn't also expand the rail.
`i` again closes it.

## Layout

```
 ╭ Projects ──────╮╭ Ovw  Todo  Note  Sched  Meet ──────── Website ╮╭ Subtasks 1/3 ╮
 │▍◈ Website  3/8 ││ ○ Design system in Figma        ↑  ⊞2/3    Sep 01││ ✔ Hero       │
 │ ◆ Voido      ✓ ││ ○ Rebuild the home page              ¶     Sep 09││ ○ Nav+footer │
 ╰────────────────╯╰──────────────────────────────────────────────╯╰──────────────╯
  N  TODOS         ☀ 16°C · Wed Aug 30 · 14:20         Website > todos
```

The middle pane's top border is the tab strip; it falls back to short labels
(`Ovw  Todo  Note  Sched  Meet`) whenever the pane is too narrow for the full
titles, as it is in the three-pane views above.

The footer carries the mode (`N`/`I`) and focused pane on the left, the weather +
clock centred, and a breadcrumb on the right. One-off events (a finished sync, a
settings reload) surface as a self-dismissing toast in the bottom-right rather
than sticking in the footer; the full history is in the `^l` panel.

The middle pane's tabs live on its top border. Left rail is the project list;
the right pane shows subtasks (Todos) or the note body (Notes).

Press **`^k`** for the main menu — a single list of the global actions: **Save
now**, **Sync to GitHub**, **Export data…**, **Import data…**, **Settings**,
**Settings from GitHub…**, **Theme**, **Weather**, **Activity log**,
**Keybindings** and **Quit**. `j`/`k` move, `enter` runs the highlighted item,
`esc` closes.

- **Export** writes your whole dataset to a JSON file (defaults to
  `~/voido-export-<date>.json`; edit the path before pressing enter).
- **Import** takes **either a file path or a GitHub link** and, after a `y`/`n`
  confirm, **replaces everything** with what it finds — see
  [Importing data from a repo](#importing-data-from-a-repo).
- **Settings from GitHub** pulls a settings file out of a repo and, after a
  `y`/`n` confirm on the exact list of changes, adopts it — see
  [Importing settings from a repo](#importing-settings-from-a-repo).

Press **`^l`** for the activity panel — a strip below the panes with two tables:
**Logs** (app events: startup, sync, settings reloads, errors) and **Changes**
(every data edit you've made this session, newest at the bottom). Session-only,
not persisted. `^l` again hides it.

- **Overview** — description, todo/subtask progress bar, note count, next
  milestone and next meeting
- **Todos** — checkbox, priority, due date, subtask progress; `l` opens the
  **Subtasks** pane on the right for the selected todo. `o` opens the **sort
  menu** for the list in focus (see below). A todo that has subtasks is ticked
  automatically once they're all
  done (and un-ticked if one reopens). Each todo **and subtask** can carry a
  Markdown **note** (`¶`) and a list of **attachments** — links, files or images
  (`A`, marked `📎`); `o` / `enter` in the manager hands the item to your system
  opener.
  - `N` edits a note; `n` shows the rendered note — a **todo's** note fills the
    Subtasks pane, a **subtask's** (`↳`) opens in a section below the subtask
    list. `n` again hides it. A subtask note opens **focused** so `j`/`k` (and
    `^d` / `^u`) scroll it; `l` re-enters it from the list, `h` / `esc` steps
    back out to the subtasks
  - An open todo note wears the accent border **instead of** the todo list it
    came from — exactly one pane is lit at a time, so it's always clear what
    you're reading (the list still takes `j`/`k` to walk between todos)
  - **`^f`** blows whichever note is on screen up to **full screen** — a todo's,
    a subtask's, or the selected note's body in the Notes tab. `j`/`k`, `^d`/`^u`
    and `g`/`G` scroll it; `^f`, `esc` or `h` drops back to the panes
- **Notes** — quick per-project notes; `x` pins one (★). `l` opens the note's
  **Markdown body** in the right pane, rendered properly (headings, nested
  lists, block quotes, fenced code, rules, `**bold**` / `*italic*` / `` `code` ``).
  `e` opens a full Markdown editor (`esc` or `^s` to save)
- Whenever a note (project, todo or subtask) is on screen, **`L`** lists every
  link in it — `o` / `enter` opens one in your browser, `y` copies the URL to the
  clipboard (`pbcopy` / `wl-copy` / `xclip` / `xsel`). Bare `https://…` URLs in
  the text count, not just `[markdown](links)`.
- **Timeline** — every milestone (◆) plus every due-dated todo, sorted by date,
  with `overdue` / `in 3d` markers
- **Meetings** — what's on the calendar for the project: date, start time and
  who's in it, `● today` / `in 2d` / `3d ago` on the row, `✔` once it's been
  held (`x`). `l` opens the meeting's **agenda / minutes** in the right pane —
  the same Markdown pane the Notes tab uses, so `^f`, `L` and the editor (`N`)
  all work on it. `i` shows the detail panel, `r` moves it, `o` reorders the list

## Keys

`h` `j` `k` `l` (and the arrows) move — `h`/`l` also step between the three
panes. Click a pane, tab, or row to jump there; the wheel scrolls. Press `?` any
time for the full list.

Each list has its own set of orderings, offered by **`o`**:

| List | Orderings |
| --- | --- |
| Projects | name · next deadline · open todos · progress · created · tag |
| Todos | priority · due date · name · status · progress · tag |
| Subtasks | priority · name · status · tag |
| Notes | pinned · name · note length |
| Meetings | date · name · held · attendees |

The menu opens on the ordering that list is already in (marked `↓`/`↑` for its
direction); `r` reverses the highlighted one, `enter` applies it. Sorts are
stable, so items that tie keep the order `J`/`K` put them in, and anything
missing the value being sorted on (a todo with no due date, an untagged item)
stays at the bottom either way.

| Scope | Keys |
| --- | --- |
| Global | `h`/`l` switch pane · `w`/`s` prev/next project · `tab`/`S-tab` switch view · `1`‑`5` jump to a view · `gg`/`G` top/bottom · `esc` back out · `/` fuzzy find · `m` minimal view · `^k` main menu · `^l` activity panel · `^w` weather · `^t` theme · `^s` save to GitHub · `^e` edit settings file · `^f` full-screen the note on screen · `?` help · `q` quit (asks to confirm — `qq`, `y` or `enter`; `^c` quits straight away) |
| Projects | `a` add · `r` rename · `d` delete · `l` open · `i` detail · `t` tags · `o` sort menu · `^g` link/unlink code repo · `R` repo activity |
| Overview | `e` edit description · `r` rename · `t` tags |
| Todos | `a` add · `e` edit · `d` delete · `x` (or space) done · `p` priority · `o` sort menu · `J`/`K` reorder · `l` subtasks · `i` detail · `n` view note · `^f` full-screen it · `N` edit note · `t` tags · `A` attachments |
| Subtasks | `a` add · `e` edit · `d` delete · `x` done · `p` priority · `o` sort menu · `J`/`K` reorder · `i` detail · `n` view note · `N` edit note · `l` focus note (then `j`/`k` scroll, `h` back) · `t` tags · `A` attachments · `h` back |
| Tags (`t`) | `a` add one or more (space-separated) · `d` remove the selected tag · `esc` close |
| Sort (`o`) | `j`/`k` pick an ordering · `r` reverse it · `enter` apply · `esc` close |
| Notes | `a` add · `e` edit title · `d` delete · `x` pin · `o` sort menu · `J`/`K` reorder · `l` open body |
| Note body | `j`/`k` · `^d`/`^u` scroll · `space` expand · `^f` full screen · `e` edit · `h` back |
| MD editor | type freely · `esc` / `^s` save & close |
| Schedule | `a` add milestone · `e`/`d` edit/delete · `x` done · `r` reschedule · `f` cycle filter · `l` jump to todo |
| Meetings | `a` add · `e` edit · `d` delete · `x` held · `r` reschedule · `o` sort menu · `J`/`K` reorder · `i` detail · `l` read the notes · `N` edit them |
| Meeting notes | `j`/`k` · `^d`/`^u` scroll · `^f` full screen · `e`/`N` edit · `h` back |
| Find (`/`) | fuzzy-match projects, todos, subtasks, notes & their tags — each hit shows its type glyph, indent depth, and `project › todo` path; a project hit also shows its todo count (`8 todos · 2 overdue`, overdue in red) and a todo hit its subtask progress (`↳ 2/5`) · `↑`/`↓` or `^n`/`^p` move · `enter` jump · `esc` cancel |
| Attachments (`A`) | `a` add a URL or path (append `\| label` to name it) · `o`/`enter` open with the system opener · `d` remove · `esc` close |

### Quick-add syntax

When adding or editing a todo:

```
ship the release !3 @2026-09-15 #release #frontend
```

- `!1` `!2` `!3` — priority (low / med / high)
- `@YYYY-MM-DD` — due date
- `#tag` — a tag (lower-cased, `a-z0-9-_`); repeatable

Subtasks take `!priority` and `#tag`; adding or renaming a **project** takes
`#tag` too (`Website Redesign #web`). Tags aren't shown on the row — press `i`
for the detail panel — and `/` search matches them.

To manage tags **after** creating something, press **`t`** on the selected
project / todo / subtask (or from the Overview) — a small panel where `a` adds
one or more space-separated tags and `d` removes the highlighted one.

For milestones, add `@YYYY-MM-DD` for the date (defaults to today).

Meetings take a date, a start time and attendees:

```
Design review @2026-09-05 14:30 +ana +sam
```

- `@YYYY-MM-DD` — the day (defaults to today)
- `14:30` — the start time (optional; `9:05` works too)
- `+name` — an attendee; repeatable, and the same name twice counts once
