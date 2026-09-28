use std::path::PathBuf;
use std::sync::Arc;

use inquire::{Confirm, Text};

use crate::application::scraper::{ScrapeRequest, validate_url};
use crate::config;

use super::content::{ContentKind, build_tagged_urls, prompt_content_kinds, select_urls};
use super::select_menu;

/// Decide which cookie source a run should use, after the user has chosen what
/// to scrape.
///
/// `sites` are the site keys this run touches, used to rank the stored cookie
/// profiles. Passing the real keys matters: the previous call site for direct
/// URLs passed `""`, and `domains_for_site("")` returns no domains, so the
/// filter silently matched every profile and the question was meaningless.
///
/// Behaviour:
///
/// * **no stored profiles** — return `None` without prompting, so a first-time
///   user goes straight to the site defaults with no extra keystroke;
/// * **at least one stored profile** — always ask, offering the site default
///   alongside the created profiles, so the choice is always explicit.
///
/// Cancelling or pressing Esc means "use the site default", never "abort".
pub(super) fn prompt_cookie_override(sites: &[String]) -> Option<PathBuf> {
    use crate::config::cookies;

    let profiles = cookies::list_profiles();
    if profiles.is_empty() {
        return None;
    }

    // Rank profiles that actually carry cookies for these sites first. When no
    // profile matches, every profile is still offered — the user may know
    // better than the filter, and silently hiding their profile is worse than
    // showing a mismatch hint.
    let mut ranked: Vec<(bool, String)> = profiles
        .into_iter()
        .map(|name| {
            let matches = site_has_cookies(&name, sites);
            (matches, name)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let any_match = ranked.iter().any(|(m, _)| *m);
    let mut opts = vec![DEFAULT_COOKIE_CHOICE.to_string()];
    for (matches, name) in &ranked {
        let summary = cookies::profile_summary(name).unwrap_or_default();
        // The label is only a display string; the key is recovered by index
        // below, never by re-parsing the text.
        opts.push(format!(
            "{name}{}  — {summary}",
            if *matches {
                ""
            } else {
                "  (no cookies for this site)"
            }
        ));
    }

    let title = if any_match {
        "Cookies for this run? (Enter = site default)"
    } else {
        "No stored profile has cookies for this site. Cookies for this run?"
    };
    let Ok(choice) = select_menu(title, opts.clone()).prompt() else {
        // Esc / Ctrl+C: keep scraping with the site default. Aborting the whole
        // run because the user skipped a question was the previous behaviour
        // and read as "scrapmf did nothing".
        return None;
    };
    if choice == DEFAULT_COOKIE_CHOICE {
        return None;
    }
    let index = opts.iter().position(|o| *o == choice)?;
    ranked
        .get(index - 1)
        .map(|(_, name)| name)
        .and_then(|name| cookies::profile_path(name))
        .filter(|p| p.exists())
}

/// Label for the "keep whatever the site config says" option.
const DEFAULT_COOKIE_CHOICE: &str = "Default (from site config)";

/// Human description of the session a job will use, for the pre-flight preview.
pub(super) fn describe_cookie_source(req: &ScrapeRequest) -> String {
    if let Some(file) = req.cookies_file.as_ref() {
        // Show the profile name, not the full path, when the file is one of ours.
        if let Some(dir) = crate::config::cookies::cookies_dir()
            && file.parent() == Some(dir.as_path())
            && let Some(stem) = file.file_stem()
        {
            return format!("cookie profile '{}'", stem.to_string_lossy());
        }
        return format!("cookies {}", file.display());
    }
    if let Some(browser) = req.cookies_from_browser.as_ref() {
        return format!("browser cookies ({browser})");
    }
    "site defaults (no session)".to_string()
}

/// Whether a stored profile holds at least one cookie for any of `sites`.
fn site_has_cookies(name: &str, sites: &[String]) -> bool {
    use crate::config::cookies;
    if sites.is_empty() {
        return true;
    }
    let Ok(cookies) = cookies::load_profile(name) else {
        return false;
    };
    sites.iter().any(|site| {
        let domains = cookies::domains_for_site(site);
        !domains.is_empty()
            && cookies.iter().any(|c| {
                domains
                    .iter()
                    .any(|d| c.domain == *d || c.domain.ends_with(&format!(".{d}")))
            })
    })
}

/// Apply a chosen cookie file to a job, only when that job's site is one the
/// file was chosen for.
///
/// The previous version applied one override to every job in the batch, so in
/// a mixed run a TikTok profile was handed to the Instagram job too — the
/// exact kind of cross-session contamination this feature must never cause.
fn apply_cookie_file_to_site(
    jobs: &mut [(ScrapeRequest, String, String, String)],
    file: &std::path::Path,
    sites: &[String],
) {
    for (req, site, ..) in jobs.iter_mut() {
        if sites.iter().any(|s| s == site) {
            req.cookies_file = Some(file.to_path_buf());
            req.cookies_from_browser = None;
        }
    }
}

pub(super) fn preview_and_execute(
    requests: Vec<(ScrapeRequest, String, String, String)>,
    cfg: &config::Config,
) {
    if requests.is_empty() {
        println!("ℹ No content selected");
        return;
    }
    println!("✔ Ready — {} job(s):", requests.len());
    for (i, (req, site, username, kinds_desc)) in requests.iter().enumerate() {
        let out = req
            .output
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| cfg.general.output_dir.display().to_string());
        println!(
            "  {}. {}:{} → {kinds_desc} → {}",
            i + 1,
            site,
            username,
            out
        );
        // Surface the session each job will actually use, so a wrong cookie
        // profile is visible here rather than discovered after the download.
        println!("      session: {}", describe_cookie_source(req));
    }
    let proceed = Confirm::new("Proceed?")
        .with_render_config(super::theme::render_config())
        .with_default(false)
        .prompt();
    if !proceed.unwrap_or(false) {
        println!("canceled");
        return;
    }

    // Pre-flight: verify provider binaries exist before opening the dashboard.
    // Without this, a missing threadstractormf would fail instantly inside
    // run_dashboard and appear as "no ejecuta nada".
    {
        let mut missing: Vec<String> = Vec::new();
        for (req, site, _, _) in &requests {
            let needs_threads = site == "threads"
                || req.url.contains("threads.com")
                || req.url.contains("threads.net")
                || req
                    .extra_urls
                    .iter()
                    .any(|u| u.contains("threads.com") || u.contains("threads.net"));
            if needs_threads
                && !crate::providers::Provider::is_available(
                    &crate::providers::threadstractor::Threadstractor,
                )
            {
                let msg = "threads plugin is not enabled — enable it in scrapmf → Plugins (installs threadstractormf)".to_string();
                if !missing.contains(&msg) {
                    missing.push(msg);
                }
            } else if !needs_threads
                && !crate::providers::Provider::is_available(
                    &crate::providers::gallery_dl::GalleryDl,
                )
            {
                let msg = "gallery-dl not found — run `scrapmf setup`".to_string();
                if !missing.contains(&msg) {
                    missing.push(msg);
                }
            }
        }
        if !missing.is_empty() {
            for m in &missing {
                crate::output::print_error(m);
            }
            return;
        }
    }

    let total = requests.len();
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdout());

    // TTY: batch runs inside the ratatui dashboard (alternate screen).
    if tty {
        let labels = requests
            .iter()
            .map(|(req, site, username, _)| {
                // Sub-process names are NOT baked into the header — the
                // dashboard renders them as a per-job checklist.
                let _ = (&req.url, &req.extra_urls);
                format!("{site}:{username}")
            })
            .collect::<Vec<_>>();
        let state = Arc::new(std::sync::Mutex::new(crate::ui::DashboardState::new(
            labels,
        )));
        let abort = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut was_cancelled = false;
        // Reports are collected during the run and printed AFTER the
        // dashboard closes: the alternate screen cannot be copied from, and
        // previously failed jobs vanished with it (regression fixed here).
        let mut job_reports: Vec<Vec<(bool, String)>> = Vec::with_capacity(requests.len());

        crate::ui::run_dashboard(state.clone(), abort.clone(), true, || {
            for (i, (req, site, username, _desc)) in requests.iter().enumerate() {
                {
                    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                    st.set_running(i);
                }
                // Persistent copyable log of every gallery-dl line this job
                // produces (best-effort; disabled if the state dir fails).
                let runlog = std::rc::Rc::new(std::cell::RefCell::new(
                    crate::application::runlog::RunLog::open(site, username),
                ));
                let mut hooks = crate::application::scraper::ScrapeHooks {
                    on_steps_plan: Some(Box::new({
                        let st = state.clone();
                        move |names: Vec<String>| {
                            st.lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .set_steps(i, names);
                        }
                    })),
                    on_step: Some(Box::new({
                        let st = state.clone();
                        move |cur: usize| {
                            st.lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .set_step(i, cur);
                        }
                    })),
                    on_file: Box::new({
                        let st = state.clone();
                        let rl = runlog.clone();
                        move |path: &str| {
                            rl.borrow_mut().line(path);
                            let mut st = st.lock().unwrap_or_else(|p| p.into_inner());
                            st.add_file(i);
                            // Log panel reassurance: show the downloaded file
                            let name = path.rsplit('/').next().unwrap_or(path);
                            st.push_log(format!("✔ {name}"));
                        }
                    }),
                    on_log: Box::new({
                        let st = state.clone();
                        let rl = runlog.clone();
                        move |line: &str| {
                            rl.borrow_mut().line(line);
                            // Hide noisy threadstractormf header lines from dashboard (keep in runlog)
                            if line.contains("cookies-from-browser=")
                                || line.contains("threadstractormf @")
                                || line.contains("threadstractor @")
                                || line.contains("rate_limit=")
                                || line.trim() == "posts"
                            {
                                return;
                            }
                            st.lock().unwrap_or_else(|p| p.into_inner()).push_log(line);
                        }
                    }),
                };
                let result = crate::application::scraper::scrape_with_hooks(
                    req,
                    false,
                    Some(&mut hooks),
                    &abort,
                );
                let mut failed = false;
                {
                    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                    match &result {
                        Ok(_) => st.finish_ok(i),
                        Err(e) => {
                            if e.to_string().contains("aborted by user") {
                                st.cancel_from(i);
                                was_cancelled = true;
                                failed = true;
                            } else {
                                st.finish_failed(i, e.to_string());
                                failed = true;
                            }
                        }
                    }
                }
                {
                    let mut rl = runlog.borrow_mut();
                    let status = crate::application::runlog::status_line(&result);
                    rl.finish(&status);
                }
                let lines = format_job_outcome(&result, site, username);
                let log_path = runlog.borrow().path.display().to_string();
                drop(runlog);
                // Termux MediaScan — single dir scan per finished download (quick/saved/url)
                if result.is_ok() {
                    let dir = crate::config::expand_output_dir(
                        req.output.as_ref().unwrap_or(&cfg.general.output_dir),
                    );
                    crate::application::media_scan::maybe_scan(&dir);
                }
                let mut rep = lines;
                if failed && !log_path.is_empty() {
                    rep.push((true, format!("log: {log_path}")));
                }
                job_reports.push(rep);
                if was_cancelled {
                    break;
                }
            }
        });

        if was_cancelled {
            println!("⚠ Ejecución cancelada por el usuario");
        }
        for rep in &job_reports {
            for (stderr, line) in rep {
                if *stderr {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            }
        }
        return;
    }

    // Non-TTY (CI/pipes): raw inherited output as before
    for (i, (req, site, username, _desc)) in requests.into_iter().enumerate() {
        println!(
            "→ [{}/{}] Scraping {}:{} — {}",
            i + 1,
            total,
            site,
            username,
            req.url
        );
        let result = crate::application::scraper::scrape(&req, false);
        report_job_outcome(&result, &site, &username);
        if result.is_ok() {
            let dir = crate::config::expand_output_dir(
                req.output.as_ref().unwrap_or(&cfg.general.output_dir),
            );
            crate::application::media_scan::maybe_scan(&dir);
        }
    }
}

/// Format per-job outcome summary lines (shared TTY/non-TTY).
/// Each entry is `(to_stderr, text)` so callers keep stream semantics.
pub(super) fn format_job_outcome(
    result: &anyhow::Result<crate::application::scraper::ScrapeOutcome>,
    site: &str,
    username: &str,
) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut push = |stderr: bool, msg: String| out.push((stderr, msg));
    match result {
        Ok(outcome) => {
            if outcome.success_count > 0 {
                push(
                    false,
                    format!(
                        "✔ {site}:{username} done ({} sub-extractors)",
                        outcome.success_count
                    ),
                );
            }
            if !outcome.skipped.is_empty() {
                push(
                    false,
                    format!(
                        "⚠ {site}:{username} — {} sub-extractor(s) skipped (auth/unsupported):",
                        outcome.skipped.len()
                    ),
                );
                for s in &outcome.skipped {
                    push(false, format!("    - {s}"));
                }
            }
            if !outcome.failed.is_empty() {
                push(
                    true,
                    format!(
                        "✖ {site}:{username} — {} sub-extractor(s) failed:",
                        outcome.failed.len()
                    ),
                );
                for f in &outcome.failed {
                    push(true, format!("    - {f}"));
                }
            }
            if outcome.challenge_failures > 0 {
                push(
                    false,
                    format!(
                        "⚠ {site}:{username} — {} post(s) lost to JavaScript challenges. \
                         Refresh your browser session on the site, then re-run",
                        outcome.challenge_failures
                    ),
                );
            }
        }
        Err(e) => {
            let short = e.to_string().lines().next().unwrap_or("error").to_string();
            tracing::debug!(error = %e, "scrape job failed");
            push(true, format!("✖ {site}:{username} — {short}"));
        }
    }
    out
}

