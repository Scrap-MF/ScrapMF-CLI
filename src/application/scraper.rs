use url::Url;

use crate::error::ScrapmfError;
use crate::providers::Provider;

/// Validate URL: trim, length ≤2048, Url::parse, allow only http/https.
pub fn validate_url(raw: &str) -> Result<Url, ScrapmfError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(ScrapmfError::InvalidUrl {
            url: "URL is empty".to_string(),
        });
    }
    if s.len() > 2048 {
        return Err(ScrapmfError::InvalidUrl {
            url: format!(
                "URL too long ({} > 2048): {}",
                s.len(),
                &s[..s.len().min(50)]
            ),
        });
    }
    let url = Url::parse(s).map_err(|_| ScrapmfError::InvalidUrl { url: s.to_string() })?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(ScrapmfError::InvalidUrl {
            url: format!("unsupported scheme '{}': {}", other, s),
        }),
    }
}

/// Core scraper service — orchestrates provider + executor.
/// Stub for Phase 4. Will contain business logic without CLI concerns.
#[derive(Clone, Debug, Default)]
pub struct ScrapeOutcome {
    pub success_count: usize,
    pub failed: Vec<String>,
    pub skipped: Vec<String>,
    /// Posts lost to anti-bot JavaScript challenges (rehydration attempts
    /// exhausted). Usually means the browser session is stale — the user
    /// should refresh it before re-running.
    pub challenge_failures: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ScrapeRequest {
    pub url: String,
    pub output: Option<std::path::PathBuf>,
    pub preset: Option<String>,
    pub extra_args: Vec<String>,
    pub cookies_from_browser: Option<String>,
    pub cookies_file: Option<std::path::PathBuf>,
    /// Explicit archive override. When `None`, the automatic per-account
    /// archive (see `crate::application::archive`) kicks in unless
    /// [`Self::no_archive`] is set.
    pub archive: Option<std::path::PathBuf>,
    /// Disable dedup entirely for this request (CLI `--no-archive`).
    pub no_archive: bool,
    pub rate_limit: Option<crate::config::RateLimit>,
    pub extractor_options: std::collections::HashMap<String, toml::Value>,
    pub filename_template: Option<String>,
    pub directory_template: Option<Vec<String>>,
    pub extra_urls: Vec<String>,
    /// Profile name (person) — injected as `scrapmf_root` gallery-dl keyword so
    /// directory templates can start with {scrapmf_root} (profile/network/account/content).
    pub profile_name: Option<String>,
    /// Additional raw `-o key=value` pairs appended AFTER site extractor
    /// options (so they win). Used e.g. for the twitter two-pass photos/videos
    /// split (static directory override + file-filter per pass).
    pub extra_extractor_opts: Vec<(String, String)>,
}

/// System directory trees that must never be used as scrape output.
/// Matched as component-prefixes, so `/etc` also rejects `/etc/anything`.
/// `/opt` is deliberately NOT protected (legitimate content location).
const PROTECTED_ROOTS: &[&str] = &[
    "/", "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/proc", "/root", "/run",
    "/sbin", "/sys", "/usr", "/var",
];

/// Validate output path: reject empty, protected system trees (including any
/// path inside them), and ParentDir `..`.
pub fn validate_output_path(path: &std::path::Path) -> anyhow::Result<()> {
    use std::path::{Component, Path};
    if path.as_os_str().is_empty() {
        anyhow::bail!("output path is empty");
    }
    let s = path.to_string_lossy();
    let expanded = if let Some(stripped) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            home.join(stripped)
        } else {
            path.to_path_buf()
        }
    } else {
        path.to_path_buf()
    };
    let expanded = Path::new(&expanded);
    // Prefix match on components: `/etc` protects `/etc/passwd_backup` too.
    // The bare filesystem root `/` is special-cased: it only rejects the root
    // itself, since every absolute path technically starts with it.
    for root in PROTECTED_ROOTS {
        let protected = if *root == "/" {
            expanded == Path::new("/")
        } else {
            expanded.starts_with(root)
        };
        if protected {
            anyhow::bail!(
                "output path '{}' is inside protected system directory '{root}'",
                path.display()
            );
        }
    }
    if expanded
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        anyhow::bail!(
            "output path '{}' contains '..' (directory traversal not allowed)",
            path.display()
        );
    }
    Ok(())
}

