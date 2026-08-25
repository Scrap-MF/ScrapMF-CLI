use super::profiles::{edit_profile_menu, prompt_new_profile_accounts};
use super::{ask_nonempty, clear_screen, edit_with_editor, select_menu, theme::render_config};
use std::path::Path;

use inquire::{Confirm, MultiSelect, Text};

pub(super) fn configuration_submenu() {
    loop {
        clear_screen();
        let options = vec![
            "Cookie profiles (cookies/*.txt)",
            "Manage Sites (sites/*.toml) — $EDITOR",
            "Manage Profiles (profiles/*.toml) — $EDITOR",
            "General (output_dir)",
            "Show config path",
            "Edit config.toml ($EDITOR)",
            "Back",
        ];
        let choice = match select_menu("What do you want to configure?", options).prompt() {
            Ok(c) => c,
            Err(_) => {
                clear_screen();
                return;
            }
        };
        match choice {
            "Cookie profiles (cookies/*.txt)" => {
                cookie_profiles_menu();
                clear_screen();
            }
            "Manage Sites (sites/*.toml) — $EDITOR" => {
                manage_sites();
                clear_screen();
            }
            "Manage Profiles (profiles/*.toml) — $EDITOR" => {
                manage_profiles();
                clear_screen();
            }
            "General (output_dir)" => {
                if let Err(e) = crate::commands::config::run(None) {
                    eprintln!("error: config failed: {e}");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
                clear_screen();
            }
            "Show config path" => {
                if let Some(p) = crate::config::config_path() {
                    println!("{}", p.display());
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                }
                clear_screen();
            }
            "Edit config.toml ($EDITOR)" => {
                if let Some(p) = crate::config::config_path() {
                    edit_with_editor(&p);
                }
                clear_screen();
            }
            "Back" => {
                clear_screen();
                return;
            }
            _ => {
                clear_screen();
                return;
            }
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
    let Ok(browser) = select_menu("Capture from which browser?", browsers).prompt() else {
        return;
    };

    // Network multi-select with an "All" shortcut.
    clear_screen();
    const NETWORKS: &[&str] = &["instagram", "tiktok", "twitter", "vsco"];
    let mut net_opts: Vec<String> = vec!["All networks".into()];
    net_opts.extend(NETWORKS.iter().map(|s| super::theme::brand_site_label(s)));
    let Ok(picked_raw): Result<Vec<String>, _> = MultiSelect::new("Which networks?", net_opts)
        .without_filtering()
        .with_render_config(render_config())
        .with_help_message("[space to select · enter to confirm]")
        .prompt()
    else {
        return;
    };
    if picked_raw.is_empty() {
        println!("ℹ No networks selected");
        return;
    }
    // Labels carry ANSI; map back to clean keys via brand_site_label output.
    let mut sites: Vec<String> = Vec::new();
    for picked in &picked_raw {
        for net in NETWORKS {
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
        let options: Vec<String> = cookies::list_profiles();
        let rows: Vec<String> = options
            .iter()
            .map(|name| match cookies::profile_summary(name) {
                Ok(summary) => format!("{name}  — {summary}"),
                Err(e) => format!("{name}  — ⚠ {e}"),
            })
            .collect();
        let mut choices: Vec<String> = vec![
            "Create profile".into(),
            "Refresh profile (re-capture)".into(),
            "Import from paste ($EDITOR)".into(),
        ];
        choices.extend(rows);
        choices.push("Delete a profile".into());
        choices.push("Back".into());
        let choice = match select_menu("Cookie profiles", choices).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "Create profile" => {
                create_profile_wizard();
            }
            "Refresh profile (re-capture)" => {
                let names = cookies::list_profiles();
                if names.is_empty() {
                    println!("ℹ No profiles yet — use Create profile first");
                    std::thread::sleep(std::time::Duration::from_millis(1200));
                    continue;
                }
                // Los perfiles con cookies expiradas saltan a la vista
                let rows: Vec<String> = names
                    .iter()
                    .map(|n| match cookies::profile_summary(n) {
                        Ok(s) => format!("{n}  — {s}"),
                        Err(e) => format!("{n}  — ⚠ {e}"),
                    })
                    .collect();
                let Ok(picked_row) = select_menu("Refresh which profile?", rows).prompt() else {
                    continue;
                };
                let name = picked_row.split("  — ").next().unwrap_or("").to_string();

                println!("→ Re-capturing '{name}'…");
                let mut edited = false;
                match cookies::refresh_profile(&name) {
                    Ok(cookies::Refresh::Done { path, count }) => {
                        println!("✔ refreshed {count} cookie(s) → {}", path.display());
                        println!("✔ accounts using this profile need no changes");
                    }
                    Ok(cookies::Refresh::ManualImportRequired) => {
                        // Importado a mano: refresco vía editor
                        if let Some(p) = cookies::profile_path(&name) {
                            edit_with_editor(&p);
                            edited = true;
                        }
                    }
                    Err(e) => {
                        println!("✖ refresh failed: {e}");
                        println!(
                            "  tip: re-create it with Create profile, or paste \
                                  a fresh export via the import option"
                        );
                    }
                }
                if !edited {
                    let _ = Text::new("Press enter to continue").prompt();
                }
                clear_screen();
                continue;
            }
            "Import from paste ($EDITOR)" => {
                let Some(name) = ask_nonempty("Profile name:") else {
                    return;
                };
                let path = match cookies::profile_path(&name) {
                    Some(p) => p,
                    None => return,
                };
                let template = "# Netscape HTTP Cookie File\n# Paste your exported cookies below.\n# Get them with the 'Get cookies.txt LOCALLY' extension while logged in.\n\n";
                std::fs::write(&path, template).ok();
                edit_with_editor(&path);
                match cookies::load_profile(&name) {
                    Ok(list) => println!("✔ saved {} cookie(s)", list.len()),
                    Err(e) => println!("✖ invalid profile ({e}) — delete or fix {path:?}"),
                }
                std::thread::sleep(std::time::Duration::from_millis(1800));
            }
            "Delete a profile" => {
                let names = cookies::list_profiles();
                if names.is_empty() {
                    println!("ℹ no profiles");
                    continue;
                }
                let target = select_menu("Delete which profile?", names).prompt();
                if let Ok(target) = target {
                    match cookies::delete_profile(&target) {
                        Ok(true) => println!("✔ deleted"),
                        _ => println!("✖ not found"),
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(800));
            }
            "Back" => return,
            _ => {}
        }
    }
}

pub(super) fn site_submenu(name: &str, dir: &Path) {
    let path = dir.join(format!("{name}.toml"));
    loop {
        clear_screen();
        let options = vec!["Edit", "Delete site", "Back"];
        let choice = match select_menu(&format!("Site: {name}"), options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice {
            "Edit" => {
                if path.exists() {
                    edit_with_editor(&path);
                } else {
                    eprintln!("error: {name}.toml no longer exists");
                    return;
                }
            }
            "Delete site" => {
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
            }
            _ => return,
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
