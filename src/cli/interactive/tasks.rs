//! Saved content batches: a name, the accounts to run it on, and what each one
//! wants.
//!
//! A task answers *what and for whom*; a profile answers *how*, with per-run
//! content choices and per-site overrides. They are separate on purpose —
//! separate directory, struct, menus and flows — but both speak
//! [`config::Account`], so a cookie session behaves the same in each.
//!
//! Content belongs to the account, not the task: the wizard asks site → account
//! → content per account, so two instagram accounts in one task can want
//! different things. A task may span networks, and the kinds are a *filter*
//! rather than an instruction — each account is validated against what its site
//! actually supports, and one that cannot serve the request is named and
//! skipped instead of silently fetching something else.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::cli::interactive::menu::{self, Step};
use crate::cli::interactive::theme;
use crate::config::{self, Task, TaskAccount};

/// What a finished wizard did.
enum Created {
    Saved(String),
    /// The user stepped out; nothing was written.
    Cancelled,
}

/// What the menu can do with a saved task.
enum TaskAction {
    Run,
    Edit,
    Delete,
    Back,
}

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
            entries.push((
                theme::brand_site_label(name),
                vec![
                    format!("{} account(s) across {sites} site(s)", accounts),
                    describe_task(t),
                ],
            ));
        }
        entries.push((
            "Create new task".to_string(),
            vec![
                "Pick the content once, then reuse it.".to_string(),
                String::new(),
                "Each account gets its own content and".to_string(),
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
            i if i == create_idx => match create_task(None) {
                Created::Saved(name) => println!("✔ task '{name}' saved"),
                Created::Cancelled => println!("— cancelled, nothing saved"),
            },
            i if i == back_idx => return,
            i => {
                let id = names[i].clone();
                task_menu(&id, &tasks[&id], &cfg);
            }
        }
    }
}

/// What one can do with a single task.
fn task_menu(id: &str, task: &Task, cfg: &config::Config) {
    let action = {
        let entries = vec![
            (
                "Run".to_string(),
                vec![
                    String::new(),
                    "Media lands under the task name.".to_string(),
                ],
            ),
            (
                "Edit".to_string(),
                vec![
                    "Rewalk the wizard with today's accounts.".to_string(),
                    String::new(),
                ],
            ),
            (
                "Delete".to_string(),
                vec!["Removes the saved task.".to_string(), String::new()],
            ),
            ("Back".to_string(), vec![String::new()]),
        ];
        let picked = menu::pick_single(&theme::brand_site_label(id), entries);
        match picked {
            Step::Value(0) => TaskAction::Run,
            Step::Value(1) => TaskAction::Edit,
            Step::Value(2) => TaskAction::Delete,
            Step::Value(_) => TaskAction::Back,
            Step::Back | Step::Cancel => TaskAction::Back,
        }
    };
    match action {
        TaskAction::Run => {
            match run_task(task, cfg) {
                Step::Value(()) => super::clear_screen(),
                // Esc at the preview steps back inside the task rather than
                // out of the list.
                Step::Back | Step::Cancel => {}
            }
        }
        TaskAction::Edit => match create_task(Some(task.clone())) {
            Created::Saved(name) => println!("✔ task '{name}' updated"),
            Created::Cancelled => println!("— unchanged"),
        },
        TaskAction::Delete => {
            // Destructive: Enter keeps it, `y` deletes it.
            let sure = menu::confirm_back("Tasks", &format!("delete task '{id}'?"), false, true)
                .value()
                .unwrap_or(false);
            if sure && let Some(dir) = config::tasks_dir() {
                let path = dir.join(format!("{id}.toml"));
                match std::fs::remove_file(&path) {
                    Ok(()) => println!("✔ task '{id}' deleted"),
                    Err(e) => crate::output::print_error(&format!("could not delete '{id}': {e}")),
                }
            }
        }
        TaskAction::Back => {}
    }
}

