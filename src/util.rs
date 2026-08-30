//! Small shared helpers.

use std::hash::{Hash, Hasher};

/// Truncate `s` to at most `max` characters, adding an ellipsis when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// Nerd Font glyphs handed out to projects, one per project, for a bit of visual
/// variety in the rail. (folder · rocket · book · code · star · flag · cog ·
/// cube · flask · globe · terminal · puzzle · briefcase · bolt · leaf · brush)
pub const PROJECT_ICONS: [&str; 16] = [
    "\u{f07b}", "\u{f135}", "\u{f02d}", "\u{f121}", "\u{f005}", "\u{f024}", "\u{f013}", "\u{f1b2}",
    "\u{f0c3}", "\u{f0ac}", "\u{f120}", "\u{f12e}", "\u{f0b1}", "\u{f0e7}", "\u{f06c}", "\u{f1fc}",
];

/// A pseudo-random icon for a project, derived from its name so it stays the
/// same between runs (and only changes if the project is renamed). Callers that
/// have the whole project list should de-duplicate — see `project_icons`.
pub fn project_icon(name: &str) -> &'static str {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    PROJECT_ICONS[(h.finish() % PROJECT_ICONS.len() as u64) as usize]
}

/// One icon per name, hashed for stability then nudged off collisions so every
/// project in the list looks distinct (until there are more than
/// [`PROJECT_ICONS`] projects, when repeats are unavoidable).
pub fn project_icons<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let mut used: Vec<&'static str> = Vec::new();
    names
        .into_iter()
        .map(|name| {
            let mut icon = project_icon(name);
            if used.contains(&icon)
                && let Some(free) = PROJECT_ICONS.iter().find(|c| !used.contains(c))
            {
                icon = free;
            }
            used.push(icon);
            icon
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_short_strings() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn cuts_long_strings_with_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn counts_chars_not_bytes() {
        // 5 multi-byte chars, limit 5 -> unchanged
        assert_eq!(truncate("café☕é", 6), "café☕é");
    }

    #[test]
    fn project_icon_is_stable_and_in_set() {
        let a = project_icon("Voido2");
        assert_eq!(a, project_icon("Voido2"), "same name -> same icon");
        assert!(PROJECT_ICONS.contains(&a));
    }

    #[test]
    fn project_icons_deduplicates() {
        // More names than icons -> the first 16 are all distinct.
        let names: Vec<String> = (0..20).map(|i| format!("p{i}")).collect();
        let icons = project_icons(names.iter().map(String::as_str));
        let first16: std::collections::HashSet<_> = icons[..16].iter().collect();
        assert_eq!(first16.len(), 16, "no repeats until the set is exhausted");
    }
}