const ALLOW_LIST: &[&str] = &[
    "--proxy",
    "--user-agent",
    "--sleep",
    "--sleep-request",
    "--sleep-429",
    "--limit-rate",
    "--cookies",
    "--cookies-from-browser",
    "--restrict-filenames",
    "--destination",
    "--get-urls",
    // Download robustness (flaky CDNs, large videos): retry/timeout tuning
    "--retries",
    "--http-timeout",
    "--force-ipv4",
];

pub fn validate_extra_args(args: &[String]) -> anyhow::Result<()> {
    for arg in args {
        if arg.contains([';', '|', '&', '$', '>', '<', '`']) {
            anyhow::bail!("extra_args contains forbidden chars in '{arg}'");
        }
        if arg == "--exec" || arg.starts_with("--exec=") {
            anyhow::bail!("extra_args forbidden flag '--exec'");
        }
        if arg.starts_with("--") {
            let allowed = ALLOW_LIST
                .iter()
                .any(|allow| arg == *allow || arg.starts_with(&format!("{allow}=")));
            if !allowed {
                anyhow::bail!(
                    "extra_args forbidden flag '{arg}' (allowed: {})",
                    ALLOW_LIST.join(", ")
                );
            }
        }
    }
    Ok(())
}

pub fn validate_cookies_file(path: &std::path::Path) -> anyhow::Result<()> {
    if !path.is_file() {
        anyhow::bail!("cookies file '{}' not found or not a file", path.display());
    }
    // Check readable and not world-readable is optional warn, but check exists
    let md = std::fs::metadata(path)?;
    if md.len() == 0 {
        anyhow::bail!("cookies file '{}' is empty", path.display());
    }
    Ok(())
}

pub fn validate_cookies_browser(browser: &str) -> anyhow::Result<()> {
    let id = browser.split(':').next().unwrap_or(browser);
    let allowed = [
        "firefox", "brave", "chrome", "chromium", "edge", "opera", "vivaldi", "whale",
    ];
    if !allowed.contains(&id) {
        anyhow::bail!(
            "unsupported browser '{id}' for --cookies-from-browser (allowed: {})",
            allowed.join(", ")
        );
    }
    Ok(())
}

/// True when a gallery-dl stderr line reports a post whose rehydration data
/// could NOT be retrieved after exhausting all attempts — e.g.
/// `@user: Failed to retrieve rehydration data (10/10)`.
/// That post is lost for this run; the usual cause is an expired/flagged
/// browser session being JavaScript-challenged on every request.
pub fn is_challenge_exhausted_line(line: &str) -> bool {
    let Some((_, tail)) = line.split_once("Failed to retrieve rehydration data") else {
        return false;
    };
    // Look for "(N/M)" anywhere after the message; exhausted iff N == M > 0.
    let bytes = tail.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'('
            && let Some(close) = tail[i..].find(')')
        {
            let inner = &tail[i + 1..i + close];
            if let Some((cur, max)) = inner.split_once('/')
                && let (Ok(c), Ok(m)) = (cur.trim().parse::<u32>(), max.trim().parse::<u32>())
            {
                return c == m && c > 0;
            }
        }
    }
    false
}

/// Optional UI hooks so the presentation layer (indicatif/log panel) can
/// observe progress without application knowing about any UI crate.
pub struct ScrapeHooks<'a> {
    /// Called once per downloaded/skipped media file with its stdout line
    /// (the file path printed by gallery-dl).
    pub on_file: Box<dyn FnMut(&str) + 'a>,
    /// Called per gallery-dl stderr line (warnings, errors, notices).
    pub on_log: Box<dyn FnMut(&str) + 'a>,
}

pub fn scrape(req: &ScrapeRequest, dry_run: bool) -> anyhow::Result<ScrapeOutcome> {
    let never = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    scrape_with_hooks(req, dry_run, None, &never)
}