/// The wizard: name, then site → account → content, until the user saves.
///
/// `existing` restarts the account list for [`TaskAction::Edit`], so editing is
/// the same walk rather than a second, divergent flow. `Esc` steps back one
/// question at every level and keeps what was typed, matching every other flow.
fn create_task(existing: Option<Task>) -> Created {
    let editing = existing.is_some();
    let (name, mut accounts) = match existing {
        Some(t) => {
            let id = t.task.clone().unwrap_or_else(|| "task".to_string());
            (id, t.accounts)
        }
        None => match ask_task_name() {
            Some(n) => (n, std::collections::HashMap::new()),
            None => return Created::Cancelled,
        },
    };

    // Remembered across step-backs so a question re-opens on what the user
    // already answered, which is what every other flow does.
    let mut previous_site: Option<String> = None;
    let mut previous_username: Option<String> = None;
    let mut previous_kinds: Vec<String> = Vec::new();

    let site_opts = crate::cli::interactive::scrape_flow::site_options_with_fallbacks(&[]);
    if site_opts.is_empty() {
        crate::output::print_error("no sites configured — add one under Configuration first");
        return Created::Cancelled;
    }

    // Labelled at three levels because each `Esc` has to land on the question
    // immediately before it: a label is only in scope inside its own loop, so
    // the direction of the jump is what carries the meaning.
    // One loop level: it re-asks the site question, which is both where "step
    // back" from the username lands and where "add another account" restarts.
    'account: loop {
        let site = match ask_site(&name, &site_opts, previous_site.as_deref()) {
            Some(s) => s,
            // Nothing before the site question: cancel outright, or if
            // accounts are already collected, save what there is.
            None => {
                if accounts.is_empty() {
                    return Created::Cancelled;
                }
                return finish(&name, accounts, editing);
            }
        };
        previous_site = Some(site.clone());

        // Esc here steps back to the site list.
        let username = match ask_username(&site, previous_username.as_deref()) {
            Some(u) => u,
            None => continue 'account,
        };
        previous_username = Some(username.clone());

        let cookies = pick_cookie_for(&site);
        'detail: loop {
            // Esc here steps back to the username, which keeps its text.
            let kinds = match ask_kinds(&site, &previous_kinds) {
                Some(k) => k,
                None => continue 'detail,
            };
            previous_kinds = kinds.clone();

            let mut account = TaskAccount::default();
            account.account.username = Some(username.clone());
            account.account.cookie_profile = cookies.clone();
            account.kinds = kinds;
            let slot = accounts.entry(site.clone()).or_default();
            slot.push(account);

            // Two positive actions rather than a yes/no: there is no "no"
            // to express, and Enter takes the highlighted row, which is
            // save.
            let choice = menu::pick_single(
                &format!("New task ─ {name}"),
                vec![
                    (
                        format!("Save task '{name}'"),
                        vec![format!("{} account(s) so far", total(&accounts))],
                    ),
                    (
                        "Add another account".to_string(),
                        vec!["Site, account and content again.".to_string()],
                    ),
                ],
            );
            match choice {
                Step::Value(0) => return finish(&name, accounts, editing),
                // Add another: leave the detail loop so the site is asked
                // again. The remembered site puts the cursor back on it.
                Step::Value(_) => break 'detail,
                // Esc re-asks the content question with its selection
                // still marked.
                Step::Back => continue 'detail,
                Step::Cancel => return finish(&name, accounts, editing),
            }
        }
    }
}

fn total(accounts: &std::collections::HashMap<String, Vec<TaskAccount>>) -> usize {
    accounts.values().map(|v| v.len()).sum()
}

fn finish(
    name: &str,
    accounts: std::collections::HashMap<String, Vec<TaskAccount>>,
    editing: bool,
) -> Created {
    if accounts.is_empty() {
        println!("— no accounts, nothing saved");
        return Created::Cancelled;
    }
    let Some(dir) = config::tasks_dir() else {
        crate::output::print_error("could not locate the tasks directory");
        return Created::Cancelled;
    };
    let path = dir.join(format!("{name}.toml"));
    if !editing && path.exists() {
        crate::output::print_error(&format!("task '{name}' already exists"));
        return Created::Cancelled;
    }
    let task = Task {
        task: Some(name.to_string()),
        display_name: Some(name.to_string()),
        accounts,
    };
    match config::write_task_file(&path, &task) {
        Ok(()) => Created::Saved(name.to_string()),
        Err(e) => {
            crate::output::print_error(&format!("could not save the task: {e}"));
            Created::Cancelled
        }
    }
}

