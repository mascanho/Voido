//! A small Markdown → ratatui renderer for note bodies.
//!
//! Not a full CommonMark renderer, but it handles the things you actually put
//! in a note: headings, emphasis, inline + fenced code, bullet / numbered
//! lists (nested), block quotes, task lists, rules and links.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::{accent, blue, border, green, sel_bg, subtle, text, yellow};

/// Render `src` as styled lines sized for a `width`-column pane.
pub fn render(src: &str, width: u16) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);

    let mut r = Renderer::new(width);
    for event in Parser::new_ext(src, opts) {
        r.event(event);
    }
    r.finish()
}

fn code_style() -> Style {
    Style::new().fg(yellow()).bg(sel_bg())
}

/// Every link (and image) URL in `src`, paired with its visible text — falling
/// back to the URL itself when there's no text. Bare `http(s)://…` runs sitting
/// in plain prose are picked up too. De-duplicated by URL, in document order.
pub fn extract_links(src: &str) -> Vec<(String, String)> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);

    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push = |url: String, label: String| {
        let url = url.trim().to_string();
        if !url.is_empty() && seen.insert(url.clone()) {
            let label = label.trim();
            let label = if label.is_empty() { url.clone() } else { label.to_string() };
            out.push((label, url));
        }
    };

    // (url, accumulated link text) while inside a `[text](url)` span.
    let mut cur: Option<(String, String)> = None;

    for event in Parser::new_ext(src, opts) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                cur = Some((dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((url, text)) = cur.take() {
                    push(url, text);
                }
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => push(dest_url.to_string(), title.to_string()),
            Event::Text(t) | Event::Code(t) => match &mut cur {
                Some((_, text)) => text.push_str(t.as_ref()),
                None => {
                    for word in t.split(|c: char| {
                        c.is_whitespace()
                            || ['<', '>', '(', ')', '[', ']', '"', '\'', '`'].contains(&c)
                    }) {
                        let word = word.trim_end_matches(['.', ',', ';', ':', '!', '?']);
                        if word.starts_with("http://") || word.starts_with("https://") {
                            push(word.to_string(), word.to_string());
                        }
                    }
                }
            },
            _ => {}
        }
    }
    out
}

struct Renderer {
    width: usize,
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    line_open: bool,
    want_blank: bool,

    bold: u32,
    italic: u32,
    strike: u32,
    link: u32,
    heading: Option<HeadingLevel>,
    lists: Vec<Option<u64>>,
    quote: usize,
    code_block: bool,
}

