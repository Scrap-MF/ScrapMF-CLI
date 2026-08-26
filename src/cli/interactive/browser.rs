//! Shared two-pane browser — the ranger/yazi-style navigator used by every
//! menu in scrapmf (home, scrape flows, configuration tree, plugins).
//!
//! Left pane: entries with a cursor (and checkboxes in Multi mode). Right
//! pane: live details of the highlighted entry. Footer: key hints only —
//! the caller's box title owns the identity (version, etc.).
//!
//! No mouse capture on purpose: text selection stays available.

use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph},
};

use crate::ui::dashboard::TerminalGuard;

/// One navigable row with its right-pane description.
#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub details: Vec<String>,
}

impl Entry {
    pub fn new(label: impl Into<String>, details: Vec<String>) -> Self {
        Self {
            label: label.into(),
            details,
        }
    }
}

/// Selection semantics of the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// ↑↓ move · Enter picks the highlighted entry.
    Single,
    /// ↑↓ move cursor · Space toggles under it · `a` all/none · Enter
    /// confirms returning every checked index.
    Multi,
}

/// Result of running a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Picked(usize),
    Toggled(Vec<usize>),
    Quit,
}

/// Fluent builder — see [`Browser::run`].
#[derive(Default)]
pub struct Browser {
    title: String,
    entries: Vec<Entry>,
    mode: Option<Mode>,
    checked: Vec<usize>,
}

impl Browser {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    pub fn entry(mut self, label: impl Into<String>, details: Vec<String>) -> Self {
        self.entries.push(Entry::new(label, details));
        self
    }

    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Pre-checked indices (Multi only; ignored otherwise).
    // Used by the content-kinds / cookie-networks browsers (next phase).
    #[allow(dead_code)]
    pub fn checked(mut self, idxs: &[usize]) -> Self {
        self.checked = idxs.to_vec();
        self
    }

    pub fn run(self) -> Outcome {
        let mode = self.mode.unwrap_or(Mode::Single);
        run_browser(&self.title, &self.entries, mode, &self.checked)
    }
}

// ─── Pure helpers (unit-tested) ─────────────────────────────────────────────

/// Toggle `checked[idx]` in place.
fn toggle_at(checked: &mut [bool], idx: usize) {
    if let Some(v) = checked.get_mut(idx) {
        *v = !*v;
    }
}

/// Flip every flag toward "all on" when anything is off, otherwise all off.
/// Returns the resulting state.
fn toggle_all(checked: &mut [bool]) -> bool {
    let any_off = checked.iter().any(|v| !v);
    for v in checked.iter_mut() {
        *v = any_off;
    }
    any_off
}

/// Left gutter for one rendered row: cursor marker + checkbox (Multi only).
fn row_prefix(mode: Mode, is_cursor: bool, is_checked: bool) -> String {
    let cursor = if is_cursor { "▶" } else { " " };
    match mode {
        Mode::Single => format!("{cursor} "),
        Mode::Multi => {
            let check = if is_checked { "[x]" } else { "[ ]" };
            format!("{cursor}{check} ")
        }
    }
}

// ─── TUI ────────────────────────────────────────────────────────────────────

fn run_browser(title: &str, entries: &[Entry], mode: Mode, prechecked: &[usize]) -> Outcome {
    let fallback = || match mode {
        Mode::Single => Outcome::Quit,
        Mode::Multi => Outcome::Toggled(vec![]),
    };
    if entries.is_empty() {
        return fallback();
    }

    let guard = match TerminalGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("error: cannot enter TUI mode: {e}");
            return fallback();
        }
    };

    let mut cursor: usize = 0;
    let mut checked: Vec<bool> = vec![false; entries.len()];
    for i in prechecked {
        if let Some(v) = checked.get_mut(*i) {
            *v = true;
        }
    }
    // Loop ends exactly once, via one of these:
    let mut confirmed = false;
    let mut cancelled = false;

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let Ok(mut terminal) = ratatui::Terminal::new(backend) else {
        drop(guard);
        return fallback();
    };

    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    while !confirmed && !cancelled {
        let _ = terminal.draw(|f| {
            let chunks = Layout::vertical([
                Constraint::Percentage(52),
                Constraint::Percentage(38),
                Constraint::Length(1),
            ])
            .split(f.area());

            let rows: Vec<Line> = entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let is_cursor = i == cursor;
                    let style = if is_cursor {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let prefix = row_prefix(mode, is_cursor, checked[i]);
                    Line::styled(format!("{prefix}{}", e.label), style)
                })
                .collect();
            let nav_title = format!(" {title} ");
            let nav = Paragraph::new(rows).block(
                Block::bordered()
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(nav_title),
            );
            f.render_widget(nav, chunks[0]);

            let detail: Vec<Line> = entries[cursor]
                .details
                .iter()
                .map(|s| Line::from(s.as_str()))
                .collect();
            let details = Paragraph::new(detail).block(
                Block::bordered()
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" details "),
            );
            f.render_widget(details, chunks[1]);

            let hints = match mode {
                Mode::Single => "↑↓ Navigate · Enter Select · q Cancel",
                Mode::Multi => "↑↓ Move · Space Toggle · a All/None · Enter Confirm · q Cancel",
            };
            f.render_widget(Paragraph::new(hints), chunks[2]);
        });

        let has_event =
            crossterm::event::poll(std::time::Duration::from_millis(33)).unwrap_or(false);
        if !has_event {
            continue;
        }
        let Ok(Event::Key(key)) = crossterm::event::read() else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl_c = matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Up => cursor = cursor.saturating_sub(1),
            KeyCode::Down => cursor = (cursor + 1).min(entries.len() - 1),
            KeyCode::End => cursor = entries.len() - 1,
            KeyCode::Enter => confirmed = true,
            KeyCode::Char('a' | 'A') if mode == Mode::Multi => {
                toggle_all(&mut checked);
            }
            KeyCode::Char(' ') if mode == Mode::Multi => {
                toggle_at(&mut checked, cursor);
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => cancelled = true,
            _ => {}
        }
        if ctrl_c {
            cancelled = true;
        }
    }

    drop(guard);

    if !cancelled {
        match mode {
            Mode::Single => return Outcome::Picked(cursor),
            Mode::Multi => {
                let picked = checked
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.then_some(i))
                    .collect();
                return Outcome::Toggled(picked);
            }
        }
    }
    fallback()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn toggles_are_local_and_all_flips_both_ways() {
        let mut c = vec![false, false, true];
        toggle_at(&mut c, 0);
        assert_eq!(c, vec![true, false, true]);
        assert!(toggle_all(&mut c)); // something was off → all on
        assert_eq!(c, vec![true, true, true]);
        assert!(!toggle_all(&mut c)); // nothing off → all off
        assert_eq!(c, vec![false, false, false]);
    }

    #[test]
    fn toggle_at_ignores_out_of_range() {
        let mut c = vec![true];
        toggle_at(&mut c, 7);
        assert_eq!(c, vec![true]);
    }

    #[test]
    fn prefixes_show_cursor_and_checkbox() {
        assert_eq!(row_prefix(Mode::Single, true, false), "▶ ");
        assert_eq!(row_prefix(Mode::Single, false, false), "  ");
        assert_eq!(row_prefix(Mode::Multi, true, true), "▶[x] ");
        assert_eq!(row_prefix(Mode::Multi, false, true), " [x] ");
        assert_eq!(row_prefix(Mode::Multi, false, false), " [ ] ");
    }
}
