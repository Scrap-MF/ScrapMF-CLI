//! Unified decorated menu API — every selection goes through `Browser`
//! with the fixed chrome `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`.
//! Adding a new network/site/content kind requires no UI change: just
//! extend `sites::registry` and the menu inherits the chrome.

use crate::cli::interactive::browser::{Browser, Mode, Outcome};

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
/// Returns picked index or None on cancel.
pub fn pick_single(context: &str, options: Vec<(String, Vec<String>)>) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).mode(Mode::Single);
    for (label, details) in options {
        b = b.entry(label, details);
    }
    match b.run() {
        Outcome::Picked(i) => Some(i),
        _ => None,
    }
}

/// Multi-select decorated menu (Browser Multi). Returns picked indices.
pub fn pick_multi(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
) -> Option<Vec<usize>> {
    if options.is_empty() {
        return None;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).mode(Mode::Multi).checked(prechecked);
    for (label, details) in options {
        b = b.entry(label, details);
    }
    match b.run() {
        Outcome::Toggled(v) => Some(v),
        _ => None,
    }
}

/// Convenience for simple string options without details pane.
pub fn pick_single_labels(context: &str, labels: Vec<String>) -> Option<usize> {
    let opts = labels.into_iter().map(|l| (l, Vec::new())).collect();
    pick_single(context, opts)
}

/// Text input inside the Browser box `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`.
/// Returns the trimmed input or None on cancel (Esc/q/Ctrl-C or empty Enter).
pub fn input_text(context: &str, prompt: &str, placeholder: &str, help: &str) -> Option<String> {
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
            return inquire::Text::new(prompt)
                .with_placeholder(placeholder)
                .with_help_message(help)
                .prompt()
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
    };

    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let Ok(mut terminal) = ratatui::Terminal::new(backend) else {
        drop(guard);
        return None;
    };

    let mut input = String::new();
    let mut cursor: usize = 0; // char index
    let mut confirmed = false;
    let mut cancelled = false;

    while !confirmed && !cancelled {
        let _ = terminal.draw(|f| {
            let area = f.area();
            let chunks = Layout::vertical([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

            let block = Block::bordered()
                .border_set(ratatui::symbols::border::ROUNDED)
                .title(format!(" {title} "));
            f.render_widget(block, area);

            // Prompt line
            let prompt_line =
                Line::styled(format!("◆ {prompt}"), Style::default().fg(Color::Magenta));
            f.render_widget(Paragraph::new(vec![prompt_line]), chunks[0]);

            // Input line with cursor
            let display = if input.is_empty() && !placeholder.is_empty() {
                Line::styled(
                    format!("  {placeholder}"),
                    Style::default().fg(Color::DarkGray),
                )
            } else {
                // Render input with cursor as underline
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
            f.render_widget(Paragraph::new(vec![display]), chunks[1]);

            // Help
            if !help.is_empty() {
                f.render_widget(
                    Paragraph::new(Line::styled(help, Style::default().fg(Color::DarkGray))),
                    chunks[2],
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
            cancelled = true;
            continue;
        }
        match key.code {
            KeyCode::Enter => confirmed = true,
            KeyCode::Esc => cancelled = true,
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

    if cancelled {
        return None;
    }
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::chrome_title;

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
