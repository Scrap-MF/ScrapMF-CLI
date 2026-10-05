//! Saved content batches: a name, the content it wants, and who to get it from.
//!
//! A task answers "what and for whom"; a profile answers "how", with per-run
//! content choices and per-site overrides. They are separate on purpose —
//! separate directory, separate struct, separate menus — even though both speak
//! [`crate::config::Account`] so cookie sessions behave the same in each.
//!
//! A task may span networks, and its kinds are a *filter* rather than an
//! instruction: each account gets the intersection of what the task asked for
//! and what its site actually supports. A task spanning Instagram and TikTok
//! asking for stories downloads the Instagram stories and says plainly that it
//! dropped the TikTok account, instead of silently fetching something else.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::cli::interactive::menu::{self, Step};
use crate::cli::interactive::theme;
use crate::config::{self, Account, Task};

/// List saved tasks and act on one.
pub(super) fn tasks_menu() {
    let cfg = config::load().unwrap_or_default();
    loop {
        let tasks = config::load_tasks();
        let mut names: Vec<String> = tasks.keys().cloned().collect();
        names.sort();

        let mut entries: Vec<(String, Vec<String>)> = Vec::new();
        for name in &names {
            let t = &tasks[name];
            let accounts: usize = t.accounts.values().map(|v| v.len()).sum();
            let sites = t.accounts.len();
            let kinds = if t.kinds.is_empty() {
                "no content chosen".to_string()
            } else {
                t.kinds.join(", ")
            };
            entries.push((
                theme::brand_site_label(name),
                vec![
                    format!("content: {kinds}"),
                    format!("{accounts} account(s) across {sites} site(s)"),
                ],
            ));
        }
        entries.push((
            "Create new task".to_string(),
            vec![
                "Pick the content once, then reuse it.".to_string(),
                String::new(),
                "One or more accounts, each with its own".to_string(),
                "cookie session.".to_string(),
            ],
        ));
        entries.push(("Back".to_string(), vec![String::new()]));

        let create_idx = names.len();
        let back_idx = names.len() + 1;
        let picked = match menu::pick_single("Tasks", entries) {
            Step::Value(i) => i,
            // Nothing above this menu: Esc leaves.
            Step::Back | Step::Cancel => return,
        };
        match picked {
            i if i == create_idx => {
                create_task();
                super::clear_screen();
            }
            i if i == back_idx => return,
            i => {
                let id = names[i].clone();
                let t = &tasks[&id];
                match run_task(t, &cfg) {
                    Step::Value(()) => super::clear_screen(),
                    // Esc at the preview steps back inside the task, not out of
                    // the list, so a wrong cookie choice can be fixed by
                    // editing rather than starting over.
                    Step::Back | Step::Cancel => {}
                }
            }
        }
    }
}

/// Create a task: name, then accounts, then what content to fetch.
fn create_task() {
    let Some(name) = ask_task_name() else {
        return;
    };
    let Some(path) = config::tasks_dir().map(|d| d.join(format!("{name}.toml"))) else {
        crate::output::print_error("no se pudo localizar el directorio de tareas");
        return;
    };
    if path.exists() {
        crate::output::print_error(&format!("la tarea '{name}' ya existe"));
        return;
    }

    // Accounts, each with its own cookie session.
    let site_opts = crate::cli::interactive::scrape_flow::site_options_with_fallbacks(&[]);
    let mut accounts: std::collections::HashMap<String, Vec<Account>> =
        std::collections::HashMap::new();
    loop {
        let site_list: Vec<(String, Vec<String>)> = site_opts
            .iter()
            .map(|s| {
                let spec = crate::sites::registry::find_by_id(s);
                let details = spec
                    .map(|sp| {
                        vec![
                            format!("kinds: {}", sp.content_kinds.join(", ")),
                            format!("backend: {:?}", sp.backend),
                        ]
                    })
                    .unwrap_or_else(|| vec!["custom site (sites/*.toml)".to_string()]);
                (theme::brand_site_label(s), details)
            })
            .collect();
        let idx = match menu::pick_single_back("New task ─ site", site_list) {
            Step::Value(i) => i,
            Step::Back | Step::Cancel => return,
        };
        let site = site_opts[idx].clone();

        let Some(username) = ask_username(&site) else {
            return;
        };
        let cookies = pick_cookie_for(&site);
        let mut account = Account {
            username: Some(username.clone()),
            ..Default::default()
        };
        account.cookie_profile = cookies;
        accounts.entry(site).or_default().push(account);

        let more = menu::confirm_back(
            "New task",
            &format!("add another account to '{}'?", name),
            true,
            false,
        )
        .value()
        .unwrap_or(false);
        if !more {
            break;
        }
    }

    // Kinds are chosen once, from everything the sites involved can do.
    let mut sites: Vec<String> = accounts.keys().cloned().collect();
    sites.sort();
    let kinds = match ask_kinds(&sites) {
        Some(k) => k,
        None => return,
    };

    let task = Task {
        task: Some(name.clone()),
        display_name: Some(name.clone()),
        kinds,
        accounts,
    };
    match config::write_task_file(&path, &task) {
        Ok(()) => println!("✔ tarea '{name}' guardada"),
        Err(e) => crate::output::print_error(&format!("no se pudo guardar la tarea: {e}")),
    }
}

