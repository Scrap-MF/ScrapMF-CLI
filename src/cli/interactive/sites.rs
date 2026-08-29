use super::profiles::{edit_profile_menu, prompt_new_profile_accounts};
use super::{ask_nonempty, clear_screen, edit_with_editor, select_menu, theme::render_config};
use std::path::Path;

use inquire::{Confirm, Text};

pub(super) fn configuration_submenu() {
    use crate::cli::interactive::browser::{Browser, Outcome};

    // Live data for the right pane
    let sites: Vec<String> = crate::config::sites_dir()
        .and_then(|dir| std::fs::read_dir(&dir).ok())
        .map(|rd| {
            let mut v: Vec<String> = rd
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
                .filter_map(|e| {
                    e.path()
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .map(String::from)
                })
                .collect();
            // plugin gating mirrors the menus that consume these entries
            if !crate::plugins::threads_enabled() {
                v.retain(|n| n != "threads");
            }
            v.sort();
            v
        })
        .unwrap_or_default();
    let profiles_count = crate::config::profiles_dir()
        .and_then(|dir| std::fs::read_dir(&dir).ok())
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    let cfg = crate::config::load().unwrap_or_default();

    loop {
        let outcome = Browser::new("Configuration")
            .entry(
                "Cookie profiles",
                vec![
                    "Named cookie sets captured from your browser.".to_string(),
                    "Used by accounts to access private content.".to_string(),
                ],
            )
            .entry("Manage Sites", {
                let mut d = vec!["Site configs (editable with $EDITOR):".to_string()];
                if sites.is_empty() {
                    d.push("  (none found)".to_string());
                }
                d.extend(sites.iter().map(|s| format!("  · {s}")));
                d
            })
            .entry(
                "Manage Profiles",
                vec![format!("{profiles_count} saved scraping profile(s).")],
            )
            .entry(
                "General settings",
                vec![
                    format!(
                        "output dir : {}",
                        crate::config::expand_output_dir(&cfg.general.output_dir).display()
                    ),
                    format!(
                        "archive    : {}",
                        if cfg.general.archive { "on" } else { "off" }
                    ),
                ],
            )
            .entry("Back", vec!["Return to the main menu.".to_string()])
            .run();

        let choice = match outcome {
            Outcome::Picked(i) => i,
            _ => {
                clear_screen();
                return;
            }
        };
        match choice {
            0 => {
                cookie_profiles_menu();
                clear_screen();
            }
            1 => {
                manage_sites();
                clear_screen();
            }
            2 => {
                manage_profiles();
                clear_screen();
            }
            3 => {
                general_settings_menu();
                clear_screen();
            }
            _ => {
                clear_screen();
                return;
            }
        }
    }
}

