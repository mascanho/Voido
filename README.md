# shiki

A keyboard-first terminal app for **projects, todos, subtasks, notes and
timelines**, with Vim-style navigation. Built with Rust + [ratatui](https://ratatui.rs).

```
cargo run           # or: cargo build --release && ./target/release/shiki
```

Data is stored as pretty JSON at `~/Library/Application Support/shiki/data.json`
(macOS) / `~/.local/share/shiki/data.json` (Linux) and saved automatically.

## Layout

```
 ┌ Projects ─┐┌ <project> · Overview Todos Notes Timeline ─┐┌ Subtasks 1/3 ┐
 │ ● Website ││ ○ Design system in Figma  high  Sep 01     ││ ✔ Hero        │
 │ ● Shiki   ││ ○ Rebuild the home page   ⊞ 1/3            ││ ○ Nav+footer  │
 └───────────┘└────────────────────────────────────────────┘└──────────────┘
  status                                          context-sensitive key hints
```

- **Overview** — description, todo/subtask progress bar, note count, next milestone
- **Todos** — checkbox, priority, due date, subtask progress; `l` opens the
  **Subtasks** pane on the right for the selected todo
- **Notes** — quick per-project notes; `x` pins one (★). `l` opens the note's
  **Markdown body** in the right pane, rendered properly (headings, nested
  lists, block quotes, fenced code, rules, `**bold**` / `*italic*` / `` `code` ``).
  `e` opens a full Markdown editor (`esc` or `^s` to save)
- **Timeline** — every milestone (◆) plus every due-dated todo, sorted by date,
  with `overdue` / `in 3d` markers

## Keys

| Scope | Keys |
| --- | --- |
| Global | `1` `2` `3` focus panel (projects / middle / right) · `tab` cycle · `t`/`T` next/prev tab · `gg`/`G` top/bottom · `?` help · `q` quit |
| Projects | `j`/`k` move · `a` add · `r` rename · `d` delete · `l`/`enter` open |
| Todos | `j`/`k` move · `a` add · `e` edit · `x` done · `p` priority · `J`/`K` reorder · `l` subtasks · `d` delete |
| Subtasks | `j`/`k` move · `a` add · `e` edit · `x` done · `J`/`K` reorder · `d` delete · `h` back |
| Notes | `j`/`k` move · `a` add · `e` edit title · `x` pin · `J`/`K` reorder · `l` open · `d` delete |
| Note body | `j`/`k` scroll · `^d`/`^u` page · `e` edit Markdown · `h` back |
| MD editor | type freely · `esc` / `^s` save & close |
| Timeline | `j`/`k` move · `a` add milestone · `e`/`x`/`d` edit/toggle/delete milestone |
| Overview | `e` edit description · `r` rename project |

### Quick-add syntax

When adding or editing a todo:

```
ship the release !3 @2026-09-15
```

- `!1` `!2` `!3` — priority (low / med / high)
- `@YYYY-MM-DD` — due date

For milestones, add `@YYYY-MM-DD` for the date (defaults to today).
