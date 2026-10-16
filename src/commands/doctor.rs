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

/// Report on every `sites/*.toml`.
///
/// Nothing else in the program looks at these after they are written. A file
/// the user edited into an unparseable state is skipped by every migration
/// (rebuilding it from defaults would erase it) and then quietly ignored, so
/// the downloads that depend on it fail with no explanation anywhere. This is
/// the only place that surfaces it.
///
/// A site file that will not parse is also the one input that could once take
/// the user's settings with it, so the parse error is shown verbatim: the
/// common causes are a type typo in `[rate_limit]` (`sleep = 3` where a string
/// is expected) and a missing quote.
fn check_site_configs(ok: &mut bool) -> Vec<CheckLine> {
    let Some(dir) = config::sites_dir() else {
        return Vec::new();
    };
    site_configs_in(&dir, ok)
}

fn site_configs_in(dir: &std::path::Path, ok: &mut bool) -> Vec<CheckLine> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();

    let mut broken = 0usize;
    for path in &files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        match std::fs::read_to_string(path) {
            Err(e) => {
                out.push(CheckLine {
                    level: Level::Error,
                    text: format!("sites/{name} — cannot read: {e}"),
                });
                *ok = false;
                broken += 1;
            }
            Ok(raw) => match toml::from_str::<config::Site>(&raw) {
                Err(e) => {
                    out.push(CheckLine {
                        level: Level::Error,
                        text: format!("sites/{name} — does not parse: {e}"),
                    });
                    out.push(CheckLine {
                        level: Level::Help,
                        text: "    fix the line above, or delete the file to regenerate it"
                            .to_string(),
                    });
                    *ok = false;
                    broken += 1;
                }
                Ok(site)
                    if site.site.is_none()
                        && site.pattern.is_none()
                        && site.extractor.is_empty() =>
                {
                    out.push(CheckLine {
                        level: Level::Error,
                        text: format!(
                            "sites/{name} — empty or identity-less (no site/pattern/extractor)"
                        ),
                    });
                    out.push(CheckLine {
                        level: Level::Help,
                        text: "    delete the file to regenerate it".to_string(),
                    });
                    *ok = false;
                    broken += 1;
                }
                Ok(_) => {}
            },
        }
    }

    if files.is_empty() {
        out.push(CheckLine {
            level: Level::Warn,
            text: format!("No site configs found in {}", dir.display()),
        });
    } else if broken == 0 {
        out.push(CheckLine {
            level: Level::Success,
            text: format!("Site configs: {} file(s) OK", files.len()),
        });
    }
    out
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

    out.extend(check_site_configs(&mut ok));

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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod site_config_tests {
    use super::{Level, site_configs_in};

    fn seed(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().expect("tempdir");
        for (name, body) in files {
            std::fs::write(dir.path().join(name), body).expect("seed");
        }
        dir
    }

    fn texts(lines: &[super::CheckLine]) -> String {
        lines
            .iter()
            .map(|l| format!("{:?}: {}", l.level, l.text))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn healthy_site_configs_pass() {
        let dir = seed(&[
            ("instagram.toml", "site = \"instagram\"\n"),
            (
                "vsco.toml",
                "site = \"vsco\"\nextractor = { \"vsco:gallery\" = {} }\n",
            ),
        ]);
        let mut ok = true;
        let lines = site_configs_in(dir.path(), &mut ok);

        assert!(ok, "healthy configs must not fail:\n{}", texts(&lines));
        assert_eq!(
            lines.iter().filter(|l| l.level == Level::Success).count(),
            1,
            "expected one summary line:\n{}",
            texts(&lines)
        );
    }

    /// The whole reason this check exists: a file the user broke by hand used
    /// to be invisible. This is the exact shape that used to destroy a config
    /// — `[rate_limit] sleep = 3` where the field is a String.
    #[test]
    fn unparseable_site_config_is_reported_and_fails() {
        let dir = seed(&[(
            "instagram.toml",
            "site = \"instagram\"\n\n[rate_limit]\nsleep = 3\n\n[extractor]\n",
        )]);
        let mut ok = true;
        let lines = site_configs_in(dir.path(), &mut ok);

        assert!(!ok, "a broken site config must fail the check");
        let out = texts(&lines);
        assert!(
            out.contains("instagram.toml") && out.contains("does not parse"),
            "must name the file and the problem:\n{out}"
        );
    }

    #[test]
    fn identity_less_site_config_is_reported() {
        // Parses cleanly, but site/pattern/extractor are all absent — the
        // shape a truncated or hand-stubbed file ends up in.
        let dir = seed(&[("instagram.toml", "extra_args = []\n")]);
        let mut ok = true;
        let lines = site_configs_in(dir.path(), &mut ok);

        assert!(!ok, "identity-less config must fail the check");
        assert!(
            texts(&lines).contains("identity-less"),
            "unexpected output:\n{}",
            texts(&lines)
        );
    }

    #[test]
    fn empty_dir_warns_without_failing() {
        let dir = seed(&[]);
        let mut ok = true;
        let lines = site_configs_in(dir.path(), &mut ok);

        assert!(ok, "no site configs is not a failure");
        assert!(
            texts(&lines).contains("No site configs"),
            "unexpected output:\n{}",
            texts(&lines)
        );
    }

    /// One broken file among healthy ones: both must be visible, and the
    /// summary must not claim everything is fine.
    #[test]
    fn one_broken_file_does_not_hide_the_healthy_ones() {
        let dir = seed(&[
            ("instagram.toml", "site = \"instagram\"\n"),
            ("tiktok.toml", "site = 42\n"),
        ]);
        let mut ok = true;
        let lines = site_configs_in(dir.path(), &mut ok);

        assert!(!ok);
        let out = texts(&lines);
        assert!(out.contains("tiktok.toml"), "broken file must be named");
        assert!(
            !out.contains("Site configs:"),
            "must not claim success while a file is broken:\n{out}"
        );
    }
}