fn general_settings_menu() {
    loop {
        let cfg = crate::config::load().unwrap_or_default();
        let cfg_path = crate::config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let archive_label = if cfg.general.archive {
            "enabled"
        } else {
            "disabled"
        };
        let options: Vec<String> = vec![
            format!("Output directory: {}", cfg.general.output_dir.display()),
            format!("Download archive: {archive_label}"),
            "Show config path".to_string(),
            "Edit raw config.toml ($EDITOR)".to_string(),
            "Back".to_string(),
        ];
        let opts: Vec<(String, Vec<String>)> = options
            .iter()
            .map(|o| {
                let details = if o.starts_with("Output directory:") {
                    vec![format!("current: {}", cfg.general.output_dir.display())]
                } else if o.starts_with("Download archive:") {
                    vec![format!("archive is {archive_label}")]
                } else {
                    Vec::new()
                };
                (o.clone(), details)
            })
            .collect();
        let Some(idx) =
            crate::cli::interactive::menu::pick_single("Configuration ─ General settings", opts)
        else {
            return;
        };
        let choice = options[idx].clone();
        if choice.starts_with("Output directory:") {
            let current = cfg.general.output_dir.display().to_string();
            let Ok(new_val) = Text::new("Output directory:")
                .with_default(&current)
                .with_help_message("e.g. ~/scrapmf, ~/Pictures/scrapmf — tilde ~/ is expanded")
                .prompt()
            else {
                continue;
            };
            let new_val = new_val.trim().to_string();
            if new_val.is_empty() || new_val == current {
                continue;
            }
            let new_path = std::path::PathBuf::from(&new_val);
            if let Err(e) = crate::application::scraper::validate_output_path(&new_path) {
                println!("✖ invalid path: {e}");
                std::thread::sleep(std::time::Duration::from_millis(1500));
                continue;
            }
            let expanded = crate::config::expand_output_dir(&new_path);
            if let Some(home) = dirs::home_dir()
                && expanded == home
            {
                println!(
                    "✖ output directory cannot be $HOME itself — use a subdirectory like ~/scrapmf"
                );
                std::thread::sleep(std::time::Duration::from_millis(1500));
                continue;
            }
            let mut new_cfg = cfg.clone();
            new_cfg.general.output_dir = new_path;
            match crate::config::save(&new_cfg) {
                Ok(()) => println!("✔ saved → {cfg_path}"),
                Err(e) => println!("✖ save failed: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
        } else if choice.starts_with("Download archive:") {
            let new_val = !cfg.general.archive;
            let label = if new_val { "enabled" } else { "disabled" };
            let prompt = format!("Turn download archive {label}?");
            let ok = Confirm::new(&prompt)
                .with_default(true)
                .with_render_config(super::theme::render_config())
                .prompt()
                .unwrap_or(false);
            if !ok {
                continue;
            }
            let mut new_cfg = cfg.clone();
            new_cfg.general.archive = new_val;
            match crate::config::save(&new_cfg) {
                Ok(()) => println!("✔ archive {label} → {cfg_path}"),
                Err(e) => println!("✖ save failed: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(1000));
        } else if choice == "Show config path" {
            println!("{cfg_path}");
            let _ = Text::new("Press enter to continue").prompt();
        } else if choice == "Edit raw config.toml ($EDITOR)" {
            if let Some(p) = crate::config::config_path() {
                // Ensure file exists before opening editor
                let _ = crate::config::ensure_default_config();
                edit_with_editor(&p);
            }
        } else {
            return;
        }
    }
}

/// Manage named cookie profiles: capture from Firefox, import pasted
/// Netscape text, list with live summaries, delete.
/// Wizard: pick browser + networks, capture their session cookies into a
/// new named profile. Firefox uses direct SQLite reads; Chromium-family
/// browsers (Brave/Chrome/…) use keyring-based decryption.
fn create_profile_wizard() {
    use crate::config::cookies;
    let browsers: Vec<String> = vec![
        "Brave".into(),
        "Firefox".into(),
        "Chrome".into(),
        "Chromium".into(),
        "Edge".into(),
        "Vivaldi".into(),
        "Opera".into(),
    ];
    let Some(b_idx) = crate::cli::interactive::menu::pick_single(
        "Configuration ─ Cookie capture ─ Browser",
        browsers
            .iter()
            .map(|b| (b.clone(), vec![format!("capture from {b}")]))
            .collect(),
    ) else {
        return;
    };
    let browser = browsers[b_idx].clone();

    // Network multi-select with an "All" shortcut. Plugin-backed networks
    // (threads) only appear while their plugin is enabled.
    let mut networks: Vec<&str> = vec!["instagram", "tiktok", "twitter", "vsco"];
    if crate::plugins::threads_enabled() {
        networks.push("threads");
    }
    let net_opts: Vec<(String, Vec<String>)> = {
        let mut v = vec![("All networks".to_string(), vec!["select all".to_string()])];
        v.extend(networks.iter().map(|s| {
            (
                super::theme::brand_site_label(s),
                vec![format!("domain: {}", s)],
            )
        }));
        v
    };
    let Some(picked_idxs) = crate::cli::interactive::menu::pick_multi(
        "Configuration ─ Cookie capture ─ Networks",
        net_opts.clone(),
        &[],
    ) else {
        return;
    };
    let picked_raw: Vec<String> = picked_idxs
        .into_iter()
        .filter_map(|i| net_opts.get(i).map(|(l, _)| l.clone()))
        .collect();
    if picked_raw.is_empty() {
        println!("ℹ No networks selected");
        return;
    }
    // Labels carry ANSI; map back to clean keys via brand_site_label output.
    let mut sites: Vec<String> = Vec::new();
    for picked in &picked_raw {
        for net in &networks {
            if super::theme::brand_site_label(net) == *picked || picked == "All networks" {
                sites.push((*net).to_string());
                break;
            }
        }
    }
    sites.sort();
    sites.dedup();

    let suggested = format!("{}-{}", browser.to_lowercase(), sites.join("-"));
    let Some(name) = Text::new("Profile name:")
        .with_default(&suggested)
        .prompt()
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return;
    };

    let result = if browser.eq_ignore_ascii_case("firefox") {
        cookies::capture_firefox(&sites, &name)
    } else {
        cookies::capture_chromium(&browser, &sites, &name)
    };
    match result {
        Ok((path, count)) => {
            println!("✔ {count} cookie(s) → {}", path.display());
            println!("⚠ Cookie files grant account access — never share them.");
        }
        Err(e) => println!("✖ {e}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(2200));
}

fn cookie_profiles_menu() {
    use crate::config::cookies;
    loop {
        clear_screen();
        let count = cookies::list_profiles().len();
        let browse_label = if count == 0 {
            "Browse profiles".to_string()
        } else {
            format!("Browse profiles ({count})")
        };
        let choices: Vec<String> = vec![
            browse_label,
            "Create profile — capture from browser".into(),
            "Import profile — from file or paste".into(),
            "Back".into(),
        ];
        let choice = match select_menu("Cookie profiles", choices).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        if choice.starts_with("Browse profiles") {
            browse_profiles_menu();
        } else if choice.starts_with("Create profile") {
            create_profile_wizard();
        } else if choice.starts_with("Import profile") {
            import_profile_submenu();
        } else {
            return;
        }
    }
}

fn browse_profiles_menu() {
    use crate::config::cookies;
    loop {
        let names = cookies::list_profiles();
        if names.is_empty() {
            println!("ℹ No profiles yet — use Create or Import first");
            std::thread::sleep(std::time::Duration::from_millis(1200));
            return;
        }
        let rows: Vec<String> = names
            .iter()
            .map(|name| match cookies::profile_summary(name) {
                Ok(summary) => format!("{name}  — {summary}"),
                Err(e) => format!("{name}  — ⚠ {e}"),
            })
            .collect();
        let Ok(picked_row) = select_menu("Browse profiles — select one", rows).prompt() else {
            return;
        };
        let name = picked_row.split("  — ").next().unwrap_or("").to_string();
        if !name.is_empty() {
            profile_detail_menu(&name);
        }
    }
}

fn profile_detail_menu(name: &str) {
    use crate::config::cookies;
    loop {
        clear_screen();
        // Header with current summary
        let summary = match cookies::profile_summary(name) {
            Ok(s) => s,
            Err(e) => format!("⚠ {e}"),
        };
        let path = cookies::profile_path(name)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("  ◆ {name}");
        println!("    {summary}");
        println!("    {path}");
        println!();
        let choices: Vec<String> = vec![
            "View details".into(),
            "Refresh — re-capture from browser".into(),
            "Delete profile".into(),
            "Back".into(),
        ];
        let choice = match select_menu(&format!("Profile: {name}"), choices).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "View details" => {
                clear_screen();
                println!("  ◆ {name} — details");
                println!("  ───────────────────────────────────────");
                println!("  {summary}");
                println!("  File: {path}");
                if let Some(p) = cookies::profile_path(name)
                    && let Ok(content) = std::fs::read_to_string(p)
                {
                    if let Some(meta) = cookies::parse_source_metadata(&content) {
                        println!(
                            "  Source: {} — networks: {}",
                            meta.browser,
                            meta.networks.join(", ")
                        );
                    } else {
                        println!("  Source: manual import (no browser metadata)");
                    }
                }
                let _ = Text::new("Press enter to continue").prompt();
            }
            "Refresh — re-capture from browser" => {
                println!("→ Re-capturing '{name}'…");
                match cookies::refresh_profile(name) {
                    Ok(cookies::Refresh::Done { path, count }) => {
                        println!("✔ refreshed {count} cookie(s) → {}", path.display());
                        println!("✔ accounts using this profile need no changes");
                        let _ = Text::new("Press enter to continue").prompt();
                    }
                    Ok(cookies::Refresh::ManualImportRequired) => {
                        println!(
                            "ℹ This profile was imported manually — it has no browser to re-capture from."
                        );
                        println!(
                            "  To refresh, re-export with 'Get cookies.txt LOCALLY' while logged in,"
                        );
                        println!("  then use Import profile → From file / Paste.");
                        let go_import = Confirm::new("Open Import menu now?")
                            .with_default(false)
                            .with_render_config(super::theme::render_config())
                            .prompt()
                            .unwrap_or(false);
                        if go_import {
                            import_profile_submenu_prefilled(name);
                        } else {
                            let _ = Text::new("Press enter to continue").prompt();
                        }
                    }
                    Err(e) => {
                        println!("✖ refresh failed: {e}");
                        println!(
                            "  tip: is the browser closed? Try again, or re-create with Create profile"
                        );
                        let _ = Text::new("Press enter to continue").prompt();
                    }
                }
            }
            "Delete profile" => {
                let ok = Confirm::new(&format!("Delete profile '{name}'?"))
                    .with_default(false)
                    .with_render_config(super::theme::render_config())
                    .prompt()
                    .unwrap_or(false);
                if ok {
                    match cookies::delete_profile(name) {
                        Ok(true) => {
                            println!("✔ deleted");
                            std::thread::sleep(std::time::Duration::from_millis(800));
                            return;
                        }
                        _ => {
                            println!("✖ not found");
                            std::thread::sleep(std::time::Duration::from_millis(800));
                        }
                    }
                }
            }
            _ => return,
        }
    }
}

fn import_profile_submenu() {
    import_profile_submenu_prefilled("")
}

fn import_profile_submenu_prefilled(default_name: &str) {
    loop {
        clear_screen();
        let choices: Vec<String> = vec![
            "Import from file — select cookies.txt".into(),
            "Import from paste — open $EDITOR".into(),
            "Back".into(),
        ];
        let choice = match select_menu("Import profile", choices).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "Import from file — select cookies.txt" => {
                import_from_file_flow(default_name);
            }
            "Import from paste — open $EDITOR" => {
                import_from_paste_flow(default_name);
            }
            _ => return,
        }
    }
}

fn import_from_file_flow(default_name: &str) {
    use crate::config::cookies;
    let Some(name) = (if default_name.is_empty() {
        ask_nonempty("Profile name:")
    } else {
        let input = Text::new("Profile name:")
            .with_default(default_name)
            .prompt()
            .ok()
            .map(|s| s.trim().to_string());
        input.filter(|s| !s.is_empty())
    }) else {
        return;
    };
    let Some(src) = Text::new("Path to cookies.txt file:")
        .with_help_message("e.g. /tmp/cookies.txt or ~/Downloads/cookies.txt")
        .prompt()
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let src_path = Path::new(&src);
    let src_expanded = if let Some(stripped) = src.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(stripped)
        } else {
            src_path.to_path_buf()
        }
    } else {
        src_path.to_path_buf()
    };
    let content = match std::fs::read_to_string(&src_expanded) {
        Ok(c) => c,
        Err(e) => {
            println!("✖ cannot read {}: {e}", src_expanded.display());
            std::thread::sleep(std::time::Duration::from_millis(1500));
            return;
        }
    };
    match cookies::save_profile(&name, &content) {
        Ok(path) => {
            let count = cookies::load_profile(&name).map(|v| v.len()).unwrap_or(0);
            println!("✔ imported {count} cookie(s) → {}", path.display());
        }
        Err(e) => {
            println!("✖ invalid cookies file: {e}");
            println!("  Expected Netscape format (7 tab-separated fields).");
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(1800));
}

fn import_from_paste_flow(default_name: &str) {
    use crate::config::cookies;
    let Some(name) = (if default_name.is_empty() {
        ask_nonempty("Profile name:")
    } else {
        let input = Text::new("Profile name:")
            .with_default(default_name)
            .prompt()
            .ok()
            .map(|s| s.trim().to_string());
        input.filter(|s| !s.is_empty())
    }) else {
        return;
    };
    let path = match cookies::profile_path(&name) {
        Some(p) => p,
        None => return,
    };
    let template = "# Netscape HTTP Cookie File\n# Paste your exported cookies below.\n# Get them with the 'Get cookies.txt LOCALLY' extension while logged in.\n# 1. Install the extension in your browser\n# 2. Log in to the site (instagram.com, tiktok.com, etc.)\n# 3. Click the extension → Export → Copy\n# 4. Paste below this header, save and close the editor.\n\n";
    // Only write template if file doesn't exist, to avoid overwriting existing profile
    if !path.exists() {
        let _ = std::fs::write(&path, template);
    } else {
        // Ensure header exists for guidance
        if let Ok(existing) = std::fs::read_to_string(&path)
            && !existing.contains("Netscape HTTP Cookie File")
        {
            let _ = std::fs::write(&path, format!("{template}{existing}"));
        }
    }
    edit_with_editor(&path);
    match cookies::load_profile(&name) {
        Ok(list) => println!("✔ saved {} cookie(s)", list.len()),
        Err(e) => println!("✖ invalid profile ({e}) — delete or fix {path:?}"),
    }
    std::thread::sleep(std::time::Duration::from_millis(1800));
}

pub(super) fn site_submenu(name: &str, dir: &Path) {
    let path = dir.join(format!("{name}.toml"));
    loop {
        clear_screen();
        let options: Vec<String> = vec![
            "Edit".to_string(),
            "Delete site".to_string(),
            "Back".to_string(),
        ];
        let choice = match select_menu(&format!("Site: {name}"), options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        if choice == "Edit" {
            if path.exists() {
                edit_with_editor(&path);
            } else {
                eprintln!("error: {name}.toml no longer exists");
                return;
            }
        } else if choice == "Delete site" {
            if Confirm::new(&format!("Delete {name}.toml?"))
                .with_render_config(render_config())
                .with_default(false)
                .prompt()
                .unwrap_or(false)
            {
                let _ = std::fs::remove_file(&path);
                println!("✔ Deleted {name}");
                return;
            }
        } else {
            return;
        }
    }
}

pub(super) fn manage_sites() {
    let dir = match crate::config::sites_dir() {
        Some(d) => d,
        None => {
            eprintln!("error: cannot determine sites dir");
            return;
        }
    };
    let _ = std::fs::create_dir_all(&dir);
    loop {
        clear_screen();
        let mut entries: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if let Some(name) = e.path().file_stem().and_then(|n| n.to_str())
                    && e.path().extension().is_some_and(|ext| ext == "toml")
                {
                    entries.push(name.to_string());
                }
            }
        }
        entries.sort();
        // Plugin gating: plugin-backed sites only show when enabled.
        if !crate::plugins::threads_enabled() {
            entries.retain(|n| n != "threads");
        }
        use crate::cli::interactive::theme::SiteItem;
        let mut options: Vec<SiteItem> = vec![SiteItem::new("Create new site")];
        options.extend(entries.iter().map(|e| SiteItem::new(e.clone())));
        options.push(SiteItem::new("Back"));

        let choice = match select_menu("Manage Sites", options).prompt() {
            Ok(c) => c.key(),
            Err(_) => return,
        };
        if choice == "Back" {
            return;
        }
        if choice == "Create new site" {
            let name = match Text::new("Site name (filename without .toml):")
                .with_render_config(render_config())
                .with_placeholder("instagram")
                .with_help_message("use [a-z0-9_-], e.g. instagram, twitter, pixiv")
                .prompt()
            {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };
            if name.is_empty() || name.contains('/') || name.contains('.') {
                eprintln!("error: invalid name");
                continue;
            }
            let path = dir.join(format!("{name}.toml"));
            if path.exists() {
                eprintln!("error: {name}.toml already exists, choose Edit");
                continue;
            }
            // create minimal template with comments then open editor
            if let Err(e) = crate::config::ensure_example_sites() {
                // ensure at least dir exists, then create empty file
                let _ = e;
            }
            // If it's instagram name we already have template, otherwise create generic
            if !path.exists() {
                if name == "tiktok" {
                    let minimal = "# scrapmf site — tiktok minimal\n# File: ~/.config/scrapmf/sites/tiktok.toml (0o600, dir 0o700)\n# pattern auto-matches https://www.tiktok.com/@user\nsite = \"tiktok\"\npattern = \"tiktok.com\"\n";
                    let _ = std::fs::write(&path, minimal);
                } else {
                    let generic = format!(
                        "# scrapmf site — {name}\n# See sites/instagram.toml for all options\nsite = \"{name}\"\npattern = \"{name}.com\"\n"
                    );
                    let _ = std::fs::write(&path, generic);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
            }
            edit_with_editor(&path);
        } else if entries.contains(&choice) {
            // Selected an existing site — open its submenu
            site_submenu(&choice, &dir);
        }
    }
}

pub(super) fn manage_profiles() {
    let dir = match crate::config::profiles_dir() {
        Some(d) => d,
        None => {
            eprintln!("error: cannot determine profiles dir");
            return;
        }
    };
    let _ = std::fs::create_dir_all(&dir);
    loop {
        clear_screen();
        let mut entries: Vec<String> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if let Some(name) = e.path().file_stem().and_then(|n| n.to_str())
                    && e.path().extension().is_some_and(|ext| ext == "toml")
                {
                    entries.push(name.to_string());
                }
            }
        }
        entries.sort();
        let mut options: Vec<String> = Vec::new();
        options.push("Create new profile".to_string());
        // Entries show account counts inline (replaces the old "List all")
        for n in &entries {
            let p = dir.join(format!("{n}.toml"));
            let suffix = match std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| toml::from_str::<crate::config::Profile>(&s).ok())
            {
                Some(prof) => {
                    let total: usize = prof.accounts.values().map(|v| v.len()).sum();
                    format!(" — {} site(s), {} account(s)", prof.accounts.len(), total)
                }
                None => " — parse error".to_string(),
            };
            options.push(format!("{n}{suffix}"));
        }
        options.push("Back".to_string());

        // Strip the count suffix so a bare entry name can be matched
        let selected = match select_menu("Manage Profiles", options).prompt() {
            Ok(c) => c.split(" — ").next().unwrap_or(&c).to_string(),
            Err(_) => return,
        };
        if selected == "Back" {
            return;
        }
        if selected == "Create new profile" {
            let name = match Text::new("Profile id (filename without .toml):")
                .with_render_config(render_config())
                .with_placeholder("example_person")
                .prompt()
            {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };
            if name.is_empty() || name.contains('/') || name.contains('.') {
                eprintln!("error: invalid name");
                continue;
            }
            let path = dir.join(format!("{name}.toml"));
            if path.exists() {
                eprintln!("error: {name}.toml already exists, choose Edit");
                continue;
            }
            // Guided account creation — TOML pre-filled with real values
            let profile = prompt_new_profile_accounts(&name);
            match crate::config::write_profile_file(&path, &profile) {
                Ok(()) => println!("✔ Created {}", path.display()),
                Err(e) => {
                    eprintln!("error: write profile: {e}");
                    continue;
                }
            }
            // Continue with the menu-driven edit flow
            edit_profile_menu(&name, &path);
        } else if entries.contains(&selected) {
            // Selected an existing profile — open its submenu
            let path = dir.join(format!("{selected}.toml"));
            edit_profile_menu(&selected, &path);
        }
    }
}
