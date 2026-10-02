//! Unified decorated menu API — every selection goes through `Browser`
//! with the fixed chrome `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`.
//! Adding a new network/site/content kind requires no UI change: just
//! extend `sites::registry` and the menu inherits the chrome.

use crate::cli::interactive::browser::{Browser, Mode, Outcome};

/// Outcome of one question in a multi-step flow.
///
/// `Option` conflated "step back" with "leave": a caller that received `None`
/// could not offer to go back, so Esc anywhere meant starting over. These
/// three states are what a wizard actually needs — confirm, go back one step,
/// or abandon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step<T> {
    /// The user confirmed this question.
    Value(T),
    /// Esc — return to the previous question, preserving earlier answers.
    Back,
    /// Ctrl+C — abandon the flow entirely.
    Cancel,
}

impl<T> Step<T> {
    /// Value if confirmed, otherwise None.
    pub fn value(self) -> Option<T> {
        match self {
            Step::Value(v) => Some(v),
            Step::Back | Step::Cancel => None,
        }
    }

    /// True when the user asked to go back rather than leave.
    pub fn is_back(&self) -> bool {
        matches!(self, Step::Back)
    }
}

/// Hint line describing the keys, shown in prompts that support going back.
pub fn key_hint(back: bool) -> String {
    if back {
        "↑↓ move · ⏎ confirm · esc back · ctrl+c cancel".to_string()
    } else {
        "⏎ confirm · esc cancel".to_string()
    }
}

/// Fixed chrome prefix — always SCRAPMF version.
fn chrome_title(context: &str) -> String {
    let ver = env!("CARGO_PKG_VERSION");
    let base = format!("SCRAPMF v{ver}");
    if context.is_empty() || context == base {
        base
    } else if context.starts_with("SCRAPMF") {
        context.to_string()
    } else {
        format!("{base} ─ {context}")
    }
}

/// Single-select decorated menu (Browser Single). `context` is shown in the
/// border title, e.g. "Download content" → `╭ SCRAPMF v1.7.0 ─ Download content ─╮`.
/// Returns the picked index, `Back` on Esc/q and `Cancel` on Ctrl+C.
pub fn pick_single(context: &str, options: Vec<(String, Vec<String>)>) -> Step<usize> {
    pick_single_inner(context, options, false)
}

/// As [`pick_single`], but advertises that Esc steps back.
pub fn pick_single_back(context: &str, options: Vec<(String, Vec<String>)>) -> Step<usize> {
    pick_single_inner(context, options, true)
}

fn pick_single_inner(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    back: bool,
) -> Step<usize> {
    if options.is_empty() {
        return Step::Cancel;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).mode(Mode::Single);
    if back {
        b = b.hint(key_hint(true));
    }
    for (label, details) in options {
        b = b.entry(label, details);
    }
    match b.run() {
        Outcome::Picked(i) => Step::Value(i),
        Outcome::Back => Step::Back,
        Outcome::Abandoned => Step::Cancel,
        Outcome::Toggled(_) | Outcome::Quit => Step::Cancel,
    }
}

/// Multi-select decorated menu (Browser Multi). Returns the picked indices,
/// `Back` on Esc/q and `Cancel` on Ctrl+C.
pub fn pick_multi(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
) -> Step<Vec<usize>> {
    pick_multi_inner(context, options, prechecked, false)
}

/// As [`pick_multi`], but advertises that Esc steps back.
pub fn pick_multi_back(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
) -> Step<Vec<usize>> {
    pick_multi_inner(context, options, prechecked, true)
}

fn pick_multi_inner(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
    back: bool,
) -> Step<Vec<usize>> {
    if options.is_empty() {
        return Step::Cancel;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).mode(Mode::Multi).checked(prechecked);
    if back {
        b = b.hint(key_hint(true));
    }
    for (label, details) in options {
        b = b.entry(label, details);
    }
    match b.run() {
        Outcome::Toggled(v) => Step::Value(v),
        Outcome::Back => Step::Back,
        Outcome::Abandoned => Step::Cancel,
        Outcome::Picked(_) | Outcome::Quit => Step::Cancel,
    }
}