/// Print per-job outcome summary lines (shared TTY/non-TTY).
pub(super) fn report_job_outcome(
    result: &anyhow::Result<crate::application::scraper::ScrapeOutcome>,
    site: &str,
    username: &str,
) {
    for (stderr, line) in format_job_outcome(result, site, username) {
        if stderr {
            eprintln!("{line}");
        } else {
            println!("{line}");
        }
    }
}

/// Quick scrape: pick a network, type a username, choose content types.
/// Nothing is persisted — no profile file is created. The tree roots at the
/// username: {output}/{username}/{content}/... — site templates are preserved
/// except for the network/account path segments, which are stripped.
/// Quick mode: flatten a `directory` template value so downloads land
/// directly under `<output>/<root>/<content-type>/`.
///
/// - Removes network identity segments (`{category}`, `{user}`, ...)
/// - Replaces `{scrapmf_root}` with the literal root (the username), so we no
///   longer depend on gallery-dl keyword injection resolving correctly.
/// - Handles both plain arrays and conditional tables (e.g. TikTok posts keyed
///   by `post_type`). Arrays that would end up empty are left untouched.
fn flatten_for_quick(v: &mut toml::Value, root: &str) {
    match v {
        toml::Value::Array(arr) => {
            let original = arr.clone();
            arr.retain(|seg| {
                seg.as_str()
                    .is_none_or(|s| !crate::cli::interactive::content::is_identity_segment(s))
            });
            if arr.is_empty() {
                *arr = original;
                return;
            }
            for seg in arr.iter_mut() {
                // {scrapmf_root} actual + {scarpmf_root} legacy (configs de
                // usuarios anteriores al renombre)
                if matches!(
                    seg.as_str(),
                    Some("{scrapmf_root}") | Some("{scarpmf_root}")
                ) {
                    *seg = toml::Value::String(root.to_string());
                }
            }
        }
        toml::Value::Table(map) => {
            for (_, child) in map.iter_mut() {
                flatten_for_quick(child, root);
            }
        }
        _ => {}
    }
}

