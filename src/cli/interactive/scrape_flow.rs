use std::path::PathBuf;
use std::sync::Arc;

use inquire::{Confirm, Text};

use crate::application::scraper::{ScrapeRequest, validate_url};
use crate::config;

use super::content::{ContentKind, build_tagged_urls, prompt_content_kinds, select_urls};
use super::{ask_nonempty, select_menu};

/// Ask whether this run should use a named cookie profile instead of the
/// site defaults. Returns the profile path when overridden.
pub(super) fn prompt_cookie_override(site: &str) -> Option<PathBuf> {
    use crate::config::cookies;
    let profiles = cookies::list_profiles();
    // Filter profiles to those that actually contain cookies for this site
    let site_domains = cookies::domains_for_site(site);
    let filtered: Vec<String> = profiles
        .into_iter()
        .filter(|name| {
            if site_domains.is_empty() {
                return true;
            }
            match cookies::load_profile(name) {
                Ok(cookies) => cookies.iter().any(|c| {
                    site_domains
                        .iter()
                        .any(|d| c.domain == *d || c.domain.ends_with(&format!(".{d}")))
                }),
                Err(_) => false,
            }
        })
        .collect();
    // Always offer at least Default, even if no matching profiles
    let mut opts = vec!["Default (from site config)".to_string()];
    opts.extend(
        filtered
            .iter()
            .map(|p| format!("{p}  — {}", cookies::profile_summary(p).unwrap_or_default())),
    );
    // If only Default and no filtered profiles, still prompt so user sees the choice
    // (previous behavior returned None without prompting, which hid the cookie step)
    let Ok(choice) = select_menu("Cookies for this run?", opts).prompt() else {
        return None;
    };
    if choice.starts_with("Default") {
        return None;
    }
    let name = choice.split("  — ").next()?.trim().to_string();
    cookies::profile_path(&name).filter(|p| p.exists())
}

/// Force a cookie file onto every job (used by the per-run cookie override).
fn apply_cookie_file(jobs: &mut [(ScrapeRequest, String, String, String)], file: &std::path::Path) {
    for (req, ..) in jobs.iter_mut() {
        req.cookies_file = Some(file.to_path_buf());
        req.cookies_from_browser = None;
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
    let raw = match Text::new("Paste URL(s) — separate with spaces or commas:")
        .with_render_config(super::theme::render_config())
        .with_placeholder(
            "https://www.tiktok.com/@user/video/123 https://www.instagram.com/reel/xyz/",
        )
        .with_help_message("each URL is matched against your sites/*.toml patterns")
        .prompt()
    {
        Ok(t) => t,
        Err(_) => {
            println!("canceled");
            return;
        }
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

    // Per-run cookie override (named profile instead of site defaults)
    let cookie_override = if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        prompt_cookie_override("")
    } else {
        None
    };
    if let Some(ref file) = cookie_override {
        apply_cookie_file(&mut jobs, file);
    }

    preview_and_execute(jobs, &cfg);
}

/// Stems of sites/*.toml files, sorted, with the four known-network
/// fallbacks appended when missing. Shared by quick scrape and profiles.
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

    let items: Vec<crate::cli::interactive::theme::SiteItem> = site_opts
        .into_iter()
        .map(crate::cli::interactive::theme::SiteItem::new)
        .collect();
    let Ok(selected) = super::select_menu("Site:", items).prompt() else {
        return;
    };
    let site_name = selected.key();
    let prompt_text = if site_name == "facebook" {
        "ID o URL del perfil (ej. 123..., https://www.facebook.com/profile.php?id=...):"
    } else if site_name == "instagram" {
        "Username or ID (without @):"
    } else {
        "Username (without @):"
    };
    let raw_input = match ask_nonempty(prompt_text) {
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
        prompt_cookie_override(&site_name)
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
}
