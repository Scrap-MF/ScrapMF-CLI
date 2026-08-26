use inquire::{Select, Text};
use std::io::Write;

pub(super) fn clear_screen() {
    print!("\x1B[2J\x1B[H");
    let _ = std::io::stdout().flush();
    if theme::colors_enabled() {
        let name = anstyle::Style::new()
            .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Cyan)))
            .bold();
        let ver =
            anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Magenta)));
        println!(
            "{}  ▄▅▆▇ {} v{}{}{} — by MFApplications",
            anstyle::Reset.render(),
            name.render(),
            ver.render(),
            env!("CARGO_PKG_VERSION"),
            anstyle::Reset.render()
        );
    } else {
        println!(
            "  ▄▅▆▇ scrapmf v{} — by MFApplications",
            env!("CARGO_PKG_VERSION")
        );
    }
    println!("  ───────────────────────────────────────");
}

/// Standard option menu: clears the screen first (no stacked prompt residue)
/// and disables filter typing (option lists are short; stray keystrokes have
/// nothing to search). Chain `.with_help_message()` / `.with_default()` on
/// the returned builder before calling `.prompt()`.
pub(super) fn select_menu<'a, T: std::fmt::Display>(
    prompt: &'a str,
    options: Vec<T>,
) -> Select<'a, T> {
    clear_screen();
    Select::new(prompt, options)
        .without_filtering()
        .with_help_message("[↑↓ to move · enter to confirm]")
        .with_render_config(theme::render_config())
}

/// Run interactive prompt. All scrape flows (saved profile, URL(s), quick
/// scrape) run as internal batches via preview_and_execute. The home screen
/// is a ratatui browser; picking an entry drops to the plain terminal for
/// its inquire flow and returns to the browser afterwards.
pub fn run() {
    use home::Action;

    loop {
        let Some(action) = home::pick() else {
            println!("bye");
            return;
        };
        match action {
            Action::Scrape => {
                use browser::{Browser, Outcome};
                let outcome = Browser::new("Download content")
                    .entry(
                        "Saved profile",
                        vec![
                            "Run a saved scraping profile.".to_string(),
                            String::new(),
                            "Sites, cookies and output rules bundled".to_string(),
                            "for repeatable runs.".to_string(),
                        ],
                    )
                    .entry(
                        "URL(s)",
                        vec![
                            "Paste & go: one or more URLs.".to_string(),
                            "Site auto-detected per URL — no extra prompts.".to_string(),
                        ],
                    )
                    .entry(
                        "Quick scrape",
                        vec![
                            "One account, pick content kinds.".to_string(),
                            "Fastest path: username in, media out.".to_string(),
                        ],
                    )
                    .run();
                match outcome {
                    Outcome::Picked(0) => {
                        prompt_scrape_as_profile();
                        clear_screen();
                    }
                    // Single entry for 1..N URLs: paste, auto-match site by
                    // pattern, run — no per-run prompts ("paste and go").
                    Outcome::Picked(1) => {
                        scrape_flow::prompt_scrape_direct_urls();
                        clear_screen();
                    }
                    Outcome::Picked(2) => {
                        prompt_quick_scrape();
                        clear_screen();
                    }
                    _ => {}
                }
            }
            Action::Configuration => {
                configuration_submenu();
                clear_screen();
            }
            Action::Plugins => {
                plugins_menu::menu();
                clear_screen();
            }
            Action::Doctor => {
                let _ = crate::commands::doctor::run(1);
                std::thread::sleep(std::time::Duration::from_millis(800));
                clear_screen();
            }
            Action::Exit => {
                println!("bye");
                return;
            }
        }
    }
}

use profiles::prompt_scrape_as_profile;
use scrape_flow::prompt_quick_scrape;
use sites::configuration_submenu;

pub(crate) mod browser;
mod content;
pub(crate) mod home;
pub(crate) mod plugins_menu;
mod profiles;
mod scrape_flow;
mod sites;
pub(super) mod theme;

pub(super) fn ask_nonempty(prompt: &str) -> Option<String> {
    loop {
        let raw = Text::new(prompt).prompt().ok()?;
        let u = raw.trim().to_string();
        if !u.is_empty() {
            return Some(u);
        }
    }
}

/// Guided creation of a new profile: interactively collects accounts
/// (site, username) and returns a Profile ready to serialize.
/// Cookies source is NOT prompted — it lives in sites/*.toml and is
/// inherited via the account > site resolution precedence.
/// Site options come from sites/*.toml with instagram/tiktok fallbacks.
pub(super) fn edit_with_editor(path: &std::path::Path) {
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    if which::which(&editor).is_err() {
        eprintln!("error: editor '{editor}' not found in $PATH");
        return;
    }
    match std::process::Command::new(&editor).arg(path).status() {
        Ok(s) if s.success() => {
            // validate
            if let Ok(s) = std::fs::read_to_string(path) {
                // try both Site and Profile parse, one should succeed
                if toml::from_str::<crate::config::Site>(&s).is_ok()
                    || toml::from_str::<crate::config::Profile>(&s).is_ok()
                    || toml::from_str::<crate::config::Config>(&s).is_ok()
                {
                    println!("✔ Saved {}", path.display());
                } else {
                    eprintln!("warn: file saved but TOML parse failed, check syntax");
                }
            }
        }
        Ok(s) => eprintln!("editor exited with {:?}", s.code()),
        Err(e) => eprintln!("error: launch editor {editor}: {e}"),
    }
}

/// Helper to check if we are in interactive TTY context.
pub fn is_interactive() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}
