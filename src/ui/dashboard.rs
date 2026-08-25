//! Live scrape execution dashboard (ratatui, alternate screen).
//!
//! Shows one row per job (label, files counter, state) plus an optional
//! scrolling log feed of gallery-dl stderr lines. Ctrl-C sets the shared
//! abort flag: gallery-dl children are killed and pending jobs are marked
//! cancelled. Nothing is persisted to disk.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const LOG_CAPACITY: usize = 200;
const LOG_VISIBLE: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Done(usize), // files downloaded
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct JobRow {
    pub label: String,
    pub files: usize,
    pub state: JobState,
}

/// Shared dashboard state — mutated by scrape hooks from the worker thread,
/// read by the render loop.
#[derive(Debug, Default)]
pub struct DashboardState {
    jobs: Vec<JobRow>,
    logs: VecDeque<String>,
}

impl DashboardState {
    pub fn new(labels: impl IntoIterator<Item = String>) -> Self {
        Self {
            jobs: labels
                .into_iter()
                .map(|label| JobRow {
                    label,
                    files: 0,
                    state: JobState::Pending,
                })
                .collect(),
            logs: VecDeque::new(),
        }
    }

    pub fn push_log(&mut self, line: impl Into<String>) {
        if self.logs.len() == LOG_CAPACITY {
            self.logs.pop_front();
        }
        self.logs.push_back(line.into());
    }

    pub fn set_running(&mut self, idx: usize) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.state = JobState::Running;
        }
    }

    pub fn add_file(&mut self, idx: usize) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.files += 1;
        }
    }

    /// Mark job done (with file count) or failed with a message.
    pub fn finish_ok(&mut self, idx: usize) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.state = JobState::Done(job.files);
        }
    }

    pub fn finish_failed(&mut self, idx: usize, message: impl Into<String>) {
        if let Some(job) = self.jobs.get_mut(idx) {
            job.state = JobState::Failed(message.into());
        }
    }

    /// Cancel the running job plus every pending one (Ctrl-C path).
    pub fn cancel_from(&mut self, idx: usize) {
        for job in self.jobs.iter_mut().skip(idx) {
            if matches!(job.state, JobState::Pending | JobState::Running) {
                job.state = JobState::Cancelled;
            }
        }
    }

    fn visible_logs(&self) -> impl Iterator<Item = &String> {
        self.logs.iter().rev().take(LOG_VISIBLE).rev()
    }
}

/// Terminal guard restoring the normal terminal on drop (panic-safe).
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn draw(f: &mut ratatui::Frame, state: &DashboardState, show_logs: bool) {
    use ratatui::{
        layout::{Constraint, Layout},
        style::{Color, Modifier, Style},
        text::Line,
        widgets::{Block, Borders, Paragraph, Wrap},
    };

    let chunks: Vec<ratatui::layout::Rect> = if show_logs {
        Layout::vertical([
            Constraint::Min(5),
            Constraint::Length(u16::try_from(LOG_VISIBLE + 2).unwrap_or(12)),
        ])
        .split(f.area())
        .to_vec()
    } else {
        Layout::vertical([Constraint::Percentage(100)])
            .split(f.area())
            .to_vec()
    };

    // Jobs panel
    let mut lines: Vec<Line> = Vec::new();
    for (i, job) in state.jobs.iter().enumerate() {
        let (icon, style) = match &job.state {
            JobState::Pending => (" ", Style::default().fg(Color::DarkGray)),
            JobState::Running => ("▶", Style::default().fg(Color::Cyan)),
            JobState::Done(_) => ("✔", Style::default().fg(Color::Green)),
            JobState::Failed(_) => ("✖", Style::default().fg(Color::Red)),
            JobState::Cancelled => ("⚠", Style::default().fg(Color::Yellow)),
        };
        let detail = match &job.state {
            JobState::Failed(msg) => format!(" — {msg}"),
            _ => String::new(),
        };
        let files = if matches!(job.state, JobState::Running | JobState::Done(_)) {
            format!(" · {} archivos", job.files)
        } else {
            String::new()
        };
        lines.push(Line::styled(
            format!(
                "{icon} [{}/{}] {}{files}{detail}",
                i + 1,
                state.jobs.len(),
                job.label
            ),
            style,
        ));
    }
    lines.push(Line::styled(
        "Ctrl-C: abortar",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ));
    let jobs_block = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" scrapmf "))
        .wrap(Wrap { trim: false });
    f.render_widget(jobs_block, chunks[0]);

    // Log panel (batch flow only) — color-coded by line prefix
    if show_logs {
        let log_lines: Vec<Line> = state
            .visible_logs()
            .map(|l| {
                let raw = l.as_str();
                if raw.starts_with('✔') {
                    Line::styled(format!("· {raw}"), Style::default().fg(Color::Green))
                } else if raw.contains("[warning]") || raw.starts_with('⚠') {
                    Line::styled(format!("· {raw}"), Style::default().fg(Color::Yellow))
                } else if raw.contains("[error]") {
                    Line::styled(format!("· {raw}"), Style::default().fg(Color::Red))
                } else {
                    Line::from(format!("· {raw}"))
                }
            })
            .collect();
        let log_block = Paragraph::new(log_lines)
            .block(Block::default().borders(Borders::ALL).title(" log "))
            .wrap(Wrap { trim: false });
        f.render_widget(log_block, chunks[1]);
    }
}

/// Run the blocking TUI dashboard while `worker` executes the batch.
///
/// The worker closure receives nothing; it interacts through the shared
/// `state` (hooks mutate it) and must respect `abort` (checked between
/// reads by the executor; on abort children are killed).
/// When the user presses Ctrl-C in the dashboard the abort flag is also set.
///
/// The alternate screen is entered/exited here; menus run outside.
pub fn run_dashboard<F, R>(
    state: Arc<Mutex<DashboardState>>,
    abort: Arc<AtomicBool>,
    show_logs: bool,
    worker: F,
) -> R
where
    F: FnOnce() -> R,
{
    // Reset any stale abort request from a previous run
    abort.store(false, Ordering::Relaxed);

    let guard = match TerminalGuard::enter() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("error: cannot enter TUI mode: {e}");
            return worker();
        }
    };

    let render_abort = abort.clone();
    let handle = std::thread::spawn(move || {
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
        let mut terminal = match ratatui::Terminal::new(backend) {
            Ok(t) => t,
            Err(_) => return,
        };
        loop {
            if render_abort.load(Ordering::Relaxed) {
                break;
            }
            let _ = terminal.draw(|f| {
                let Ok(snapshot) = state.lock() else {
                    return;
                };
                draw(f, &snapshot, show_logs);
            });
            // Poll keyboard: Ctrl-C sets abort
            let has_event =
                crossterm::event::poll(std::time::Duration::from_millis(33)).unwrap_or(false);
            if has_event && let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
                // Some terminals/layouts report Ctrl+C as 'C' — accept both.
                if matches!(key.code, crossterm::event::KeyCode::Char('c' | 'C'))
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    render_abort.store(true, Ordering::Relaxed);
                }
            }
        }
    });

    let result = worker();

    // Stop the render thread and wait for it before restoring the terminal
    abort.store(true, Ordering::Relaxed);
    let _ = handle.join();

    drop(guard);
    result
}