impl Renderer {
    fn new(width: u16) -> Self {
        Self {
            width: width.max(8) as usize,
            lines: Vec::new(),
            spans: Vec::new(),
            line_open: false,
            want_blank: false,
            bold: 0,
            italic: 0,
            strike: 0,
            link: 0,
            heading: None,
            lists: Vec::new(),
            quote: 0,
            code_block: false,
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.end_line();
        while matches!(self.lines.last(), Some(l) if line_is_blank(l)) {
            self.lines.pop();
        }
        self.lines
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.text(&t),
            Event::Code(t) => {
                self.ensure_line();
                self.spans.push(Span::styled(t.to_string(), code_style()));
            }
            Event::SoftBreak => {
                if self.code_block {
                    self.end_line();
                    self.ensure_line();
                } else {
                    self.ensure_line();
                    self.spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                self.end_line();
                self.ensure_line();
            }
            Event::Rule => {
                self.end_line();
                self.want_blank = true;
                self.flush_blank();
                let n = self.width.saturating_sub(2).clamp(3, 72);
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(n),
                    Style::new().fg(border()),
                )));
                self.want_blank = true;
            }
            Event::TaskListMarker(done) => {
                self.ensure_line();
                let (glyph, style) = if done {
                    ("[x] ", Style::new().fg(green()))
                } else {
                    ("[ ] ", Style::new().fg(subtle()))
                };
                self.spans.push(Span::styled(glyph, style));
            }
            Event::Html(_) | Event::InlineHtml(_) | Event::FootnoteReference(_) => {}
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                self.ensure_line();
                self.spans.push(Span::styled(t.to_string(), code_style()));
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush_blank();
                self.ensure_line();
            }
            Tag::Heading { level, .. } => {
                self.want_blank = true;
                self.flush_blank();
                self.heading = Some(level);
                self.ensure_line();
            }
            Tag::BlockQuote(_) => {
                self.end_line();
                self.want_blank = true;
                self.flush_blank();
                self.quote += 1;
            }
            Tag::CodeBlock(_) => {
                self.end_line();
                self.want_blank = true;
                self.flush_blank();
                self.code_block = true;
                self.ensure_line();
            }
            Tag::List(first) => {
                if self.lists.is_empty() {
                    self.want_blank = true;
                    self.flush_blank();
                }
                self.lists.push(first);
            }
            Tag::Item => {
                self.end_line();
                let depth = self.lists.len();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}. ");
                        *n += 1;
                        m
                    }
                    _ if depth >= 2 => "◦ ".to_string(),
                    _ => "• ".to_string(),
                };
                self.ensure_line();
                self.spans
                    .push(Span::styled(marker, Style::new().fg(accent())));
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { .. } => self.link += 1,
            Tag::Image { .. } => {
                self.ensure_line();
                self.spans
                    .push(Span::styled("🖼 ", Style::new().fg(subtle())));
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_line(),
            TagEnd::Heading(_) => {
                self.end_line();
                self.heading = None;
                self.want_blank = true;
            }
            TagEnd::BlockQuote(_) => {
                self.end_line();
                self.quote = self.quote.saturating_sub(1);
                self.want_blank = true;
            }
            TagEnd::CodeBlock => {
                self.end_line();
                self.code_block = false;
                self.want_blank = true;
            }
            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.want_blank = true;
                }
            }
            TagEnd::Item => self.end_line(),
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => self.link = self.link.saturating_sub(1),
            _ => {}
        }
    }

    fn text(&mut self, t: &str) {
        if self.code_block {
            for (i, part) in t.split('\n').enumerate() {
                if i > 0 {
                    self.end_line();
                    self.ensure_line();
                }
                if !part.is_empty() {
                    self.spans
                        .push(Span::styled(format!("  {part}"), code_style()));
                }
            }
        } else {
            self.ensure_line();
            let style = self.inline_style();
            self.spans.push(Span::styled(t.to_string(), style));
        }
    }

    fn inline_style(&self) -> Style {
        if let Some(level) = self.heading {
            return match level {
                HeadingLevel::H1 => Style::new()
                    .fg(accent())
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                HeadingLevel::H2 => Style::new().fg(accent()).add_modifier(Modifier::BOLD),
                _ => Style::new().fg(text()).add_modifier(Modifier::BOLD),
            };
        }

        let mut s = if self.quote > 0 {
            Style::new().fg(subtle()).add_modifier(Modifier::ITALIC)
        } else {
            Style::new().fg(text())
        };
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.link > 0 {
            s = s.fg(blue()).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn prefix(&self) -> Option<Span<'static>> {
        let mut pre = String::new();
        for _ in 0..self.quote {
            pre.push_str("▎ ");
        }
        let depth = self.lists.len();
        if depth > 1 {
            pre.push_str(&"  ".repeat(depth - 1));
        }
        if pre.is_empty() {
            None
        } else {
            Some(Span::styled(pre, Style::new().fg(border())))
        }
    }

    fn ensure_line(&mut self) {
        if self.line_open {
            return;
        }
        self.line_open = true;
        if let Some(p) = self.prefix() {
            self.spans.push(p);
        }
    }

    fn end_line(&mut self) {
        if !self.line_open {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        self.lines.push(Line::from(spans));
        self.line_open = false;
    }

    fn flush_blank(&mut self) {
        if !self.want_blank {
            return;
        }
        self.want_blank = false;
        match self.lines.last() {
            None => {}
            Some(l) if line_is_blank(l) => {}
            Some(_) => self.lines.push(Line::default()),
        }
    }
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(src: &str) -> Vec<String> {
        render(src, 40)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn headings_lists_quotes() {
        let out = plain(
            "# Title\n\nsome **bold** text\n\n- one\n- two\n  - nested\n\n> quoted\n\n1. first\n2. second\n",
        );
        let joined = out.join("\n");
        assert!(joined.contains("Title"));
        assert!(joined.contains("• one"));
        assert!(joined.contains("◦ nested"));
        assert!(joined.contains("▎ quoted"));
        assert!(joined.contains("1. first"));
        assert!(joined.contains("2. second"));
        // blank line between blocks, never doubled
        assert!(!joined.contains("\n\n\n"));
    }

    #[test]
    fn extracts_links_and_bare_urls() {
        let links = extract_links(
            "See [the docs](https://example.com/docs) and https://bare.example.org.\n\n\
             Also [dup](https://example.com/docs) again, and <https://auto.example>.",
        );
        assert_eq!(
            links,
            vec![
                ("the docs".to_string(), "https://example.com/docs".to_string()),
                (
                    "https://bare.example.org".to_string(),
                    "https://bare.example.org".to_string()
                ),
                (
                    "https://auto.example".to_string(),
                    "https://auto.example".to_string()
                ),
            ]
        );
    }

    #[test]
    fn fenced_code_and_rule() {
        let out = plain("text\n\n```\nfn main() {}\n```\n\n---\n\nmore");
        let joined = out.join("\n");
        assert!(joined.contains("fn main() {}"));
        assert!(joined.contains("───"));
    }
}