fn ask_task_name() -> Option<String> {
    loop {
        let raw = menu::input_text_back(
            "New task",
            "Task name (becomes the output folder):",
            "historias_del_colegio",
            "letters, digits, - and _",
            "",
        );
        let name = match raw {
            Step::Value(v) => v,
            Step::Back | Step::Cancel => return None,
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            println!("⚠ the task needs a name");
            continue;
        }
        // Same rules as profile ids: the name becomes a filename.
        if name.contains('/') || name.contains('.') || name.contains('\\') {
            println!("⚠ no / . or \\ in a task name — it becomes a folder");
            continue;
        }
        return Some(name);
    }
}

fn ask_username(site: &str) -> Option<String> {
    loop {
        let raw = menu::input_text_back(
            &format!("New task ─ {site}"),
            "Username:",
            "someone",
            "no @ needed",
            "",
        );
        let value = match raw {
            Step::Value(v) => v,
            Step::Back | Step::Cancel => return None,
        };
        let value = value.trim().trim_start_matches('@').trim().to_string();
        if value.is_empty() {
            println!("⚠ enter a username");
            continue;
        }
        return Some(value);
    }
}

/// Choose which stored cookie session this account uses.
///
/// Only stored profiles are offered, and the ones that actually carry cookies
/// for this site come first — offering a session that cannot authenticate the
/// account is worse than offering none.
fn pick_cookie_for(site: &str) -> Option<String> {
    use crate::config::cookies;
    let stored = cookies::list_profiles();
    if stored.is_empty() {
        return None;
    }
    let sites = vec![site.to_string()];
    let mut ranked: Vec<(bool, String)> = stored
        .into_iter()
        .map(|name| {
            let matches = cookies::has_cookies_for(&name, &sites);
            (matches, name)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    entries.push((
        "Site default".to_string(),
        vec!["whatever sites/*.toml configures".to_string()],
    ));
    for (matches, name) in &ranked {
        let summary = cookies::profile_summary(name).unwrap_or_default();
        entries.push((
            format!("{name}  — {summary}"),
            vec![if *matches {
                format!("has cookies for {site}")
            } else {
                format!("⚠ no cookies for {site}")
            }],
        ));
    }
    match menu::pick_single_back(&format!("Cookies for {site}"), entries) {
        // Index 0 is the site default, an explicit answer rather than a skip.
        Step::Value(0) | Step::Back => None,
        Step::Value(i) => ranked.get(i - 1).map(|(_, name)| name.clone()),
        Step::Cancel => None,
    }
}

/// Pick content from everything the sites involved can do.
fn ask_kinds(sites: &[String]) -> Option<Vec<String>> {
    let options = union_kind_options(sites);
    if options.is_empty() {
        return Some(vec!["All".to_string()]);
    }
    let entries: Vec<(String, Vec<String>)> = options
        .iter()
        .map(|o| (o.to_string(), Vec::new()))
        .collect();
    let idxs = match menu::pick_multi_back("New task ─ content", entries, &[]) {
        Step::Value(v) => v,
        Step::Back | Step::Cancel => return None,
    };
    let picked: Vec<String> = idxs
        .into_iter()
        .filter_map(|i| options.get(i).map(|o| o.to_string()))
        .collect();
    if picked.is_empty() {
        println!("⚠ pick at least one kind");
        return ask_kinds(sites);
    }
    Some(picked)
}

/// Every kind any of `sites` offers, `"All"` first, deduplicated.
///
/// A task spanning networks picks from the union, because the kinds are a
/// filter each site applies for itself rather than an instruction to obey
/// blindly.
pub(super) fn union_kind_options(sites: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for site in sites {
        for k in crate::cli::interactive::content::content_options(site) {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    // Insertion order, not alphabetical: for a single-site task this is exactly
    // the menu the user already knows from quick scrape. "All" leads because it
    // is first in every site's menu.
    out
}

/// Run a saved task.
fn run_task(task: &Task, cfg: &config::Config) -> Step<()> {
    let cfg = cfg.clone();
    let task_id = task.task.clone().unwrap_or_else(|| "task".to_string());
    let requested = resolve_requested_kinds(&task.kinds);

    let mut jobs = Vec::new();
    // Which job indices still need a session for this run, so the per-run
    // cookie question can never overwrite one the task already pinned.
    let mut needs_session: Vec<usize> = Vec::new();

    let mut sites: Vec<&String> = task.accounts.keys().collect();
    sites.sort();
    for site in sites {
        for account in &task.accounts[site] {
            let Some(username) = account
                .username
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
            else {
                println!("⚠ {site}: account without a username — skipped");
                continue;
            };
            let kinds = kinds_for_site(site, &requested);
            if kinds.is_empty() {
                // Name the account and the reason: a silent skip reads as
                // "the task has nothing to fetch".
                println!(
                    "⚠ {site}:{username} — '{}' has no content this site can fetch — skipped",
                    describe_kinds(&task.kinds)
                );
                continue;
            }

            let tagged = crate::cli::interactive::content::build_tagged_urls(site, username);
            let site_cfg = cfg.sites.get(site.as_str()).cloned();
            let (cookies_file, cookies_from_browser) = config::cookies::resolve_session(
                account.cookie_profile.as_deref(),
                account.cookies.clone(),
                account.cookies_from_browser.clone(),
                site_cfg.as_ref(),
            );
            let pinned = account.cookie_profile.is_some() || cookies_file.is_some();
            let kind_labels: Vec<crate::cli::interactive::content::ContentKind> = kinds.clone();

            let ctx = crate::cli::interactive::scrape_flow::AccountCtx {
                site: site.clone(),
                username: username.to_string(),
                kinds: kind_labels,
                tagged,
                directory_template: site_cfg.as_ref().and_then(|s| s.directory_template.clone()),
                extractor_options: site_cfg
                    .as_ref()
                    .map(|s| s.extractor.clone())
                    .unwrap_or_default(),
                extra_args: site_cfg
                    .as_ref()
                    .map(|s| s.extra_args.clone())
                    .unwrap_or_default(),
                cookies_file: cookies_file.clone(),
                cookies_from_browser: cookies_from_browser.clone(),
                archive: site_cfg.as_ref().and_then(|s| s.archive.clone()),
                rate_limit: site_cfg.as_ref().and_then(|s| s.rate_limit.clone()),
                filename_template: site_cfg.as_ref().and_then(|s| s.filename_template.clone()),
                // The task's name becomes `{scrapmf_root}`, so its media lands
                // in its own tree instead of mixing with a profile's.
                profile_name: task_id.clone(),
                output_dir: cfg.general.output_dir.clone(),
            };
            match crate::cli::interactive::scrape_flow::jobs_for_account(&ctx) {
                Ok(built) => {
                    let first = jobs.len();
                    jobs.extend(built);
                    if !pinned {
                        for i in first..jobs.len() {
                            needs_session.push(i);
                        }
                    }
                }
                Err(reason) => println!("⚠ {site}:{username} — {reason}"),
            }
        }
    }

    if jobs.is_empty() {
        println!("ℹ nothing to download for '{task_id}'");
        return Step::Value(());
    }

    // Only ask about the accounts the task did not already give a session.
    if !needs_session.is_empty() && std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        let open_sites: Vec<String> = needs_session
            .iter()
            .filter_map(|i| jobs.get(*i).map(|(_, site, ..)| site.clone()))
            .collect();
        let mut unique = open_sites.clone();
        unique.sort();
        unique.dedup();
        if let Some(file) = prompt_cookie_override(&unique) {
            crate::cli::interactive::scrape_flow::apply_cookie_file_where(
                &mut jobs,
                &file,
                |i, _| needs_session.contains(&i),
            );
        }
    }

    crate::cli::interactive::scrape_flow::preview_and_execute(jobs, &cfg)
}

/// Ask which stored session to use for the accounts left open.
fn prompt_cookie_override(sites: &[String]) -> Option<PathBuf> {
    use crate::config::cookies;
    let stored = cookies::list_profiles();
    if stored.is_empty() {
        return None;
    }
    let mut ranked: Vec<(bool, String)> = stored
        .into_iter()
        .map(|name| {
            let matches = cookies::has_cookies_for(&name, sites);
            (matches, name)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut entries: Vec<(String, Vec<String>)> = vec![(
        "Site default".to_string(),
        vec!["whatever sites/*.toml configures".to_string()],
    )];
    for (matches, name) in &ranked {
        let summary = cookies::profile_summary(name).unwrap_or_default();
        entries.push((
            format!("{name}  — {summary}"),
            vec![if *matches {
                "has cookies for these sites".to_string()
            } else {
                "⚠ no cookies for these sites".to_string()
            }],
        ));
    }
    match menu::pick_single_back("Cookies for this run?", entries) {
        Step::Value(0) | Step::Back => None,
        Step::Value(i) => ranked
            .get(i - 1)
            .map(|(_, name)| name)
            .and_then(|n| cookies::profile_path(n))
            .filter(|p| p.exists()),
        Step::Cancel => None,
    }
}

/// Turn the stored labels into concrete kinds, resolving `"All"`.
///
/// `"All"` cannot expand here: what it means depends on the site, so each
/// account resolves it against its own menu.
fn resolve_requested_kinds(labels: &[String]) -> HashSet<String> {
    labels.iter().cloned().collect()
}

/// What one account can actually get: the task's kinds minus what the site has
/// no menu entry for, with `"All"` meaning everything this site supports.
fn kinds_for_site(
    site: &str,
    requested: &HashSet<String>,
) -> Vec<crate::cli::interactive::content::ContentKind> {
    use crate::cli::interactive::content::{ContentKind, content_options};
    let options = content_options(site);
    if requested.contains("All") {
        return options
            .iter()
            .filter(|o| **o != "All")
            .filter_map(|o| ContentKind::from_label(o))
            .collect();
    }
    options
        .iter()
        .filter(|o| **o != "All")
        .filter(|o| requested.contains(**o))
        .filter_map(|o| ContentKind::from_label(o))
        .collect()
}

fn describe_kinds(labels: &[String]) -> String {
    if labels.is_empty() {
        "no content".to_string()
    } else {
        labels.join(", ")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn kinds_for_site_keeps_only_what_the_site_offers() {
        let requested: HashSet<String> = ["Stories".to_string()].into_iter().collect();
        let ig = kinds_for_site("instagram", &requested);
        assert_eq!(ig.len(), 1, "instagram has stories");
        // TikTok has no Stories menu entry, so the account is skipped rather
        // than silently given something else.
        assert!(
            kinds_for_site("tiktok", &requested).is_empty(),
            "tiktok offers no stories"
        );
    }

    #[test]
    fn all_expands_per_site_not_globally() {
        let requested: HashSet<String> = ["All".to_string()].into_iter().collect();
        let ig = kinds_for_site("instagram", &requested);
        let tiktok = kinds_for_site("tiktok", &requested);
        assert!(!ig.is_empty() && !tiktok.is_empty());
        // Different menus, so "all" is not the same set per site.
        assert_ne!(
            ig.len(),
            tiktok.len(),
            "'All' means whatever this site supports"
        );
    }

    #[test]
    fn a_site_with_no_menu_falls_back_without_dropping_everything() {
        let requested: HashSet<String> = ["Posts".to_string(), "Stories".to_string()]
            .into_iter()
            .collect();
        let kinds = kinds_for_site("unknown", &requested);
        assert_eq!(
            kinds.len(),
            1,
            "the fallback menu is All+Posts, so Posts survives"
        );
    }

    #[test]
    fn union_lists_all_first_then_dedupes() {
        let union = union_kind_options(&["instagram".to_string(), "tiktok".to_string()]);
        assert_eq!(union.first(), Some(&"All"), "All must lead the union");
        let mut seen = std::collections::HashSet::new();
        for k in &union {
            assert!(seen.insert(*k), "duplicate kind {k} in the union");
        }
        // Stories only exists on instagram, but the task spanning both sites
        // must still be able to ask for it.
        assert!(union.contains(&"Stories"));
        assert!(union.contains(&"Videos"), "tiktok's own kind is present");
    }

    #[test]
    fn union_of_one_site_matches_that_site_menu() {
        let union = union_kind_options(&["tiktok".to_string()]);
        let menu = crate::cli::interactive::content::content_options("tiktok");
        assert_eq!(union, menu);
    }
}
