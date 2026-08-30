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
startup and pushes on exit (and on demand with `^y`).

**If you have the [`gh` CLI](https://cli.github.com) set up** (`gh auth login`),
it's zero config: press `^y`, hit Enter to accept `voido-data`, and voido
creates the private repo and syncs. Same deal if `GITHUB_TOKEN` / `GH_TOKEN` is
in your environment.

**Otherwise** `^y` asks for a repo (`owner/repo` or just a name) and a token —
classic PAT with **`repo`** scope, or fine-grained with **Contents: read and
write**. voido creates the repo if it's under your account and doesn't exist yet.

- Token precedence: the one you enter in `^y` → `gh auth token` → environment.
  Only a token you type is written to config.
- Conflicts are last-write-wins (whichever machine exits last).
- A failed sync pops a dialog with GitHub's exact error.

#### Settings file

Hand-editable JSON at `~/Library/Application Support/voido/config.json` (macOS) /
`~/.config/voido/config.json` (Linux). Every key is optional — omit one (or set
it to `null`) for the default.

```json
{
  "storage": "github",
  "github_repo": "me/my-notes",
  "github_file": "notes.json",
  "github_token": null
}
```

| key | default | meaning |
| --- | --- | --- |
| `storage` | `"local"` | `"local"` or `"github"` (turns sync on) |
| `github_repo` | – | `owner/repo`, or a bare name (owner = the token's account) |
| `github_file` | `voido-data.json` | name of the data file inside the repo; subpaths like `data/notes.json` work |
| `github_token` | `null` | leave `null` to use `gh` / `$GITHUB_TOKEN` |

`^y` writes `storage` and `github_repo` for you; set `github_file` yourself if you
want the synced file called something else. Changing it just starts a fresh file
under the new name (the old one is left in the repo). If the file is malformed,
voido refuses to start rather than overwrite your edits.

Press **`^e`** in the app to open this file in `$VISUAL` / `$EDITOR` (or `vi`);
it's reloaded when you close the editor.

### Themes

Press **`^t`** for the theme picker — `j`/`k` previews live, `enter` keeps it,
`esc` reverts. The choice is saved as `"theme"` in the settings file, so you can
also set it there directly. Bundled: Catppuccin (Mocha / Latte), Tokyo Night,
Dracula, Nord, Gruvbox (Dark / Light), One Dark, Rosé Pine, Solarized
(Dark / Light).

Add your own in the settings file — they show up in the picker alongside the
built-ins:

```json
"themes": [
  {
    "name": "My Neon",
    "accent": "#ff00ff", "green": "#00ff88", "red": "#ff3355",
    "yellow": "#ffee00", "blue": "#22ddff", "text": "#e8e8ff",
    "subtle": "#7a7a99", "border": "#333355", "sel_bg": "#222244",
    "bg": "#0a0a14"
  }
]
```

All slots take `#rrggbb`. `on_accent` (text on a colour fill) is optional and
defaults to `bg`. The name is slugified for the `"theme"` key (`My Neon` →
`my-neon`); reuse a built-in's slug to override it. A theme that fails to parse
is skipped with a message on startup.

### GitHub activity

The resolved token also lifts the anonymous rate limit on the repo-activity view
(`^g` to link a code repo, `o` to view).

Each project gets its own icon in the rail (picked from its name, so it's stable
between runs). The icon's colour is the status: dim = empty, accent = open
todos, green = everything done — and a finished project's name is struck through.

## Layout

```
 ╭ Projects ─╮╭ Overview  Todos  Notes  Schedule ─── Website ╮╭ Subtasks 1/3 ╮
 │ ◈ Website ││ ○ Design system in Figma   high   Sep 01     ││ ✔ Hero        │
 │ ◆ Voido   ││ ○ Rebuild the home page    med  ⊞ 1/3        ││ ○ Nav+footer  │
 ╰───────────╯╰──────────────────────────────────────────────╯╰──────────────╯
  NORMAL  TODOS   <status>   context key hints          Website > todos
```

The middle pane's tabs live on its top border. Left rail is the project list;
the right pane shows subtasks (Todos) or the note body (Notes).

- **Overview** — description, todo/subtask progress bar, note count, next milestone
- **Todos** — checkbox, priority, due date, subtask progress; `l` opens the
  **Subtasks** pane on the right for the selected todo. `o` sorts the list by
  priority. A todo that has subtasks is ticked automatically once they're all
  done (and un-ticked if one reopens). Each todo **and subtask** can carry a
  Markdown **note** (`¶`) and a list of **attachments** — links, files or images
  (`A`, marked `📎`); `o` / `enter` in the manager hands the item to your system
  opener.
  - `N` edits a note; `n` shows the rendered note — a **todo's** note fills the
    Subtasks pane, a **subtask's** (`↳`) opens in a section below the subtask
    list. `n` again hides it; `^d` / `^u` scroll it
- **Notes** — quick per-project notes; `x` pins one (★). `l` opens the note's
  **Markdown body** in the right pane, rendered properly (headings, nested
  lists, block quotes, fenced code, rules, `**bold**` / `*italic*` / `` `code` ``).
  `e` opens a full Markdown editor (`esc` or `^s` to save)
- **Timeline** — every milestone (◆) plus every due-dated todo, sorted by date,
  with `overdue` / `in 3d` markers

## Keys

`h` `j` `k` `l` (and the arrows) move — `h`/`l` also step between the three
panes. Click a pane, tab, or row to jump there; the wheel scrolls. Press `?` any
time for the full list.

| Scope | Keys |
| --- | --- |
| Global | `h`/`l` switch pane · `w`/`s` prev/next project · `tab`/`S-tab` switch view · `1`‑`4` jump to a view · `gg`/`G` top/bottom · `esc` back out · `/` fuzzy find · `m` minimal view · `^t` theme · `^y` GitHub sync · `^e` edit settings file · `?` help · `q` quit |
| Projects | `a` add · `r` rename · `d` delete · `l` open · `^g` link/unlink code repo · `o` repo activity |
| Overview | `e` edit description · `r` rename |
| Todos | `a` add · `e` edit · `d` delete · `x` (or space) done · `p` priority · `o` sort by priority · `J`/`K` reorder · `l` subtasks · `n` view note · `N` edit note · `A` attachments |
| Subtasks | `a` add · `e` edit · `d` delete · `x` done · `p` priority · `J`/`K` reorder · `n` view note · `N` edit note · `^d`/`^u` scroll note · `A` attachments · `h` back |
| Notes | `a` add · `e` edit title · `d` delete · `x` pin · `J`/`K` reorder · `l` open body |
| Note body | `j`/`k` · `^d`/`^u` scroll · `space` expand · `e` edit · `h` back |
| MD editor | type freely · `esc` / `^s` save & close |
| Schedule | `a` add milestone · `e`/`d` edit/delete · `x` done · `r` reschedule · `f` cycle filter · `l` jump to todo |
| Find (`/`) | type to filter projects, todos & notes · `↑`/`↓` or `^n`/`^p` move · `enter` jump · `esc` cancel |
| Attachments (`A`) | `a` add a URL or path (append `\| label` to name it) · `o`/`enter` open with the system opener · `d` remove · `esc` close |

### Quick-add syntax

When adding or editing a todo:

```
ship the release !3 @2026-09-15
```

- `!1` `!2` `!3` — priority (low / med / high)
- `@YYYY-MM-DD` — due date

For milestones, add `@YYYY-MM-DD` for the date (defaults to today).
