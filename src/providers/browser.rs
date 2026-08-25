use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct BrowserInfo {
    pub id: &'static str,
    pub display: String,
    pub cookie_db: Option<PathBuf>,
    pub profile: Option<String>,
    pub available: bool,
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("~/.config"))
}

fn which_any(cands: &[&str]) -> Option<PathBuf> {
    cands.iter().find_map(|b| which::which(b).ok())
}

fn newest_cookie_db(root: &Path, filename: &str) -> Option<PathBuf> {
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == filename)
                && let Ok(md) = std::fs::metadata(&p)
                && md.len() > 0
            {
                let mtime = md.modified().unwrap_or(std::time::UNIX_EPOCH);
                if best.as_ref().is_none_or(|(_, t)| mtime > *t) {
                    best = Some((p, mtime));
                }
            }
        }
    }
    best.map(|(p, _)| p)
}

/// Factual one-line summary of a detected cookie DB. Never guesses the
/// cookie count (that would require decrypting the DB) — reports size instead.
fn cookie_db_summary(db: &Path) -> String {
    match std::fs::metadata(db) {
        Ok(md) if md.len() > 0 => {
            let kb = md.len() / 1024;
            format!("cookie DB ({kb} KB)")
        }
        _ => "empty cookie DB".to_string(),
    }
}

/// Detect available browsers for --cookies-from-browser
pub fn detect_available_browsers() -> Vec<BrowserInfo> {
    let mut out = Vec::new();

    // Firefox family
    {
        let id = "firefox";
        let bins = ["firefox", "firefox-developer-edition"];
        let bin = which_any(&bins);
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        let roots = [
            config_home().join("mozilla/firefox"),
            home.join(".mozilla/firefox"),
            home.join(".var/app/org.mozilla.firefox/.mozilla/firefox"),
            home.join(".var/app/org.mozilla.firefox/config/mozilla/firefox"),
            home.join("snap/firefox/common/.mozilla/firefox"),
        ];
        let mut db: Option<PathBuf> = None;
        let mut best_mtime = std::time::UNIX_EPOCH;
        for r in &roots {
            if !r.exists() {
                continue;
            }
            if let Some(cand) = newest_cookie_db(r, "cookies.sqlite")
                && let Ok(md) = std::fs::metadata(&cand)
                && let Ok(mtime) = md.modified()
                && mtime > best_mtime
            {
                best_mtime = mtime;
                db = Some(cand);
            }
        }
        let profile = db
            .as_ref()
            .and_then(|p| p.parent())
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let available = bin.is_some() && db.is_some() && db.as_ref().is_some_and(|p| p.is_file());
        let display = match db.as_ref() {
            Some(p) if available => format!(
                "firefox{} — {}",
                profile
                    .as_ref()
                    .map(|pr| format!(" ({pr})"))
                    .unwrap_or_default(),
                cookie_db_summary(p)
            ),
            _ => "firefox — no DB".to_string(),
        };
        out.push(BrowserInfo {
            id,
            display,
            cookie_db: db,
            profile,
            available,
        });
    }

    // Chromium family: brave, chrome, chromium, edge, opera, vivaldi
    let chromium_map: &[(&str, &[&str], &str, bool)] = &[
        (
            "brave",
            &["brave-browser", "brave", "brave-browser-stable"],
            "BraveSoftware/Brave-Browser",
            true,
        ),
        (
            "chrome",
            &["google-chrome", "google-chrome-stable", "chrome"],
            "google-chrome",
            true,
        ),
        (
            "chromium",
            &["chromium", "chromium-browser"],
            "chromium",
            true,
        ),
        (
            "edge",
            &[
                "microsoft-edge",
                "microsoft-edge-stable",
                "microsoft-edge-beta",
                "microsoft-edge-dev",
            ],
            "microsoft-edge",
            true,
        ),
        ("opera", &["opera", "opera-stable"], "opera", false),
        ("vivaldi", &["vivaldi", "vivaldi-stable"], "vivaldi", true),
    ];

    for (id, bins, rel, supports_profiles) in chromium_map {
        let bin = which_any(bins);
        let root = config_home().join(rel);
        // Brave also has Brave-Origin, check alternative
        let roots = if *id == "brave" {
            vec![
                config_home().join("BraveSoftware/Brave-Browser"),
                config_home().join("BraveSoftware/Brave-Origin"),
            ]
        } else {
            vec![root.clone()]
        };

        let mut db: Option<PathBuf> = None;
        let mut profile: Option<String> = None;
        for r in &roots {
            if !r.exists() {
                continue;
            }
            let cand = if *supports_profiles {
                // Prefer Default/Cookies, else newest
                let def = r.join("Default").join("Cookies");
                if def.is_file() {
                    Some(def)
                } else {
                    newest_cookie_db(r, "Cookies")
                }
            } else {
                let p = r.join("Cookies");
                if p.is_file() {
                    Some(p)
                } else {
                    newest_cookie_db(r, "Cookies")
                }
            };
            if let Some(c) = cand {
                let prof = if *supports_profiles
                    && c.parent()
                        .is_some_and(|d| d.file_name().is_some_and(|n| n == "Default"))
                {
                    Some("Default".to_string())
                } else {
                    c.parent()
                        .and_then(|d| d.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                };
                db = Some(c);
                profile = prof;
                break;
            }
        }
        let available = bin.is_some() && db.is_some() && db.as_ref().is_some_and(|p| p.is_file());
        let display = match db.as_ref() {
            Some(p) if available => format!(
                "{id}{} — {}",
                profile
                    .as_ref()
                    .map(|pr| format!(" ({pr})"))
                    .unwrap_or_default(),
                cookie_db_summary(p)
            ),
            _ => format!("{id} — not available"),
        };
        out.push(BrowserInfo {
            id,
            display,
            cookie_db: db,
            profile,
            available,
        });
    }

    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::detect_available_browsers;

    #[test]
    fn detect_runs_without_panic() {
        let browsers = detect_available_browsers();
        // Should always have at least firefox entry
        assert!(!browsers.is_empty());
        assert!(browsers.iter().any(|b| b.id == "firefox"));
    }

    #[test]
    fn available_if_binary_and_db() {
        let browsers = detect_available_browsers();
        for b in browsers {
            if b.available {
                assert!(b.cookie_db.is_some());
            }
        }
    }
}