fn ask_task_name() -> Option<String> {
    loop {
        let raw = menu::input_text_back(
            "New task",
            "Task name (becomes the output folder):",
            "school_stories",
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
        // Same rules as profile ids: the name becomes a folder name.
        if name.contains('/') || name.contains('.') || name.contains('\\') {
            println!("⚠ no / . or \\ in a task name — it becomes a folder");
            continue;
        }
        return Some(name);
    }
}

fn ask_site(task: &str, site_opts: &[String], previous: Option<&str>) -> Option<String> {
    let site_list: Vec<(String, Vec<String>)> = site_opts
        .iter()
        .map(|s| {
            let details = crate::sites::registry::find_by_id(s)
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
    let initial = previous
        .and_then(|p| site_opts.iter().position(|s| s == p))
        .unwrap_or(0);
    let picked = menu::pick_single_back_at(&format!("Task '{task}' ─ site"), site_list, initial);
    match picked {
        Step::Value(i) => site_opts.get(i).cloned(),
        Step::Back | Step::Cancel => None,
    }
}

fn ask_username(site: &str, previous: Option<&str>) -> Option<String> {
    loop {
        let raw = menu::input_text_back(
            &format!("Task ─ {site}"),
            "Username:",
            "someone",
            "no @ needed",
            previous.unwrap_or(""),
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
    let ranked = rank_cookie_profiles(stored, &sites);

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

/// Stored profiles, the ones carrying cookies for `sites` first, then by name.
fn rank_cookie_profiles(stored: Vec<String>, sites: &[String]) -> Vec<(bool, String)> {
    use crate::config::cookies;
    let mut ranked: Vec<(bool, String)> = stored
        .into_iter()
        .map(|name| {
            let matches = cookies::has_cookies_for(&name, sites);
            (matches, name)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    ranked
}

/// Pick what this account wants, from its own site's menu.
fn ask_kinds(site: &str, previous: &[String]) -> Option<Vec<String>> {
    let options = crate::cli::interactive::content::content_options(site);
    if options.is_empty() {
        return Some(vec!["All".to_string()]);
    }
    let entries: Vec<(String, Vec<String>)> = options
        .iter()
        .map(|o| (o.to_string(), Vec::new()))
        .collect();
    let prechecked = prechecked_indices(&options, previous);
    let idxs =
        match menu::pick_multi_back(&format!("Task ─ {site} ─ content"), entries, &prechecked) {
            Step::Value(v) => v,
            Step::Back | Step::Cancel => return None,
        };
    let picked: Vec<String> = idxs
        .into_iter()
        .filter_map(|i| options.get(i).map(|o| o.to_string()))
        .collect();
    if picked.is_empty() {
        println!("⚠ pick at least one kind");
        return ask_kinds(site, previous);
    }
    Some(picked)
}

/// Indices of `previous` within `options`, for the picker to pre-mark.
///
/// A label the site no longer offers is skipped rather than treated as an
/// error: the task was written when the site offered it, and the run reports
/// the account as skipped instead of refusing to open the menu.
fn prechecked_indices(options: &[&str], previous: &[String]) -> Vec<usize> {
    previous
        .iter()
        .filter_map(|k| options.iter().position(|o| *o == k.as_str()))
        .collect()
}

/// Run a saved task.
fn run_task(task: &Task, cfg: &config::Config) -> Step<()> {
    let cfg = cfg.clone();
    let task_id = task.task.clone().unwrap_or_else(|| "task".to_string());

    let mut jobs = Vec::new();
    // Which job indices still need a session for this run, so the per-run
    // cookie question can never overwrite one the task already pinned.
    let mut needs_session: Vec<usize> = Vec::new();

    let mut sites: Vec<&String> = task.accounts.keys().collect();
    sites.sort();
    for site in sites {
        for ta in &task.accounts[site] {
            let account = &ta.account;
            let Some(username) = account
                .username
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
            else {
                println!("⚠ {site}: account without a username — skipped");
                continue;
            };
            let requested: HashSet<String> = ta.kinds.iter().cloned().collect();
            let kinds = kinds_for_site(site, &requested);
            if kinds.is_empty() {
                // Name the account and the reason: a silent skip reads as
                // "the task has nothing to fetch".
                println!(
                    "⚠ {site}:{username} — {} has no content this site can fetch — skipped",
                    describe_kinds(&ta.kinds)
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

            let ctx = crate::cli::interactive::scrape_flow::AccountCtx {
                site: site.clone(),
                username: username.to_string(),
                kinds,
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
                cookies_file,
                cookies_from_browser,
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
        let mut unique: Vec<String> = needs_session
            .iter()
            .filter_map(|i| jobs.get(*i).map(|(_, site, ..)| site.clone()))
            .collect();
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
    let ranked = rank_cookie_profiles(stored, sites);

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

/// What one account can actually get: the content it asked for minus what the
/// site has no menu entry for, with `"All"` meaning everything this site
/// supports.
fn kinds_for_site(
    site: &str,
    requested: &HashSet<String>,
) -> Vec<crate::cli::interactive::content::ContentKind> {
    use crate::cli::interactive::content::{ContentKind, content_options};
    let options = content_options(site);
    if requested.is_empty() {
        // A task written before kinds were per-account, or hand-edited.
        return options
            .iter()
            .filter(|o| **o != "All")
            .filter_map(|o| ContentKind::from_label(o))
            .collect();
    }
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

/// One line describing what a task fetches, for the list.
fn describe_task(task: &Task) -> String {
    let mut per: Vec<String> = Vec::new();
    let mut sites: Vec<&String> = task.accounts.keys().collect();
    sites.sort();
    for site in sites {
        let mut kinds: Vec<&String> = task.accounts[site]
            .iter()
            .flat_map(|a| a.kinds.iter())
            .collect();
        kinds.sort();
        kinds.dedup();
        per.push(format!("{site}: {}", describe_kinds_owned(&kinds)));
    }
    per.join("  ")
}

fn describe_kinds_owned(labels: &[&String]) -> String {
    if labels.is_empty() {
        "no content".to_string()
    } else {
        labels
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
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
        assert_ne!(
            ig.len(),
            tiktok.len(),
            "'All' means whatever this site supports"
        );
    }

    #[test]
    fn two_accounts_can_want_different_things() {
        // Content is per account, so one account asking for stories does not
        // drag the other along — the whole reason kinds moved onto the account.
        let stories: HashSet<String> = ["Stories".to_string()].into_iter().collect();
        let reels: HashSet<String> = ["Reels".to_string()].into_iter().collect();
        assert_eq!(kinds_for_site("instagram", &stories).len(), 1);
        assert_eq!(kinds_for_site("instagram", &reels).len(), 1);
        assert_ne!(
            kinds_for_site("instagram", &stories),
            kinds_for_site("instagram", &reels)
        );
    }

    #[test]
    fn a_stale_kind_skips_the_account_instead_of_failing() {
        // A hand-edited task, or a site whose menu changed: the account is
        // dropped and named, never silently given different content.
        let gone: HashSet<String> = ["Nonsense".to_string()].into_iter().collect();
        assert!(kinds_for_site("instagram", &gone).is_empty());
    }

    #[test]
    fn an_account_with_no_kinds_falls_back_to_the_whole_menu() {
        let requested = HashSet::new();
        assert!(
            !kinds_for_site("instagram", &requested).is_empty(),
            "a task written before kinds were per-account still runs"
        );
    }

    #[test]
    fn precheck_marks_the_kinds_already_chosen() {
        // Esc at the content question has to re-open it marked, or the user
        // loses the selection just by stepping back one question.
        let options = ["All", "Posts", "Reels", "Highlights", "Stories", "Profile"];
        // Order follows the stored selection, not the menu: `prechecked` is a
        // set of rows to mark, so the order carries no meaning either way.
        assert_eq!(
            prechecked_indices(&options, &["Stories".into(), "Highlights".into()]),
            vec![4, 3]
        );
        assert_eq!(prechecked_indices(&options, &["Posts".into()]), vec![1]);
    }

    #[test]
    fn precheck_skips_a_kind_the_site_no_longer_offers() {
        // Written when the site offered it, or kept across a registry change:
        // the menu still opens, and the run reports the account as skipped.
        let options = ["All", "Posts", "Reels"];
        let stale: Vec<usize> = prechecked_indices(&options, &["Stories".into()]);
        assert!(stale.is_empty());
        let both = prechecked_indices(&options, &["Reels".into(), "Gone".into()]);
        assert_eq!(
            both,
            vec![2],
            "the stale kind is dropped, the live one is not"
        );
    }

    #[test]
    fn precheck_of_nothing_marks_nothing() {
        let options = ["All", "Posts"];
        assert!(prechecked_indices(&options, &[]).is_empty());
    }

    #[test]
    fn task_list_line_names_each_site_and_its_content() {
        let mut task = Task {
            task: Some("colegio".into()),
            display_name: Some("colegio".into()),
            accounts: Default::default(),
        };
        task.accounts.insert(
            "instagram".into(),
            vec![TaskAccount {
                account: config::Account {
                    username: Some("one".into()),
                    ..Default::default()
                },
                kinds: vec!["Stories".into()],
            }],
        );
        task.accounts.insert(
            "tiktok".into(),
            vec![TaskAccount {
                account: config::Account {
                    username: Some("two".into()),
                    ..Default::default()
                },
                kinds: vec!["Videos".into()],
            }],
        );
        let line = describe_task(&task);
        assert!(line.contains("instagram: Stories"), "{line}");
        assert!(line.contains("tiktok: Videos"), "{line}");
    }
}
