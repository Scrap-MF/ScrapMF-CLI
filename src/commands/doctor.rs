use anyhow::Result;

use crate::config;
use crate::output;
use crate::providers::gallery_dl::GalleryDl;
use crate::providers::{Provider, browser::detect_available_browsers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    Success,
    Info,
    Error,
    Help,
}

#[derive(Debug, Clone)]
pub struct CheckLine {
    pub level: Level,
    pub text: String,
}

/// Collect all checks as structured lines (used by both CLI and TUI).
pub fn collect(verbose: u8) -> (Vec<CheckLine>, bool) {
    let mut out: Vec<CheckLine> = Vec::new();
    let mut ok = true;

    // Check gallery-dl (resolved source: bundled pinned > overrides > system)
    let gallery = GalleryDl;
    let source = crate::application::backend::resolve(
        config::load()
            .unwrap_or_default()
            .backend
            .gallery_dl_path
            .clone(),
    );
    if gallery.is_available() {
        match gallery.version() {
            Ok(v) if !v.is_empty() => out.push(CheckLine {
                level: Level::Success,
                text: format!(
                    "gallery-dl {v} found [{}]{}",
                    source.label(),
                    if matches!(source, crate::application::backend::Source::Managed(_)) {
                        format!(" pinned v{}", crate::application::backend::GALLERY_DL_PIN)
                    } else {
                        String::new()
                    }
                ),
            }),
            Ok(_) | Err(_) => {
                out.push(CheckLine {
                    level: Level::Error,
                    text: "gallery-dl found but --version failed".to_string(),
                });
                ok = false;
            }
        }
    } else {
        out.push(CheckLine {
            level: Level::Error,
            text: "gallery-dl not found in $PATH".to_string(),
        });
        out.push(CheckLine {
            level: Level::Help,
            text: "run scrapmf (interactive) and it will offer to install the pinned backend; 'scrapmf setup' also works".to_string(),
        });
        ok = false;
    }

    // Check threadstractormf via the plugins system
    {
        use crate::plugins::PluginState;
        match crate::plugins::threads_state() {
            PluginState::Enabled(v) => out.push(CheckLine {
                level: Level::Success,
                text: format!("plugins: threads enabled (threadstractormf {v})"),
            }),
            PluginState::Disabled => out.push(CheckLine {
                level: Level::Info,
                text: "plugins: threads disabled — re-enable in scrapmf → Plugins (files kept)"
                    .to_string(),
            }),
            PluginState::NotInstalled => out.push(CheckLine {
                level: Level::Info,
                text: "plugins: threads not installed — optional; enable in scrapmf → Plugins"
                    .to_string(),
            }),
        }
    }

    // Check browsers for cookies
    let browsers = detect_available_browsers();
    let available: Vec<_> = browsers.iter().filter(|b| b.available).collect();
    if available.is_empty() {
        out.push(CheckLine {
            level: Level::Info,
            text: "No browser cookie DBs detected (checked: firefox, brave, chrome, chromium, edge, opera, vivaldi)".to_string(),
        });
        for b in &browsers {
            tracing::debug!(browser = %b.id, display = %b.display, "browser check");
        }
    } else {
        out.push(CheckLine {
            level: Level::Success,
            text: "Browsers with cookies:".to_string(),
        });
        for b in available {
            out.push(CheckLine {
                level: Level::Info,
                text: format!("  - {}", b.display),
            });
        }
    }

    // Keyring diagnostics — Arch pacman + kwallet por defecto, engloba futuros keyring/cifrados
    {
        let has_secret_tool = which::which("secret-tool").is_ok();
        let has_kwallet = which::which("kwallet-query").is_ok();
        out.push(CheckLine {
            level: if has_secret_tool {
                Level::Success
            } else {
                Level::Info
            },
            text: format!(
                "secret-tool (libsecret): {}",
                if has_secret_tool {
                    "found"
                } else {
                    "not found — pacman -S libsecret"
                }
            ),
        });
        out.push(CheckLine {
            level: if has_kwallet {
                Level::Success
            } else {
                Level::Info
            },
            text: format!(
                "kwallet-query (KDE): {}",
                if has_kwallet {
                    "found — KWallet active"
                } else {
                    "not found — optional, for KDE kwallet"
                }
            ),
        });
        if let Some(home) = dirs::home_dir() {
            let brave_path = home.join(".config/BraveSoftware/Brave-Browser/Default/Cookies");
            let brave_flatpak = home.join(
                ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser/Default/Cookies",
            );
            let found = brave_path.is_file() || brave_flatpak.is_file();
            let size = std::fs::metadata(&brave_path)
                .or_else(|_| std::fs::metadata(&brave_flatpak))
                .map(|m| m.len())
                .unwrap_or(0);
            out.push(CheckLine {
                level: if found { Level::Success } else { Level::Info },
                text: format!(
                    "Brave Cookies DB: {} ({} bytes){}",
                    if found { "found" } else { "not found" },
                    size,
                    if found {
                        ""
                    } else {
                        " — pacman: ~/.config/BraveSoftware/..., flatpak: ~/.var/app/..."
                    }
                ),
            });
        }
    }

    // Check resolved backend binary is reachable
    if crate::application::backend::gallery_dl_executable().is_ok() {
        tracing::debug!("backend resolution OK");
        if verbose > 0 {
            out.push(CheckLine {
                level: Level::Success,
                text: "Backend resolution OK".to_string(),
            });
        }
    }

    // Download archive stats (per-account JSONL files)
    if let Some(archive_dir) =
        crate::config::config_path().and_then(|p| p.parent().map(|b| b.join("archive")))
    {
        let mut files = 0usize;
        let mut entries = 0usize;
        if let Ok(rd) = std::fs::read_dir(&archive_dir) {
            for site in rd.flatten() {
                let site_path = site.path();
                if !site_path.is_dir() {
                    continue;
                }
                if let Ok(accounts) = std::fs::read_dir(site_path) {
                    for acc in accounts.flatten() {
                        if acc.path().extension().is_some_and(|e| e == "jsonl")
                            && let Ok(content) = std::fs::read_to_string(acc.path())
                        {
                            files += 1;
                            entries += content.lines().filter(|l| !l.trim().is_empty()).count();
                        }
                    }
                }
            }
        }
        if files == 0 {
            out.push(CheckLine {
                level: Level::Info,
                text: "Download archive: empty (dedup records appear after first scrape)"
                    .to_string(),
            });
        } else {
            out.push(CheckLine {
                level: Level::Success,
                text: format!(
                    "Download archive: {entries} media across {files} account(s) in {}",
                    archive_dir.display()
                ),
            });
        }
    }

    // Check temp dir writable
    let test_dir = std::env::temp_dir().join("scrapmf_doctor_test");
    match std::fs::create_dir_all(&test_dir) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&test_dir);
            out.push(CheckLine {
                level: Level::Success,
                text: format!("Temp dir writable: {}", test_dir.display()),
            });
        }
        Err(e) => {
            out.push(CheckLine {
                level: Level::Error,
                text: format!("Temp dir not writable: {e}"),
            });
            ok = false;
        }
    }

    if ok {
        out.push(CheckLine {
            level: Level::Success,
            text: "All checks passed".to_string(),
        });
    }
    (out, ok)
}

pub fn run(verbose: u8) -> Result<()> {
    tracing::debug!(verbose = verbose, "doctor start");
    println!("scrapmf doctor — checking backends and system");
    println!("─────────────────────────────────────────────");

    let (lines, ok) = collect(verbose);
    for line in &lines {
        match line.level {
            Level::Success => output::print_success(&line.text),
            Level::Info => output::print_info(&line.text),
            Level::Error => output::print_error(&line.text),
            Level::Help => output::print_help(&line.text),
        }
    }

    println!("─────────────────────────────────────────────");
    if ok {
        // "All checks passed" is already the last line from collect
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "some doctor checks failed (see details above)"
        ))
    }
}
