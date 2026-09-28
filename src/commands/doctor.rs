use anyhow::Result;

use crate::config;
use crate::output;
use crate::providers::gallery_dl::GalleryDl;
use crate::providers::{Provider, browser::detect_available_browsers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    Success,
    Info,
    Warn,
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
        match crate::plugins::termux_scan_state() {
            PluginState::Enabled(_) => out.push(CheckLine {
                level: Level::Success,
                text: "plugins: termux-scan enabled (termux-media-scan)".to_string(),
            }),
            PluginState::Disabled => out.push(CheckLine {
                level: Level::Info,
                text: "plugins: termux-scan disabled — enable in scrapmf → Plugins for Android gallery scan"
                    .to_string(),
            }),
            PluginState::NotInstalled => out.push(CheckLine {
                level: Level::Info,
                text: "plugins: termux-scan not installed (termux-media-scan not found) — install termux:api app + pkg install termux-api, then enable in Plugins".to_string(),
            }),
        }
    }

    // Check browsers for cookies
    let browsers = detect_available_browsers();
    let available: Vec<_> = browsers.iter().filter(|b| b.available).collect();
    if available.is_empty() {
        out.push(CheckLine {
            level: Level::Info,
            text: format!(
                "No browser cookie DBs detected (checked: {})",
                crate::browsers::CHANNELS
                    .iter()
                    .map(|c| c.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
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

    // Keyring diagnostics. Distribution-agnostic on purpose: the previous
    // wording hardcoded Arch pacman commands, which is wrong advice for a KDE
    // user whose actual problem was a locked or absent KWallet.
    {
        let has_secret_tool = which::which("secret-tool").is_ok();
        // `kwallet-query` is the legacy KDE 4 client; a modern Plasma 6 install
        // ships `kwalletctl6` and no legacy binary at all.
        let kwallet_clients: Vec<&str> = crate::browsers::kwallet_clients()
            .iter()
            .copied()
            .filter(|b| which::which(b).is_ok())
            .collect();
        out.push(CheckLine {
            level: if has_secret_tool {
                Level::Success
            } else {
                Level::Info
            },
            text: format!(
                "secret-tool (libsecret/GNOME): {}",
                if has_secret_tool {
                    "found"
                } else {
                    "not found (optional — needed for GNOME/libsecret keyrings)"
                }
            ),
        });
        out.push(CheckLine {
            level: if kwallet_clients.is_empty() {
                Level::Info
            } else {
                Level::Success
            },
            text: if kwallet_clients.is_empty() {
                "kwallet (KDE): no client found (optional — install kwallet if the browser uses it)"
                    .to_string()
            } else {
                format!("kwallet (KDE): {}", kwallet_clients.join(", "))
            },
        });
        // A v11 cookie DB cannot be decrypted without a D-Bus session. Report
        // the socket that was actually located rather than merely whether
        // DBUS_SESSION_BUS_ADDRESS is set: libdbus discovers the bus on its
        // own, so an unset variable does not mean a broken keyring.
        let bus = crate::config::cookies::resolve_dbus_address_for_doctor();
        out.push(CheckLine {
            level: if bus.is_some() {
                Level::Success
            } else {
                Level::Warn
            },
            text: match &bus {
                Some(addr) => format!("D-Bus session: reachable at {addr}"),
                None if crate::config::cookies::keyring_outside_namespace_for_doctor() => {
                    "D-Bus session: /run/user/<uid>/bus is NOT visible here (chroot, container \
                     or bare TTY) — the OS keyring runs in the host session, so browser cookies \
                     cannot be decrypted from in here"
                        .to_string()
                }
                None => "D-Bus session: no bus found — browser cookies that need the keyring \
                 cannot be decrypted (import them instead: Configuration → Cookie profiles)"
                    .to_string(),
            },
        });
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
            Level::Warn => output::print_warn(&line.text),
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