/// Convenience for simple string options without details pane.
pub fn pick_single_labels(context: &str, labels: Vec<String>) -> Option<usize> {
    let opts = labels.into_iter().map(|l| (l, Vec::new())).collect();
    pick_single(context, opts).value()
}

/// Text input inside the Browser box `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`.
/// Returns `Back` on Esc/q, `Cancel` on Ctrl+C, and `Value` on a non-empty
/// submission. `initial` pre-fills the field so stepping back to fix a typo
/// shows what was typed instead of an empty box.
pub fn input_text(context: &str, prompt: &str, placeholder: &str, help: &str) -> Step<String> {
    input_text_inner(context, prompt, placeholder, help, "", false)
}

/// As [`input_text`], pre-filled with `initial` and advertising that Esc goes back.
pub fn input_text_back(
    context: &str,
    prompt: &str,
    placeholder: &str,
    help: &str,
    initial: &str,
) -> Step<String> {
    input_text_inner(context, prompt, placeholder, help, initial, true)
}

fn input_text_inner(
    context: &str,
    prompt: &str,
    placeholder: &str,
    help: &str,
    initial: &str,
    back: bool,
) -> Step<String> {
    use crate::ui::dashboard::TerminalGuard;
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::{
        layout::{Constraint, Layout},
        style::{Color, Style},
        text::Line,
        widgets::{Block, Paragraph},
    };

    let title = chrome_title(context);
    let guard = match TerminalGuard::enter() {
        Ok(g) => g,
        Err(_) => {
            // Fallback to plain inquire if TUI cannot be entered
            return match inquire::Text::new(prompt)
                .with_initial_value(initial)
                .with_placeholder(placeholder)
                .with_help_message(help)
                .prompt()
            {
                Ok(s) => {
                    let t = s.trim().to_string();
                    if t.is_empty() {
                        Step::Back
                    } else {
                        Step::Value(t)
                    }
                }
                Err(_) => Step::Cancel,
            };
        }
    };

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let Ok(mut terminal) = ratatui::Terminal::new(backend) else {
        drop(guard);
        return Step::Cancel;
    };

    let mut input = initial.to_string();
    let mut cursor: usize = input.chars().count();
    let mut confirmed = false;
    let mut went_back = false;
    let mut abandoned = false;

    while !confirmed && !went_back && !abandoned {
        let _ = terminal.draw(|f| {
            let area = f.area();
            let block = Block::bordered()
                .border_set(ratatui::symbols::border::ROUNDED)
                .title(format!(" {title} "));
            let inner = block.inner(area);
            f.render_widget(block, area);

            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);

            // Prompt line inside box
            let prompt_line =
                Line::styled(format!("◆ {prompt}"), Style::default().fg(Color::Magenta));
            f.render_widget(Paragraph::new(vec![prompt_line]), chunks[0]);
            f.render_widget(
                Paragraph::new(Line::styled(
                    "─".repeat(inner.width as usize),
                    Style::default().fg(Color::DarkGray),
                )),
                chunks[1],
            );

            // Input line with cursor — wrap placeholder/input to avoid cutting
            let display = if input.is_empty() && !placeholder.is_empty() {
                Line::styled(
                    format!("  {placeholder}"),
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                let before: String = input.chars().take(cursor).collect();
                let at = input.chars().nth(cursor).unwrap_or(' ');
                let after: String = input.chars().skip(cursor + 1).collect();
                if cursor < input.chars().count() {
                    Line::from(vec![
                        ratatui::text::Span::raw(format!("  {before}")),
                        ratatui::text::Span::styled(
                            at.to_string(),
                            Style::default().bg(Color::White).fg(Color::Black),
                        ),
                        ratatui::text::Span::raw(after),
                    ])
                } else {
                    Line::from(vec![
                        ratatui::text::Span::raw(format!("  {input}")),
                        ratatui::text::Span::styled(
                            " ",
                            Style::default().bg(Color::White).fg(Color::Black),
                        ),
                    ])
                }
            };
            f.render_widget(
                Paragraph::new(vec![display]).wrap(ratatui::widgets::Wrap { trim: true }),
                chunks[2],
            );

            // Help inside box bottom. When the screen supports going back, say
            // so — an Esc that silently jumps to the previous question would
            // otherwise look like the run was cancelled.
            let help_text = if back {
                let mut h = String::new();
                if !help.is_empty() {
                    h.push_str(help);
                    h.push_str("  ·  ");
                }
                h.push_str(&key_hint(true));
                h
            } else {
                help.to_string()
            };
            if !help_text.is_empty() {
                f.render_widget(
                    Paragraph::new(Line::styled(
                        help_text,
                        Style::default().fg(Color::DarkGray),
                    ))
                    .wrap(ratatui::widgets::Wrap { trim: true }),
                    chunks[3],
                );
            }
        });

        if !crossterm::event::poll(std::time::Duration::from_millis(50)).unwrap_or(false) {
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
        if ctrl_c {
            abandoned = true;
            continue;
        }
        match key.code {
            KeyCode::Enter => confirmed = true,
            KeyCode::Esc => went_back = true,
            KeyCode::Backspace => {
                if cursor > 0 {
                    let mut chars: Vec<char> = input.chars().collect();
                    chars.remove(cursor - 1);
                    input = chars.into_iter().collect();
                    cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let mut chars: Vec<char> = input.chars().collect();
                if cursor < chars.len() {
                    chars.remove(cursor);
                    input = chars.into_iter().collect();
                }
            }
            KeyCode::Left => cursor = cursor.saturating_sub(1),
            KeyCode::Right => {
                let len = input.chars().count();
                if cursor < len {
                    cursor += 1;
                }
            }
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = input.chars().count(),
            KeyCode::Char(c) => {
                let mut chars: Vec<char> = input.chars().collect();
                chars.insert(cursor, c);
                input = chars.into_iter().collect();
                cursor += 1;
            }
            _ => {}
        }
    }

    drop(guard);

    if abandoned {
        return Step::Cancel;
    }
    if went_back {
        return Step::Back;
    }
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        // An empty submission is not an answer; treat it as "step back" so the
        // user is not stranded on a screen with nothing typed.
        Step::Back
    } else {
        Step::Value(trimmed)
    }
}

/// Generic nonempty input inside the box — loops until nonempty or cancel.
pub fn ask_nonempty(context: &str, prompt: &str) -> Option<String> {
    loop {
        let s = input_text(context, prompt, "", "").value()?;
        let t = s.trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Step, chrome_title, key_hint};

    #[test]
    fn step_separates_back_from_cancel() {
        // The whole point: these two used to be the same `None`, which is why Esc
        // anywhere in a flow meant starting over.
        assert_ne!(Step::<u8>::Back, Step::<u8>::Cancel);
        assert!(Step::<u8>::Back.is_back());
        assert!(!Step::<u8>::Cancel.is_back());
        assert!(!Step::Value(1).is_back());
    }

    #[test]
    fn value_carries_the_answer() {
        assert_eq!(Step::Value(7).value(), Some(7));
        assert_eq!(Step::<u8>::Back.value(), None);
        assert_eq!(Step::<u8>::Cancel.value(), None);
    }

    #[test]
    fn hint_advertises_back_only_when_supported() {
        assert!(key_hint(true).contains("esc back"));
        assert!(!key_hint(false).contains("back"));
    }

    #[test]
    fn chrome_title_formats() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(chrome_title(""), format!("SCRAPMF v{v}"));
        assert_eq!(
            chrome_title("Download content"),
            format!("SCRAPMF v{v} ─ Download content")
        );
        assert_eq!(
            chrome_title(&format!("SCRAPMF v{v}")),
            format!("SCRAPMF v{v}")
        );
    }
}