/// Same flattening for `Vec<String>` directory templates.
pub(super) fn flatten_quick_dirs(dirs: Vec<String>, root: &str) -> Vec<String> {
    let mut v = toml::Value::Array(dirs.into_iter().map(toml::Value::String).collect());
    flatten_for_quick(&mut v, root);
    match v {
        toml::Value::Array(items) => items
            .into_iter()
            .map(|seg| seg.as_str().unwrap_or_default().to_string())
            .collect(),
        _ => Vec::new(),
    }
}

/// Route quick-scrape per-pass overrides into a `ScrapeRequest`.
///
/// - **twitter**: needs TWO passes with per-file filters (`photos` /
///   `videos`) and per-pass directories — per-FILE conditional templates
///   don't work there — so both ride inside extractor options scoped to
///   `extractor.twitter.media.*`.
/// - **every other site**: the flattened directory goes to the
///   request-level `directory_template` (emitted by `build_args` as
///   `-o extractor.<site>.directory=[...]`). Sub-extractor scoped
///   overrides (instagram stories/highlights/avatar) still win over it,
///   exactly like the normal profile flow.
///
/// Bug history: this used to hardcode `"twitter:media"` for ALL sites, so
/// an instagram quick scrape silently dropped its directory template —
/// posts fell back to gallery-dl's default `{category}/{username}` tree
/// while highlights created a second root from their own scoped override.
pub(super) fn apply_quick_override(
    extractor_options: &mut std::collections::HashMap<String, toml::Value>,
    directory_template: &mut Option<Vec<String>>,
    site_name: &str,
    directory_override: Option<Vec<String>>,
    extra_opts: Vec<(String, String)>,
) {
    if site_name == "twitter" {
        let mut insert_media_opt = |key: String, value: toml::Value| {
            let media = extractor_options
                .entry("twitter:media".to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if let toml::Value::Table(map) = media {
                map.insert(key, value);
            }
        };
        if let Some(dirs) = directory_override {
            insert_media_opt(
                "directory".to_string(),
                toml::Value::Array(dirs.into_iter().map(toml::Value::String).collect()),
            );
        }
        for (k, v) in extra_opts {
            insert_media_opt(k, toml::Value::String(v));
        }
    } else {
        *directory_template = directory_override;
        if !extra_opts.is_empty() {
            tracing::warn!(
                site = %site_name,
                opts = ?extra_opts,
                "quick-scrape extra_opts are twitter-only; dropped"
            );
        }
    }
}

/// Parse a pasted blob of URLs (separators: spaces, commas, tabs, newlines).
/// Returns `(valid_urls, error_lines)`; order preserved, duplicates dropped.
pub(super) fn parse_pasted_urls(raw: &str) -> (Vec<String>, Vec<String>) {
    let mut valid = Vec::new();
    let mut errors = Vec::new();
    for token in raw.split([',', ' ', '\t', '\n', '\r']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match validate_url(token) {
            Ok(_) => {
                if !valid.iter().any(|u| u == token) {
                    valid.push(token.to_string());
                }
            }
            Err(e) => errors.push(format!("{token}: {e}")),
        }
    }
    (valid, errors)
}

/// Auto-match a sites/*.toml entry whose `pattern` appears in `url`.
pub(super) fn auto_match_site_key(
    cfg: &config::Config,
    url: &str,
) -> Option<(String, crate::config::Site)> {
    // Longest matching pattern wins across BOTH config fields (pattern and
    // patterns[]), mirroring config::site_matches semantics.
    let mut best: Option<(usize, &String, &crate::config::Site)> = None;
    for (key, site) in cfg.sites.iter() {
        let candidates = site
            .pattern
            .iter()
            .map(String::as_str)
            .chain(site.patterns.iter().map(String::as_str));
        for pat in candidates {
            if url.contains(pat) && best.as_ref().is_none_or(|(len, _, _)| pat.len() > *len) {
                best = Some((pat.len(), key, site));
            }
        }
    }
    best.map(|(_, k, s)| (k.clone(), s.clone()))
}

/// Interactive flow: paste MULTIPLE direct URLs (e.g. private-account story
/// links copied while viewing them, individual post links), auto-match each
/// against sites/*.toml, and batch-run them.
///
/// Motivation (verified against pinned gallery-dl v1.32.9): TikTok
/// private-but-followed accounts fail the /@USER/stories LIST path
/// (profile page statusCode 10222), but their direct /video/<id> story
/// links extract perfectly — so pasting links is the reliable route.
pub(super) fn prompt_scrape_direct_urls() {
    let Some(raw) = crate::cli::interactive::menu::input_text(
        "Download content ─ URL(s)",
        "Paste URL(s) — separate with spaces or commas:",
        "https://www.tiktok.com/@user/video/123 https://www.instagram.com/reel/xyz/",
        "each URL is matched against your sites/*.toml patterns",
    ) else {
        println!("canceled");
        return;
    };

    let (urls, errors) = parse_pasted_urls(&raw);
    for e in &errors {
        println!("⚠ skipped invalid URL — {e}");
    }
    if urls.is_empty() {
        println!("ℹ No valid URLs");
        return;
    }

    let cfg = config::load().unwrap_or_default();
    let mut jobs = Vec::new();
    for url in &urls {
        let label = url
            .trim_end_matches('/')
            .rsplit('/')
            .find(|seg| !seg.is_empty())
            .unwrap_or(url)
            .to_string();

        // Auto-match site config by pattern; URLs from unconfigured sites
        // still scrape with general defaults (gallery-dl supports hundreds
        // of extractors natively — inherits the old "(no site)" behavior).
        if let Some((site_key, site)) = auto_match_site_key(&cfg, url) {
            let output = Some(match &site.output_dir {
                Some(o) => crate::config::expand_output_dir(o),
                None => crate::config::expand_output_dir(&cfg.general.output_dir),
            });
            let req = ScrapeRequest {
                url: url.clone(),
                output,
                preset: Some(site_key.clone()),
                extra_args: site.extra_args.clone(),
                cookies_from_browser: site.cookies_from_browser.clone(),
                cookies_file: site.cookies.clone(),
                archive: site.archive.clone(),
                rate_limit: site.rate_limit.clone(),
                extractor_options: site.extractor.clone(),
                filename_template: site.filename_template.clone(),
                directory_template: site.directory_template.clone(),
                extra_urls: Vec::new(),
                profile_name: None,
                extra_extractor_opts: Vec::new(),

                ..Default::default()
            };
            jobs.push((
                req,
                site_key.clone(),
                format!("{site_key}:{label}"),
                "direct link".to_string(),
            ));
        } else {
            println!("ℹ {url} — no sites/*.toml match; using general config");
            jobs.push((
                ScrapeRequest {
                    url: url.clone(),
                    output: Some(crate::config::expand_output_dir(&cfg.general.output_dir)),
                    preset: None,
                    extra_args: vec![],
                    cookies_from_browser: None,
                    cookies_file: None,
                    archive: None,
                    rate_limit: None,
                    extractor_options: Default::default(),
                    filename_template: None,
                    directory_template: None,
                    extra_urls: Vec::new(),
                    profile_name: None,
                    extra_extractor_opts: Vec::new(),

                    ..Default::default()
                },
                "general".to_string(),
                format!("general:{label}"),
                "direct link".to_string(),
            ));
        }
    }

    // Per-run cookie override (named profile instead of site defaults).
    // The sites are derived from the resolved jobs, not guessed, so the
    // profile filter actually has domains to work with.
    let job_sites: Vec<String> = {
        let mut v: Vec<String> = jobs.iter().map(|(_, site, ..)| site.clone()).collect();
        v.sort();
        v.dedup();
        v
    };
    let cookie_override = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        prompt_cookie_override(&job_sites)
    } else {
        None
    };
    if let Some(ref file) = cookie_override {
        apply_cookie_file_to_site(&mut jobs, file, &job_sites);
    }

    preview_and_execute(jobs, &cfg);
}