pub fn scrape_with_hooks(
    req: &ScrapeRequest,
    dry_run: bool,
    mut hooks: Option<&mut ScrapeHooks>,
    abort: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<ScrapeOutcome> {
    validate_extra_args(&req.extra_args)?;
    if let Some(ref f) = req.cookies_file {
        validate_cookies_file(f)?;
    }
    if let Some(ref b) = req.cookies_from_browser {
        validate_cookies_browser(b)?;
    }
    if let Some(ref out) = req.output {
        validate_output_path(out)?;
        if !dry_run {
            let expanded = crate::config::expand_output_dir(out);
            std::fs::create_dir_all(&expanded)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perm = std::fs::Permissions::from_mode(0o755);
                let _ = std::fs::set_permissions(&expanded, perm);
            }
        }
    }

    // ── Download archive (dedup) ────────────────────────────────────────────
    // Canonical store is ours (JSONL per site/account); the sqlite passed to
    // gallery-dl is a disposable cache seeded from it before the run and
    // drained back into the JSONL afterwards.
    let mut req = req.clone();
    let mut archive_keys: std::collections::HashSet<String> = Default::default();
    let mut archive_files: Option<(std::path::PathBuf, std::path::PathBuf)> = None;
    if !req.no_archive && req.archive.is_none() && !dry_run {
        use crate::application::archive;
        let enabled = crate::config::load()
            .map(|c| c.general.archive)
            .unwrap_or(true);
        if enabled
            && let Some((site, account)) = archive::site_account_from_url(&req.url)
            && let (Some(entries), Some(cache)) = (
                archive::entries_path(&site, &account),
                archive::cache_path(&site, &account),
            )
        {
            archive_keys = archive::load_keys(&entries).unwrap_or_default();
            match archive::seed_cache(&cache, &archive_keys) {
                Ok(()) => {
                    tracing::debug!(
                        site = %site, account = %account,
                        keys = archive_keys.len(),
                        "archive cache seeded"
                    );
                    req.archive = Some(cache.clone());
                    archive_files = Some((entries, cache));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not seed download archive — proceeding without dedup");
                }
            }
        }
    }

    let mut outcome = ScrapeOutcome::default();
    // Instagram: when dedup_stories_from_highlights is true, run highlights
    // before stories so the archive already contains highlight media_ids
    // and stories with the same media_id are skipped automatically.
    let urls: Vec<&String> = {
        let mut v: Vec<&String> = std::iter::once(&req.url)
            .chain(req.extra_urls.iter())
            .collect();
        let dedup_enabled = crate::config::load()
            .ok()
            .and_then(|cfg| {
                crate::config::resolve_site(&req.url, req.preset.as_deref(), &cfg)
                    .map(|(_, s)| s.dedup_stories_from_highlights.unwrap_or(false))
            })
            .unwrap_or(false);
        if dedup_enabled
            && v.iter().any(|u| is_highlights_url(u))
            && v.iter().any(|u| is_stories_url(u))
        {
            v.sort_by_key(|u| {
                if is_highlights_url(u) {
                    0
                } else if is_stories_url(u) {
                    2
                } else {
                    1
                }
            });
            tracing::debug!("dedup_stories_from_highlights: reordered highlights before stories");
        }
        v
    };
    for sub_url in urls {
        match run_one_sub_scrape(&req, sub_url, dry_run, hooks.as_deref_mut(), abort) {
            Ok(challenge_failures) => {
                outcome.success_count += 1;
                outcome.challenge_failures += challenge_failures;
            }
            Err(e) => {
                let msg = e.to_string();
                match classify_failure(&msg) {
                    FailureKind::AuthRequired => {
                        outcome.skipped.push(format!("{sub_url}: {msg}"));
                        tracing::warn!(
                            url = %sub_url,
                            error = %e,
                            "sub-extractor skipped (auth required)"
                        );
                    }
                    FailureKind::Unsupported => {
                        outcome
                            .skipped
                            .push(format!("[unsupported] {sub_url}: {msg}"));
                        tracing::warn!(
                            url = %sub_url,
                            error = %e,
                            "sub-extractor skipped (unsupported URL)"
                        );
                    }
                    FailureKind::Unknown => {
                        outcome.failed.push(format!("{sub_url}: {msg}"));
                        tracing::debug!(url = %sub_url, error = %e, "sub-extractor failed");
                    }
                }
            }
        }
    }
    if outcome.success_count == 0 && outcome.failed.is_empty() && outcome.skipped.is_empty() {
        // No sub-url succeeded and none reported — treat as overall failure
        anyhow::bail!("all sub-extractors failed or were skipped");
    }
    // If at least one succeeded, return outcome even if some failed/skipped
    if outcome.success_count == 0 && !outcome.failed.is_empty() {
        // All failed (no skipped) → propagate error
        let details = outcome.failed.join("; ");
        anyhow::bail!("scrape failed: {details}");
    }

    // Drain the cache: keys gallery-dl inserted this run become permanent
    // JSONL entries. Best-effort — a failure here must not fail the scrape.
    if let Some((entries, cache)) = archive_files {
        match crate::application::archive::drain_cache(&cache) {
            Ok(all) => {
                match crate::application::archive::append_entries(&entries, &archive_keys, all) {
                    Ok(n) if n > 0 => {
                        tracing::info!(new_entries = n, "download archive updated");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "could not append archive entries"),
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not read download archive cache"),
        }
    }
    Ok(outcome)
}

/// Why a sub-extractor run did not produce downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// gallery-dl needs a valid session (cookies expired / private content).
    AuthRequired,
    /// No extractor can handle the URL — retrying with cookies won't help.
    Unsupported,
    /// Anything else: network errors, extractor bugs, unexpected output.
    Unknown,
}

/// Classify a sub-extractor error message using gallery-dl's real failure
/// signatures. Conservative: only well-known patterns map to the benign
/// categories; everything else is treated as a hard failure.
pub fn classify_failure(msg: &str) -> FailureKind {
    // gallery-dl raises its own AuthRequired exception for these cases.
    if msg.contains("AuthRequired")
        || msg.contains("401 Unauthorized")
        || msg.contains("403 Forbidden")
        || msg.contains("login required")
        // TikTok private profiles: webapp.user-detail returns statusCode
        // 10222 and gallery-dl logs this exact sentence (the profile-page
        // fetch fails even for accounts your session follows — the story
        // list extractor needs the authorId from that page). Direct
        // /video/<id> URLs still work; surface as auth-skip, not a crash.
        || msg.contains("Login required to access this profile")
        || msg.contains("private account")
        || msg.contains("authentication required")
    {
        return FailureKind::AuthRequired;
    }
    if msg.contains("Unsupported URL") || msg.contains("no suitable extractor") {
        return FailureKind::Unsupported;
    }
    FailureKind::Unknown
}

/// True when `url` is the Instagram profile root — the "posts" pass.
///
/// gallery-dl maps this URL to the dispatching user extractor, whose primary
/// sub-extractor (`instagram.posts`) reads the feed endpoint. Instagram mixes
/// reels into that feed, so without a filter every reel downloads twice:
/// once into `posts/` here and again into `reels/` in the dedicated pass
/// (different directories defeat gallery-dl's existing-file skip).
pub fn is_instagram_posts_pass(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let Some(path) = rest.strip_prefix("instagram.com/") else {
        return false;
    };
    // Drop query string / fragment, then require exactly one path segment
    // (the username), optional trailing slash. Sub-pages like USER/reels/,
    // USER/avatar or stories/USER/ contain '/'.
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let seg = path.trim_end_matches('/');
    !seg.is_empty() && !seg.contains('/')
}

pub fn is_highlights_url(url: &str) -> bool {
    url.contains("highlights")
}

pub fn is_stories_url(url: &str) -> bool {
    url.contains("/stories/") && !is_highlights_url(url)
}

/// Per-sub-url overrides applied right before building argv.
///
/// Instagram dedup (always on): scope a post-filter to the
/// `instagram.posts` sub-extractor only. Posts carrying metadata
/// `type == "reel"` are skipped here; they download exactly once, from the
/// dedicated `/USER/reels/` pass. The filter is scoped per-subcategory so a
/// queued dispatch inside the same run cannot suppress the other extractors,
/// and missing `type` (e.g. GraphQL path) evaluates to keep — fail-open: worst
/// case is a duplicate download, never lost content.
fn apply_sub_url_overrides(req: &mut ScrapeRequest, url: &str) {
    if is_instagram_posts_pass(url) {
        req.extra_extractor_opts.push((
            "extractor.instagram.posts.post-filter".to_string(),
            "type != 'reel'".to_string(),
        ));
    }
}

fn run_one_sub_scrape(
    req: &ScrapeRequest,
    url: &str,
    dry_run: bool,
    hooks: Option<&mut ScrapeHooks>,
    abort: &std::sync::atomic::AtomicBool,
) -> anyhow::Result<usize> {
    let provider = crate::providers::gallery_dl::GalleryDl;
    // Resolved binary (bundled pinned copy by default, never the user's own)
    let binary = crate::providers::gallery_dl::GalleryDl::binary()?
        .to_string_lossy()
        .into_owned();
    // Build args with the specific sub-url
    let mut sub_req = req.clone();
    sub_req.url = url.to_string();
    apply_sub_url_overrides(&mut sub_req, url);
    let mut args = provider.build_args(&sub_req)?;
    if dry_run {
        args.push(std::ffi::OsString::from("--get-urls"));
        crate::process::Executor::run_inherited_checked(&binary, &args)?;
        return Ok(0);
    }

    // Count posts lost to anti-bot JS challenges (rehydration exhausted).
    let challenge_failures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    match hooks {
        Some(hooks) => {
            let on_file = &mut hooks.on_file;
            let on_log = &mut hooks.on_log;
            let counter = challenge_failures.clone();
            crate::process::Executor::run_streaming(
                &binary,
                &args,
                abort,
                // one stdout line per downloaded/skipped file (the path)
                &mut |line| on_file(line),
                &mut |line| {
                    tracing::debug!(target = "gallery-dl", "{line}");
                    if is_challenge_exhausted_line(line) {
                        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    on_log(line);
                },
            )?;
        }
        None => {
            // No UI attached — inherit stdio (raw output, CI-friendly).
            // stderr lines are not captured here, so challenge counting is
            // unavailable in this mode.
            crate::process::Executor::run_inherited_checked(&binary, &args)?;
        }
    }
    Ok(challenge_failures.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        FailureKind, ScrapeRequest, apply_sub_url_overrides, classify_failure,
        is_instagram_posts_pass, validate_cookies_browser, validate_cookies_file,
        validate_extra_args, validate_output_path, validate_url,
    };
    use std::io::Write;

    /// TikTok private-but-followed profiles: the story list extractor dies
    /// fetching webapp.user-detail (statusCode 10222) even though the session
    /// is valid — direct /video/<id> URLs work. Must surface as an auth skip,
    /// not a hard failure.
    #[test]
    fn classifies_tiktok_private_profile_login_wall_as_auth() {
        let msg = "scrape failed: https://www.tiktok.com/@demo_user/stories: \
backend '/home/sebas/.local/share/scrapmf/bin/gallery-dl.bin' failed with exit code Some(4)\n  \
stderr: [tiktok][error] https://www.tiktok.com/@demo_user: Login required to access this \
profile, or this profile has no videos posted";
        assert!(matches!(classify_failure(msg), FailureKind::AuthRequired));
    }

    #[test]
    fn instagram_posts_pass_matches_profile_root() {
        assert!(is_instagram_posts_pass("https://www.instagram.com/user/"));
        assert!(is_instagram_posts_pass("https://instagram.com/user/"));
        assert!(is_instagram_posts_pass("https://www.instagram.com/user"));
        assert!(is_instagram_posts_pass("http://www.instagram.com/user/"));
        // query string still identifies the profile-root pass
        assert!(is_instagram_posts_pass(
            "https://www.instagram.com/user/?hl=en"
        ));
    }

    #[test]
    fn instagram_posts_pass_rejects_subpages_and_other_sites() {
        // dedicated passes must NOT get the posts filter
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/user/reels/"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/user/highlights/"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/user/avatar"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/stories/user/"
        ));
        assert!(!is_instagram_posts_pass(
            "https://www.instagram.com/p/Cxyz123/"
        ));
        // other sites / schemes
        assert!(!is_instagram_posts_pass(
            "https://www.tiktok.com/@user/posts"
        ));
        assert!(!is_instagram_posts_pass("ftp://instagram.com/user/"));
        assert!(!is_instagram_posts_pass("not a url"));
    }

    fn base_req() -> ScrapeRequest {
        ScrapeRequest {
            no_archive: false,
            url: String::new(),
            output: None,
            preset: Some("instagram".to_string()),
            extra_args: vec![],
            cookies_from_browser: None,
            cookies_file: None,
            archive: None,
            rate_limit: None,
            extractor_options: Default::default(),
            filename_template: None,
            directory_template: None,
            extra_urls: vec![],
            profile_name: None,
            extra_extractor_opts: vec![],
        }
    }

    #[test]
    fn sub_url_overrides_inject_dedup_filter_only_for_posts_pass() {
        let mut req = base_req();

        apply_sub_url_overrides(&mut req, "https://www.instagram.com/user/");
        assert_eq!(
            req.extra_extractor_opts,
            vec![(
                "extractor.instagram.posts.post-filter".to_string(),
                "type != 'reel'".to_string()
            )],
            "posts pass must carry the dedup post-filter"
        );

        for url in [
            "https://www.instagram.com/user/reels/",
            "https://www.instagram.com/user/highlights/",
            "https://www.instagram.com/stories/user/",
        ] {
            let mut r = base_req();
            apply_sub_url_overrides(&mut r, url);
            assert!(
                r.extra_extractor_opts.is_empty(),
                "{url} must not get the posts dedup filter"
            );
        }
    }

    #[test]
    fn dedup_filter_lands_in_built_args() {
        use crate::providers::Provider;
        use crate::providers::gallery_dl::GalleryDl;

        let mut req = base_req();
        req.url = "https://www.instagram.com/user/".to_string();
        let url = req.url.clone();
        apply_sub_url_overrides(&mut req, &url);

        let args = GalleryDl.build_args(&req).unwrap();
        let strs: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(
            strs.contains(&"-o".to_string()),
            "-o flag missing in {strs:?}"
        );
        assert!(
            strs.iter()
                .any(|a| a == "extractor.instagram.posts.post-filter=type != 'reel'"),
            "scoped post-filter missing in {strs:?}"
        );
    }

    #[test]
    fn validate_url_accepts_https() {
        assert!(validate_url("https://example.com").is_ok());
        assert!(validate_url("https://example.com/gallery/123").is_ok());
        assert!(validate_url("  https://example.com  ").is_ok());
    }

    #[test]
    fn validate_url_accepts_http() {
        assert!(validate_url("http://example.com").is_ok());
    }

    #[test]
    fn validate_url_rejects_javascript() {
        assert!(validate_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn validate_url_rejects_file_scheme() {
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn validate_url_rejects_empty() {
        assert!(validate_url("").is_err());
        assert!(validate_url("   ").is_err());
    }

    #[test]
    fn validate_url_rejects_too_long() {
        let long = format!("https://example.com/{}", "a".repeat(2048));
        assert!(validate_url(&long).is_err());
    }

    #[test]
    fn validate_url_rejects_invalid() {
        assert!(validate_url("not a url").is_err());
    }

    #[test]
    fn validate_extra_args_allows_valid() {
        assert!(validate_extra_args(&["--proxy".to_string(), "http://a.com".to_string()]).is_ok());
        assert!(validate_extra_args(&["--sleep".to_string(), "1".to_string()]).is_ok());
        assert!(validate_extra_args(&["--cookies".to_string()]).is_ok());
        // Download robustness flags (large videos, flaky CDNs)
        assert!(
            validate_extra_args(&["--retries".to_string(), "10".to_string()]).is_ok(),
            "--retries must be allowed"
        );
        assert!(
            validate_extra_args(&["--http-timeout".to_string(), "120".to_string()]).is_ok(),
            "--http-timeout must be allowed"
        );
        assert!(validate_extra_args(&["--force-ipv4".to_string()]).is_ok());
        assert!(validate_extra_args(&[]).is_ok());
    }

    #[test]
    fn validate_extra_args_rejects_exec() {
        assert!(validate_extra_args(&["--exec".to_string()]).is_err());
        assert!(validate_extra_args(&["--exec=rm -rf".to_string()]).is_err());
    }

    #[test]
    fn validate_extra_args_rejects_forbidden_flag() {
        assert!(validate_extra_args(&["--unknown-flag".to_string()]).is_err());
        assert!(validate_extra_args(&["--proxy; rm".to_string()]).is_err());
    }

    #[test]
    fn validate_extra_args_rejects_forbidden_chars() {
        assert!(validate_extra_args(&["--proxy; rm -rf".to_string()]).is_err());
        assert!(validate_extra_args(&["--user-agent | cat".to_string()]).is_err());
    }

    #[test]
    fn validate_cookies_browser_accepts() {
        assert!(validate_cookies_browser("firefox").is_ok());
        assert!(validate_cookies_browser("firefox:profile").is_ok());
        assert!(validate_cookies_browser("brave").is_ok());
        assert!(validate_cookies_browser("chrome:Default").is_ok());
    }

    #[test]
    fn validate_cookies_browser_rejects() {
        assert!(validate_cookies_browser("invalidbrowser").is_err());
        assert!(validate_cookies_browser("firefox; rm").is_err());
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn validate_cookies_file_accepts() {
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let _ = writeln!(tmp, "# Netscape HTTP Cookie File");
        assert!(validate_cookies_file(tmp.path()).is_ok());
    }

    #[test]
    fn validate_cookies_file_rejects_missing() {
        assert!(
            validate_cookies_file(std::path::Path::new("/tmp/fake_cookies_12345.txt")).is_err()
        );
    }

    #[test]
    fn failure_classification() {
        use super::{FailureKind, classify_failure};
        // AuthRequired: real gallery-dl signatures
        assert_eq!(
            classify_failure(
                "scrape failed: backend 'gallery-dl' failed ... AuthRequired exception"
            ),
            FailureKind::AuthRequired
        );
        assert_eq!(
            classify_failure("HTTP Error 401 Unauthorized"),
            FailureKind::AuthRequired
        );
        assert_eq!(
            classify_failure("403 Forbidden for url https://x.com/i/1"),
            FailureKind::AuthRequired
        );
        assert_eq!(
            classify_failure("login required to view this profile"),
            FailureKind::AuthRequired
        );
        // Unsupported
        assert_eq!(
            classify_failure("'https://example.com/x': Unsupported URL"),
            FailureKind::Unsupported
        );
        assert_eq!(
            classify_failure("no suitable extractor found"),
            FailureKind::Unsupported
        );
        // Unknown — including the old false positives:
        // a bare "403" inside unrelated text must NOT be auth...
        assert_ne!(
            classify_failure("connection to port 403 failed"),
            FailureKind::AuthRequired
        );
        // ...and 'cookies' mentions in generic network errors are hard failures now.
        assert_eq!(
            classify_failure("error reading cookies jar file: no such file"),
            FailureKind::Unknown
        );
        assert_eq!(
            classify_failure("Connection reset by peer"),
            FailureKind::Unknown
        );
        assert_eq!(classify_failure(""), FailureKind::Unknown);
    }

    #[test]
    fn challenge_exhausted_detection() {
        // Exhausted attempts → true
        assert!(super::is_challenge_exhausted_line(
            "[tiktok][warning] @user: Failed to retrieve rehydration data (10/10)"
        ));
        assert!(super::is_challenge_exhausted_line(
            "123: Failed to retrieve rehydration data (4/4)"
        ));
        // Still retrying → false
        assert!(!super::is_challenge_exhausted_line(
            "@user: Failed to retrieve rehydration data (1/10)"
        ));
        assert!(!super::is_challenge_exhausted_line(
            "@user: Failed to retrieve rehydration data (3/4)"
        ));
        // Unrelated lines → false
        assert!(!super::is_challenge_exhausted_line(
            "Failed to solve JavaScript challenge"
        ));
        assert!(!super::is_challenge_exhausted_line(
            "[download] file.mp4 done"
        ));
        assert!(!super::is_challenge_exhausted_line(
            "Failed to retrieve rehydration data (0/0)"
        ));
    }

    #[test]
    fn validate_output_path_rejects_forbidden() {
        assert!(validate_output_path(std::path::Path::new("/")).is_err());
        assert!(validate_output_path(std::path::Path::new("/etc")).is_err());
        assert!(validate_output_path(std::path::Path::new("../out")).is_err());
    }

    #[test]
    fn validate_output_path_protects_system_trees_not_just_exact_matches() {
        // Anything INSIDE a protected tree is rejected too
        for p in [
            "/etc/passwd_backup",
            "/var/lib/scrapmf",
            "/usr/local/share/out",
            "/boot/vmlinuz.d",
            "/root/downloads",
            "/proc/self/out",
            "/sys/x",
            "/dev/null-dir",
            "/run/media",
            "/bin/sh-out",
            "/lib64/x",
        ] {
            assert!(
                validate_output_path(std::path::Path::new(p)).is_err(),
                "{p} must be rejected"
            );
        }
    }

    #[test]
    fn validate_output_path_allows_legitimate_locations() {
        for p in [
            "/tmp/scrapmf-out",
            "/opt/data/archives",
            "/home/someone/pics",
            "./downloads",
            "~/Pictures/scrapmf",
            "relative/out",
        ] {
            assert!(
                validate_output_path(std::path::Path::new(p)).is_ok(),
                "{p} must be allowed"
            );
        }
    }
}