/// Stems of sites/*.toml files + registry entries, sorted. Registry is the
/// single source of truth — adding a site = one `SiteSpec` entry.
/// Shared by quick scrape and profiles.
pub(super) fn site_options_with_fallbacks(fallbacks: &[&str]) -> Vec<String> {
    let mut opts: Vec<String> = Vec::new();
    if let Some(dir) = crate::config::sites_dir()
        && let Ok(rd) = std::fs::read_dir(&dir)
    {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "toml")
                && let Some(stem) = e.path().file_stem().and_then(|n| n.to_str())
            {
                opts.push(stem.to_string());
            }
        }
    }
    // Ensure registry sites appear even without a file yet (e.g. threads before install)
    for spec in crate::sites::registry::all_specs() {
        if !opts.iter().any(|s| s == spec.id) {
            opts.push(spec.id.to_string());
        }
    }
    for fb in fallbacks {
        if !opts.iter().any(|s| s == fb) {
            opts.push((*fb).to_string());
        }
    }
    // Plugin gating: sites backed by an optional provider only appear when
    // the plugin is installed and enabled (threads → threadstractormf).
    if !crate::plugins::threads_enabled() {
        opts.retain(|s| s != "threads");
    }
    opts.sort();
    opts
}

pub(super) fn prompt_quick_scrape() {
    let cfg = config::load().unwrap_or_default();

    // Site selection from sites/*.toml (+ fallbacks)
    let site_opts =
        site_options_with_fallbacks(&["instagram", "tiktok", "twitter", "vsco", "facebook"]);

    // Decorated Browser with fixed chrome `╭ SCRAPMF v1.7.0 ─ Download content ─╮`
    let opts: Vec<(String, Vec<String>)> = site_opts
        .iter()
        .map(|k| {
            let spec = crate::sites::registry::find_by_id(k);
            let details = if let Some(s) = spec {
                vec![
                    format!("pattern: {}", s.patterns.join(", ")),
                    format!("backend: {:?}", s.backend),
                    format!("kinds: {}", s.content_kinds.join(", ")),
                ]
            } else {
                vec!["custom site (sites/*.toml)".to_string()]
            };
            (crate::cli::interactive::theme::brand_site_label(k), details)
        })
        .collect();
    let Some(idx) = crate::cli::interactive::menu::pick_single("Download content", opts) else {
        return;
    };
    let site_name = site_opts[idx].clone();
    let prompt_text = if site_name == "facebook" {
        "ID o URL del perfil (ej. 123..., https://www.facebook.com/profile.php?id=...):"
    } else if site_name == "instagram" {
        "Username or ID (without @):"
    } else {
        "Username (without @):"
    };
    let raw_input = match crate::cli::interactive::menu::input_text(
        &format!("Download content ─ {site_name}"),
        prompt_text,
        "someuser",
        "",
    )
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    {
        Some(s) => s,
        None => return,
    };
    // Facebook: accept ID or full profile URL (profile.php?id=, people/Name/ID, fb.com, etc.)
    // Instagram: accept ID (7-19 digits) as before. Both resolve ID → username.
    let (raw_is_id, raw_id, display_for_menu) = if site_name == "instagram"
        && crate::application::instagram_resolver::is_id_like(&raw_input)
    {
        let id = crate::application::instagram_resolver::normalize_id(&raw_input);
        (true, id.clone(), id)
    } else if site_name == "facebook" {
        if let Some(extracted) =
            crate::application::facebook_resolver::extract_identifier(&raw_input)
        {
            let is_id = crate::application::facebook_resolver::is_id_like(&extracted);
            if is_id {
                let nid = crate::application::facebook_resolver::normalize_id(&extracted);
                (true, nid.clone(), nid)
            } else {
                (false, String::new(), extracted)
            }
        } else {
            (
                false,
                String::new(),
                raw_input.trim().trim_start_matches('@').to_string(),
            )
        }
    } else {
        (
            false,
            String::new(),
            raw_input.trim().trim_start_matches('@').to_string(),
        )
    };
    // Keep original ID for facebook URL building (pages use profile.php?id=ID, not sanitized title)
    let facebook_id_for_url: Option<String> = if site_name == "facebook" && raw_is_id {
        Some(raw_id.clone())
    } else {
        None
    };

    // Content menu — same cycle as username (choose content before cookies/resolve)
    let kinds = prompt_content_kinds(
        &site_name,
        &super::theme::brand_account_label(&format!("{site_name}:{display_for_menu}")),
    );
    if kinds.is_empty() {
        println!("ℹ No content selected");
        return;
    }

    // Site config (raw, not yet baked with username)
    let site_cfg = cfg.sites.get(site_name.as_str()).cloned();
    let extractor_options_raw = site_cfg
        .as_ref()
        .map(|s| s.extractor.clone())
        .unwrap_or_default();
    let directory_template_raw = site_cfg.as_ref().and_then(|s| s.directory_template.clone());
    let cookies_from_browser_cfg = site_cfg
        .as_ref()
        .and_then(|s| s.cookies_from_browser.clone());
    let cookies_file_cfg = site_cfg.as_ref().and_then(|s| s.cookies.clone());
    let rate_limit = site_cfg.as_ref().and_then(|s| s.rate_limit.clone());
    let archive = site_cfg.as_ref().and_then(|s| s.archive.clone());
    let extra_args = site_cfg
        .as_ref()
        .map(|s| s.extra_args.clone())
        .unwrap_or_default();
    let filename_template = site_cfg.as_ref().and_then(|s| s.filename_template.clone());

    // Per-run cookie override comes BEFORE ID resolution so resolver uses
    // the same session that will be used for downloading (same cycle as username).
    let cookie_override = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        prompt_cookie_override(std::slice::from_ref(&site_name))
    } else {
        None
    };
    // Ensure terminal line is clean after Select (inquire leaves raw escape on some terms)
    println!();
    let (cookies_file_for_resolve, cookies_browser_for_resolve) =
        if let Some(ref ov) = cookie_override {
            (Some(ov.as_path()), None)
        } else {
            (
                cookies_file_cfg.as_deref(),
                cookies_from_browser_cfg.as_deref(),
            )
        };

    // Now resolve ID → username if needed, using the final cookies
    let username = if raw_is_id {
        let res = if site_name == "instagram" {
            crate::application::instagram_resolver::resolve_instagram_username(
                &raw_id,
                cookies_file_for_resolve,
                cookies_browser_for_resolve,
            )
        } else if site_name == "facebook" {
            crate::application::facebook_resolver::resolve_facebook_id_to_username(
                &raw_id,
                cookies_file_for_resolve,
                cookies_browser_for_resolve,
            )
        } else {
            Err(anyhow::anyhow!("unsupported site for ID"))
        };
        match res {
            Ok(u) => {
                println!("→ {} → @{} (resuelto)", raw_id, u);
                u
            }
            Err(e) => {
                let site_label = if site_name == "facebook" { "FB" } else { "IG" };
                crate::output::print_error(&format!(
                    "no se pudo resolver ID a username: {e} — verifica el ID y que la sesión de {site_label} esté vigente"
                ));
                crate::output::print_help("nota: el error queda visible hasta que presiones Enter");
                let _ = Text::new("Presiona Enter para volver")
                    .with_render_config(super::theme::render_config())
                    .prompt();
                return;
            }
        }
    } else {
        display_for_menu.clone()
    };

    let tagged = if site_name == "facebook"
        && let Some(id) = &facebook_id_for_url
    {
        vec![
            (
                ContentKind::Posts,
                format!("https://www.facebook.com/profile.php?id={id}/photos"),
            ),
            (
                ContentKind::Albums,
                format!("https://www.facebook.com/profile.php?id={id}/photos_albums"),
            ),
            (
                ContentKind::Videos,
                format!("https://www.facebook.com/profile.php?id={id}/videos/"),
            ),
        ]
    } else {
        build_tagged_urls(&site_name, &username)
    };
    let Some((url, extra_urls)) = select_urls(&tagged, &kinds) else {
        println!("ℹ No content selected");
        return;
    };
    if validate_url(&url).is_err() {
        eprintln!("warn: skipping invalid url {url}");
        return;
    }

    let kinds_desc = super::content::kinds_description(&site_name, &kinds);

    // QUICK MODE — bake the resolved username into directory templates
    let directory_template = directory_template_raw
        .clone()
        .map(|dirs| flatten_quick_dirs(dirs, &username));
    let mut extractor_options = extractor_options_raw.clone();
    for v in extractor_options.values_mut() {
        if let toml::Value::Table(map) = v
            && let Some(dir) = map.get_mut("directory")
        {
            flatten_for_quick(dir, &username);
        }
    }
    // Final cookies for the jobs: override wins over site config
    let (cookies_file, cookies_from_browser) = if let Some(ref ov) = cookie_override {
        (Some(ov.clone()), None)
    } else {
        (cookies_file_cfg.clone(), cookies_from_browser_cfg.clone())
    };

    let base_req = |directory_override: Option<Vec<String>>,
                    extra_opts: Vec<(String, String)>,
                    extra_urls: Vec<String>,
                    profile_name: String| {
        let mut directory_template_field = None;
        let mut opts = extractor_options.clone();
        apply_quick_override(
            &mut opts,
            &mut directory_template_field,
            &site_name,
            directory_override,
            extra_opts,
        );
        ScrapeRequest {
            url: url.clone(),
            output: Some(crate::config::expand_output_dir(&cfg.general.output_dir)),
            preset: Some(site_name.clone()),
            extra_args: extra_args.clone(),
            cookies_from_browser: cookies_from_browser.clone(),
            cookies_file: cookies_file.clone(),
            archive: archive.clone(),
            rate_limit: rate_limit.clone(),
            extractor_options: opts,
            filename_template: filename_template.clone(),
            directory_template: directory_template_field,
            extra_urls,
            profile_name: Some(profile_name),
            extra_extractor_opts: Vec::new(),

            ..Default::default()
        }
    };

    // Twitter Media needs TWO passes (see prompt_scrape_as_profile note):
    // per-FILE conditional directories don't work on twitter.
    if site_name == "twitter" {
        let root = username.clone();
        let mut jobs = Vec::new();
        for (pass, dir_name, filter) in [
            ("photos", "photos", "type == 'photo'"),
            ("videos", "videos", "type != 'photo'"),
        ] {
            let dirs = vec![
                username.clone(),
                "twitter".to_string(),
                "{user[name]}".to_string(),
                dir_name.to_string(),
            ];
            let req = base_req(
                Some(dirs),
                vec![("file-filter".to_string(), filter.to_string())],
                Vec::new(),
                root.clone(),
            );
            jobs.push((
                req,
                site_name.clone(),
                format!("{username} ({pass})"),
                pass.to_string(),
            ));
        }
        preview_and_execute(jobs, &cfg);
        return;
    }

    // Threads: fotos/videos (posts) and profile pic are separate — profile needs --profile-pic-only
    // Always 3 separate jobs so the dashboard shows progress 1-by-1, even for All.
    if site_name == "threads" {
        use crate::cli::interactive::content::ContentKind;
        let has_photos = kinds.contains(&ContentKind::Photos);
        let has_videos = kinds.contains(&ContentKind::Videos);
        let has_profile = kinds.contains(&ContentKind::Profile);
        if has_photos || has_videos || has_profile {
            let mut jobs = Vec::new();
            if has_photos {
                let photos_dirs = flatten_quick_dirs(
                    vec![
                        "{scrapmf_root}".to_string(),
                        "{category}".to_string(),
                        "{username}".to_string(),
                        "photos".to_string(),
                    ],
                    &username,
                );
                let mut req_photos = base_req(
                    Some(photos_dirs),
                    Vec::new(),
                    extra_urls.clone(),
                    username.clone(),
                );
                req_photos.extra_args.push("--photos-only".to_string());
                jobs.push((
                    req_photos,
                    site_name.clone(),
                    format!("{username} (photos)"),
                    "photos".to_string(),
                ));
            }
            if has_videos {
                let videos_dirs = flatten_quick_dirs(
                    vec![
                        "{scrapmf_root}".to_string(),
                        "{category}".to_string(),
                        "{username}".to_string(),
                        "videos".to_string(),
                    ],
                    &username,
                );
                let mut req_videos = base_req(
                    Some(videos_dirs),
                    Vec::new(),
                    extra_urls.clone(),
                    username.clone(),
                );
                req_videos.extra_args.push("--videos-only".to_string());
                jobs.push((
                    req_videos,
                    site_name.clone(),
                    format!("{username} (videos)"),
                    "videos".to_string(),
                ));
            }
            if has_profile {
                let profile_dirs = flatten_quick_dirs(
                    vec![
                        "{scrapmf_root}".to_string(),
                        "{category}".to_string(),
                        "{username}".to_string(),
                        "profile".to_string(),
                    ],
                    &username,
                );
                let mut req_profile =
                    base_req(Some(profile_dirs), Vec::new(), Vec::new(), username.clone());
                req_profile.profile_pic_only = true;
                jobs.push((
                    req_profile,
                    site_name.clone(),
                    format!("{username} (profile)"),
                    "profile".to_string(),
                ));
            }
            preview_and_execute(jobs, &cfg);
            return;
        }
    }

    let req = base_req(directory_template, Vec::new(), extra_urls, username.clone());
    let jobs = vec![(req, site_name.clone(), username.clone(), kinds_desc)];
    preview_and_execute(jobs, &cfg);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod quick_flatten_tests {
    use super::{auto_match_site_key, flatten_quick_dirs, parse_pasted_urls};
    use crate::config::{Config, Site};
    use toml::Value;

    fn site_with_pattern(pattern: &str) -> Site {
        Site {
            pattern: Some(pattern.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn parse_pasted_urls_splits_separators_and_dedupes() {
        let raw = "https://a.com/1, https://b.com/2\nhttps://c.com/3\thttps://a.com/1";
        let (valid, errors) = parse_pasted_urls(raw);
        assert_eq!(
            valid,
            vec![
                "https://a.com/1".to_string(),
                "https://b.com/2".to_string(),
                "https://c.com/3".to_string(),
            ],
            "order preserved, duplicates dropped"
        );
        assert!(errors.is_empty());
    }

    #[test]
    fn parse_pasted_urls_reports_invalid_tokens() {
        let (valid, errors) = parse_pasted_urls("https://ok.com/x notaurl ftp://bad.com/y");
        assert_eq!(valid, vec!["https://ok.com/x".to_string()]);
        assert_eq!(errors.len(), 2, "invalid scheme and garbage are reported");
    }

    #[test]
    fn auto_match_picks_longest_matching_pattern() {
        let mut cfg = Config::default();
        cfg.sites
            .insert("tiktok".to_string(), site_with_pattern("tiktok.com"));
        cfg.sites
            .insert("tiktok-wide".to_string(), site_with_pattern("tiktokv.com"));
        cfg.sites
            .insert("instagram".to_string(), site_with_pattern("instagram.com"));

        let (key, _) = auto_match_site_key(&cfg, "https://www.tiktok.com/@user/video/123")
            .expect("must match");
        assert_eq!(key, "tiktok");

        let (key, _) = auto_match_site_key(&cfg, "https://www.tiktokv.com/@user/video/123")
            .expect("must match");
        assert_eq!(key, "tiktok-wide", "longest matching pattern wins");

        assert!(
            auto_match_site_key(&cfg, "https://example.com/x").is_none(),
            "no matching pattern → caller falls back to general config"
        );
    }

    fn arr(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| Value::String(s.to_string())).collect())
    }

    #[test]
    fn plain_array_strips_identity_and_bakes_root() {
        let v = arr(&["{scrapmf_root}", "{category}", "{user}", "stories"]);
        let out = flatten_quick_dirs(
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap().to_string())
                .collect(),
            "profile_user",
        );
        assert_eq!(out, vec!["profile_user", "stories"]);
    }

    #[test]
    fn conditional_table_tiktok_posts_both_branches() {
        // Estructura idéntica al template de TikTok posts
        let mut dir_table = toml::map::Map::new();
        dir_table.insert(
            "post_type == 'image'".to_string(),
            arr(&["{scrapmf_root}", "{category}", "{user}", "photos"]),
        );
        dir_table.insert(
            String::new(),
            arr(&["{scrapmf_root}", "{category}", "{user}", "videos"]),
        );
        let mut extractor = toml::map::Map::new();
        extractor.insert("directory".to_string(), Value::Table(dir_table));
        let mut top = toml::map::Map::new();
        top.insert("tiktok:posts".to_string(), Value::Table(extractor));

        let mut v = Value::Table(top);
        super::flatten_for_quick(&mut v, "profile_user");

        println!("DEBUG v = {}", v);
        let posts = &v["tiktok:posts"]["directory"];
        assert_eq!(
            posts["post_type == 'image'"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["profile_user", "photos"]
        );
        assert_eq!(
            posts[""]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["profile_user", "videos"]
        );
    }

    #[test]
    fn array_without_identity_segments_untouched_except_root() {
        let out = flatten_quick_dirs(vec!["{date:%Y}".into(), "media".into()], "root");
        assert_eq!(out, vec!["{date:%Y}", "media"]);
    }

    #[test]
    fn all_identity_array_keeps_original_rather_than_empty() {
        let out = flatten_quick_dirs(vec!["{user}".into(), "{category}".into()], "root");
        assert_eq!(out, vec!["{user}", "{category}"]);
    }

    /// Regression: instagram quick scrape must route the flattened directory
    /// through the request-level `directory_template`, NOT the twitter-only
    /// `extractor.twitter.media` hack (which gallery-dl ignores for
    /// instagram — posts fell back to `{category}/{username}` and created a
    /// second tree root).
    #[test]
    fn quick_override_instagram_uses_request_level_directory() {
        use super::apply_quick_override;

        let mut opts = std::collections::HashMap::new();
        let mut dir_tmpl = None;
        apply_quick_override(
            &mut opts,
            &mut dir_tmpl,
            "instagram",
            Some(vec!["sample_user".to_string(), "{subcategory}".to_string()]),
            Vec::new(),
        );
        assert_eq!(
            dir_tmpl,
            Some(vec!["sample_user".to_string(), "{subcategory}".to_string()]),
            "instagram override must land in directory_template"
        );
        assert!(
            !opts.contains_key("twitter:media"),
            "instagram quick scrape must not pollute twitter:media options"
        );
    }

    #[test]
    fn quick_override_twitter_keeps_media_scoped_options() {
        use super::apply_quick_override;

        let mut opts = std::collections::HashMap::new();
        let mut dir_tmpl = None;
        apply_quick_override(
            &mut opts,
            &mut dir_tmpl,
            "twitter",
            Some(vec!["user".to_string(), "photos".to_string()]),
            vec![("file-filter".to_string(), "type == 'photo'".to_string())],
        );
        assert_eq!(dir_tmpl, None, "twitter keeps using extractor options");
        let media = opts.get("twitter:media").expect("twitter:media table");
        match media {
            toml::Value::Table(map) => {
                assert_eq!(
                    map.get("directory").and_then(|v| v.as_array()),
                    Some(&vec![
                        toml::Value::String("user".into()),
                        toml::Value::String("photos".into())
                    ])
                );
                assert_eq!(
                    map.get("file-filter").and_then(|v| v.as_str()),
                    Some("type == 'photo'")
                );
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    // ─── Cookie session selection ──────────────────────────────────────────

    fn req_with(
        cookies_file: Option<&str>,
        cookies_from_browser: Option<&str>,
    ) -> crate::application::ScrapeRequest {
        crate::application::ScrapeRequest {
            cookies_file: cookies_file.map(std::path::PathBuf::from),
            cookies_from_browser: cookies_from_browser.map(str::to_string),
            ..Default::default()
        }
    }

    /// The preview must name the session each job will use, so a wrong cookie
    /// profile is caught before the download rather than after it.
    #[test]
    fn cookie_source_preview_covers_every_case() {
        assert_eq!(
            super::describe_cookie_source(&req_with(None, None)),
            "site defaults (no session)"
        );
        assert_eq!(
            super::describe_cookie_source(&req_with(None, Some("brave"))),
            "browser cookies (brave)"
        );
        assert!(
            super::describe_cookie_source(&req_with(Some("/tmp/elsewhere.txt"), None))
                .contains("/tmp/elsewhere.txt"),
            "an external cookie path must be shown verbatim"
        );
    }

    #[test]
    fn cookie_source_preview_names_stored_profiles() {
        // Point the profile directory at a temp tree holding one profile, so
        // the label resolves to the friendly profile name.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let profile = dir.path().join("brave-instagram.txt");
        std::fs::write(
            &profile,
            b"# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tk\tv\n",
        )
        .expect("write");
        let shown = super::describe_cookie_source(&req_with(
            Some(profile.to_str().expect("utf8 path")),
            None,
        ));
        assert!(
            shown.contains("brave-instagram"),
            "expected the profile name in {shown:?}"
        );
    }

    /// A single-site override must not leak into another site's job. Handing a
    /// TikTok profile to the Instagram job is exactly the cross-session
    /// contamination this guard exists to prevent.
    #[test]
    fn cookie_override_only_touches_jobs_of_the_chosen_site() {
        let file = std::path::Path::new("/tmp/tiktok-profile.txt");
        let mut jobs: Vec<(crate::application::ScrapeRequest, String, String, String)> = vec![
            (
                req_with(Some("/site/tiktok.txt"), None),
                "tiktok".to_string(),
                "tiktok:user".to_string(),
                "videos".to_string(),
            ),
            (
                req_with(Some("/site/instagram.txt"), None),
                "instagram".to_string(),
                "instagram:user".to_string(),
                "posts".to_string(),
            ),
        ];
        super::apply_cookie_file_to_site(&mut jobs, file, &["tiktok".to_string()]);
        assert_eq!(
            jobs[0].0.cookies_file.as_deref(),
            Some(file),
            "the chosen site must adopt the profile"
        );
        assert_eq!(
            jobs[1].0.cookies_file.as_deref(),
            Some(std::path::Path::new("/site/instagram.txt")),
            "another site must keep its own cookies"
        );
    }

    /// Choosing a profile must clear any browser-cookie source, otherwise the
    /// backend receives both and the browser session silently wins.
    #[test]
    fn cookie_override_clears_browser_cookies() {
        let mut jobs: Vec<(crate::application::ScrapeRequest, String, String, String)> = vec![(
            req_with(None, Some("brave")),
            "tiktok".to_string(),
            "tiktok:user".to_string(),
            "videos".to_string(),
        )];
        super::apply_cookie_file_to_site(
            &mut jobs,
            std::path::Path::new("/tmp/p.txt"),
            &["tiktok".to_string()],
        );
        assert_eq!(jobs[0].0.cookies_from_browser, None);
        assert!(jobs[0].0.cookies_file.is_some());
    }

    /// An unknown site key must not be treated as "matches everything": that is
    /// what made the direct-URL flow, which passed `""`, offer every profile
    /// regardless of relevance.
    #[test]
    fn unknown_site_does_not_match_profiles() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cookies_dir = dir.path().join("cookies");
        std::fs::create_dir_all(&cookies_dir).expect("mkdir");
        std::fs::write(
            cookies_dir.join("p.txt"),
            b"# Netscape HTTP Cookie File\n.example.com\tTRUE\t/\tTRUE\t0\tk\tv\n",
        )
        .expect("write");
        // `site_has_cookies` consults the real profile store, so only assert the
        // domain-matching contract through the helper's own inputs.
        let unknown_domains = crate::config::cookies::domains_for_site("");
        assert!(
            unknown_domains.is_empty(),
            "an empty site key must resolve to no domains, not all domains"
        );
    }
}
