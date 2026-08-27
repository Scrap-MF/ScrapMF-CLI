//! Generated site/profile template files (sites/*.toml, profiles/*.toml).
use std::path::Path;

use anyhow::Context;

use super::fs::write_config_file;
use super::migrations::{migrate_highlights_if_needed, migrate_tiktok_robustness};
use super::{Profile, RateLimit, Site, sites_dir};

pub(crate) fn tiktok_posts_conditional_directory() -> toml::Value {
    let seg = |s: &str| toml::Value::String(s.to_string());
    let dir = |last: &str| {
        toml::Value::Array(vec![
            seg("{scrapmf_root}"),
            seg("{category}"),
            seg("{user}"),
            seg(last),
        ])
    };
    let mut m = toml::map::Map::new();
    m.insert("post_type == 'image'".to_string(), dir("photos"));
    // default: videos (audio tracks disabled site-wide via audio=false)
    m.insert(String::new(), dir("videos"));
    toml::Value::Table(m)
}

/// Ensure sites/instagram.toml example exists with 0o600 (no clobber).
pub fn ensure_example_sites() -> anyhow::Result<()> {
    let Some(dir) = sites_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    crate::config::fs::restrict_perms(&dir, true);
    let target = dir.join("instagram.toml");
    if target.exists() {
        // Migrate existing instagram.toml to new highlights rule (no date, highlight_id, media_id only)
        let _ = migrate_highlights_if_needed(&target);
        return Ok(());
    }
    let mut extractor = std::collections::HashMap::new();
    // Instagram highlights: use sub-extractor override with colon (gallery-dl 1.32.9: instagram:highlights)
    // HIGHLIGHTS rule (specific): no date in filename/dir, identity via highlight_id+title / media_id
    //   instagram/highlights/{post_id}{highlight_title:?_//}/{media_id}.{extension} (inside instagram, same level as posts)
    //   archive handles dedup (no deterministic date fallback needed)
    // gallery-dl keyword for highlight_id is `post_id` (reel_id, e.g. 180...), `highlight_id` alias not guaranteed — use post_id
    let mut highlight_table = toml::map::Map::new();
    highlight_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("highlights".to_string())]),
    );
    highlight_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("highlights".to_string()),
            toml::Value::String("{post_id}{highlight_title:?_//}".to_string()),
        ]),
    );
    highlight_table.insert(
        "filename".to_string(),
        toml::Value::String("{media_id}.{extension}".to_string()),
    );
    extractor.insert(
        "instagram:highlights".to_string(),
        toml::Value::Table(highlight_table),
    );
    // Instagram stories (ephemeral 24h): date-based tree, all lowercase
    //   instagram/stories/{year}/{month-lowercase}/{date}_{num}.{ext}
    //   e.g. example_user/instagram/stories/2026/10-october/2026-10-28_01.mp4
    // NOTE: stories filename uses {date} + tray position ({num}) — NOT
    // {post_id}/{media_id}: for stories both are stable per user, so two
    // different days computed identical names and existing files silently
    // skipped the new day's stories (false dedup).
    // gallery-dl strftime cannot lowercase %B natively; use f-string formatter prefix
    // `\fF` (\f = form feed escape in JSON value, F = FStringFormatter) with Python strftime:
    //   "\fF {date.strftime(\"%m-%B\").lower()}" → "10-october"
    // NOTE: the \f here is the two-character sequence backslash+f (JSON escape), not a control char.
    let stories_year = "\\fF {date.strftime(\"%Y\")}".to_string();
    let stories_month = "\\fF {date.strftime(\"%m-%B\").lower()}".to_string();
    let mut stories_table = toml::map::Map::new();
    stories_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("stories".to_string())]),
    );
    stories_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("stories".to_string()),
            toml::Value::String(stories_year),
            toml::Value::String(stories_month),
        ]),
    );
    stories_table.insert(
        "filename".to_string(),
        toml::Value::String("{date:%Y-%m-%d}_{num:02d}.{extension}".to_string()),
    );
    extractor.insert(
        "instagram:stories".to_string(),
        toml::Value::Table(stories_table),
    );
    // Instagram avatar (profile pic): same pattern as tiktok avatar.
    // {date} is invalid for avatars ([Invalid DateTime]); {media_id}
    // (= profile_pic_id) changes when the avatar changes — deterministic
    // filename, gallery-dl skips existing files, history kept per version.
    let mut avatar_table = toml::map::Map::new();
    avatar_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("profile".to_string()),
        ]),
    );
    avatar_table.insert(
        "filename".to_string(),
        toml::Value::String("{username}_{media_id}.{extension}".to_string()),
    );
    extractor.insert(
        "instagram:avatar".to_string(),
        toml::Value::Table(avatar_table),
    );
    let site = Site {
        site: Some("instagram".to_string()),
        pattern: Some("instagram.com".to_string()),
        patterns: Vec::new(),
        output_dir: None,
        // gallery-dl native sanitization (replaces / : ? * etc.) — do not implement custom logic
        extra_args: vec!["--restrict-filenames".to_string(), "auto".to_string()],
        cookies: None,
        cookies_from_browser: Some("brave".to_string()),
        cookie_profile: None,
        rate_limit: Some(RateLimit {
            sleep: Some("3-6".to_string()),
            sleep_request: Some("8-15".to_string()),
            sleep_429: Some(120),
            limit_rate: None,
        }),
        archive: None,
        extractor,
        // Naming: posts/reels → post_id, stories/highlights inner → media_id, date %Y-%m-%d verified via gallery-dl -K
        // _num:02d preserves carrousel order (01,02,03) with _ separator (only for posts/reels, NOT highlights)
        // HIGHLIGHTS: {media_id}.{extension} inside highlights/{post_id}{highlight_title:?_//}/ — no date, no num (archive dedup); raw title kept visible (emoji included), empty titles collapse to just {post_id}
        filename_template: Some("{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}".to_string()),
        // Tree: {scrapmf_root}/{category}/{username}/{subcategory} → example/instagram/example_user/posts
        // {scrapmf_root} = profile name injected via extractor.keywords at runtime (fallback "default")
        // Highlights: .../highlights/{post_id}{highlight_title:?_//}/{media_id}.{ext}
        // Verified: gallery-dl exposes {post_id}, {highlight_id}, {media_id}, {highlight_title}, {date}, {category}, {subcategory}, {username}
        // {site} not exposed by gallery-dl, but {category}=site (instagram), {subcategory}=posts/reels/stories/highlights
        directory_template: Some(vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{username}".to_string(),
            "{subcategory}".to_string(),
        ]),
    };
    let body = toml::to_string_pretty(&site).context("serialize instagram site")?;
    let header = r#"# scrapmf — site config (1 file per site)
# File: ~/.config/scrapmf/sites/<name>.toml (0o600, dir 0o700)
# Auto-matched when URL contains `pattern`. CLI --preset can also force it.
#
# Instagram strategy — structured tree (verified via gallery-dl -K: category=instagram, subcategory=posts/reels/highlights):
#   PERFIL/  (scrapmf_root = profile name, e.g. example; output default ~/scrapmf)
#   └── instagram/  (category)
#       └── CUENTA/  ({username} account handle)
#       ├── posts/    → {date:%Y-%m-%d}_{post_id}_{num:02d}.{ext}  (DATE first so alphabetical == chronological; _num:02d for carrousel order 01,02,03)
#       ├── reels/    → {date:%Y-%m-%d}_{post_id}_{num:02d}.{ext}
#       ├── highlights/{post_id}{highlight_title:?_//}/ → {media_id}.{ext}  (NO date, NO num; identity via media_id, highlight via id+raw title)
#       ├── stories/2026/10-october/ → {post_id}_{num:02d}.{ext}  (ephemeral 24h; year/month lowercase via f-string formatter)
#       └── profile/ → {username}_{media_id}.{ext}  (avatar; {media_id}=profile_pic_id changes with avatar)
#   Tree root: {scrapmf_root}(PROFILE, e.g. example) / {category}(network) / {username}(account) / content
#   Actual paths:
#     posts/reels: ~/scrapmf/PERFIL/instagram/{username}/{posts,reels,...}
#     highlights:  ~/scrapmf/PERFIL/instagram/{username}/highlights/{post_id}{highlight_title:?_//}/{media_id}.{ext}
#     stories:     ~/scrapmf/PERFIL/instagram/{username}/stories/{year}/{month-lower}/{post_id}_{num}.{ext}
#   {scrapmf_root} injected at runtime via -o extractor.keywords.scrapmf_root=<profile> (fallback "default" without profile)
#   NOTE: data downloaded before this structure lives under the old layout — move it manually if desired.
#   Sub-URLs: https://www.instagram.com/{username}/ , /reels/ , /highlights/ , /stories/{username}/ , /{username}/avatar
#   Variables verified: post_id, highlight_id, media_id, highlight_title, date (exists), category(=site), subcategory(=posts...), username
#   Titles: highlight_title kept RAW via conditional separator
#     ({title:?_//} — two slashes required, else gallery-dl raises
#     DirectoryFormatError: expected 3, got 2). Emoji-only names stay
#     visible per user preference ("id_❤️‍🩹"); truly empty titles drop
#     the separator → folder is just {post_id}. No collisions: the id
#     prefix is unique. Slugify (!g) was tried and retired: it erased
#     emoji-only titles entirely.
#   Dedup: archive (gallery-dl --download-archive) is primary dedup; highlights use {media_id} only — no date fallback needed
#   Reels dedup: the profile-root pass ({username}/) reads IG's feed, which mixes
#     reels into posts. scrapmf injects extractor.instagram.posts.post-filter =
#     "type != 'reel'" so reels download exactly once — only into reels/ from
#     the dedicated /reels/ pass. Fail-open: if IG drops the `type` metadata,
#     worst case is a duplicate download, never lost content.
#
# All options (all optional unless noted):
#   site = "instagram"                     # logical name (usually filename)
#   pattern = "instagram.com"              # substring to auto-match URL
#   output_dir = "~/Pictures/instagram"    # per-site output (CLI --output wins; default HOME = ~)
#   cookies = "/path/cookies.txt"          # Netscape cookies.txt file (gallery-dl --cookies)
#   cookies_from_browser = "brave"         # brave | firefox | chrome | chromium | edge | opera | vivaldi[:profile] (gallery-dl --cookies-from-browser; DESKTOP ONLY — Android/Termux browsers are unreadable, use `cookies` files there)
#   archive = "/custom/path.sqlite"        # explicit archive override (advanced).
#     By default scrapmf keeps its own per-account download archive in
#     ~/.config/scrapmf/archive/<site>/<account>.jsonl — dedup is automatic;
#     disable with [general] archive = false or `scrapmf scrape --no-archive`.
#   extra_args = ["--restrict-filenames", "auto", "--proxy", "http://..."]  # allow-list: --proxy, --user-agent, --sleep, --sleep-request, --sleep-429, --limit-rate, --cookies, --cookies-from-browser, --restrict-filenames, --destination, --get-urls (--exec forbidden; ;|&$><` rejected)
#   filename_template = "{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}"  # → gallery-dl -o filename= (date first; _num:02d preserves carrousel order 01,02,03)
#   directory_template = ["{scrapmf_root}", "{category}", "{username}", "{subcategory}"]  # → PERFIL/instagram/CUENTA/posts
#   media_id = "..."                       # custom media id filter
#   include_highlights = true              # instagram highlights toggle (fetch all highlights)
#   highlights = ["id1", "id2"]            # list of highlight ids (empty = all)
#   [rate_limit]                           # rate limiting (keep proven: brave cookies, sleep 3-6, sleep-request 8-15, sleep-429 120)
#     sleep = "3-6"                        # --sleep (supports range)
#     sleep_request = "8-15"               # --sleep-request
#     sleep_429 = 120                      # --sleep-429 (seconds on 429)
#     limit_rate = "500k"                  # --limit-rate
#   [extractor]                            # gallery-dl -o key=value
#     instagram = "true"                   # string or array: key = ["a","b"]
#

"#;
    let content = format!("{header}{body}");
    write_config_file(&target, &content)
}

/// Ensure sites/tiktok.toml exists with 0o600 (no clobber).
///
/// Variables VERIFIED against gallery-dl 1.32.9 source (`extractor/tiktok.py`):
///   - TikTok uses `{user}` (NOT `{username}` like Instagram)
///   - posts/stories items: `{id}` (post id), `{date}` (createTime), `{num}`
///     (carousel index, absent on single videos), `{file_id}`, `{title}`
///   - avatar: `{id}` = numeric user id, `{user}`, `{type}="avatar"`, `{file_id}`;
///     NO intrinsic date → avatar filename uses {file_id} (per avatar version)
///   - highlights: NOT supported by gallery-dl 1.32.9 for TikTok — omitted
pub fn ensure_tiktok_site() -> anyhow::Result<()> {
    let Some(dir) = sites_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    crate::config::fs::restrict_perms(&dir, true);
    let target = dir.join("tiktok.toml");
    if target.exists() {
        migrate_tiktok_robustness(&target)?;
        return Ok(());
    }

    let mut extractor: std::collections::HashMap<String, toml::Value> =
        std::collections::HashMap::new();

    let mut posts_table = toml::map::Map::new();
    posts_table.insert(
        "directory".to_string(),
        tiktok_posts_conditional_directory(),
    );
    posts_table.insert(
        "filename".to_string(),
        toml::Value::String("{date:%Y-%m-%d}_{id}{num:?_//>02}.{extension}".to_string()),
    );
    // Music tracks extracted from posts are not wanted content
    posts_table.insert("audio".to_string(), toml::Value::Boolean(false));
    extractor.insert("tiktok:posts".to_string(), toml::Value::Table(posts_table));

    let mut stories_table = toml::map::Map::new();
    stories_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{user}".to_string()),
            toml::Value::String("stories".to_string()),
        ]),
    );
    stories_table.insert(
        "filename".to_string(),
        toml::Value::String("{date:%Y-%m-%d}_{id}{num:?_//>02}.{extension}".to_string()),
    );
    stories_table.insert("audio".to_string(), toml::Value::Boolean(false));
    extractor.insert(
        "tiktok:stories".to_string(),
        toml::Value::Table(stories_table),
    );

    let mut avatar_table = toml::map::Map::new();
    avatar_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{user}".to_string()),
            toml::Value::String("profile".to_string()),
        ]),
    );
    avatar_table.insert(
        "filename".to_string(),
        toml::Value::String("{user}_{file_id}.{extension}".to_string()),
    );
    extractor.insert(
        "tiktok:avatar".to_string(),
        toml::Value::Table(avatar_table),
    );

    let site = Site {
        site: Some("tiktok".to_string()),
        pattern: Some("tiktok.com".to_string()),
        patterns: vec!["tiktok.com".to_string()],
        output_dir: None,
        extra_args: vec![
            "--restrict-filenames".to_string(),
            "auto".to_string(),
            // Robustness for large MP4 downloads from an expiring CDN:
            // more per-file retries and a longer read timeout (defaults 4 / 30s
            // truncate videos on flaky connections).
            "--retries".to_string(),
            "10".to_string(),
            "--http-timeout".to_string(),
            "120".to_string(),
        ],
        cookies: None,
        // TikTok returns 403 (JS challenge) without a valid session —
        // browser cookies mitigate it; requires an open session.
        cookies_from_browser: Some("brave".to_string()),
        cookie_profile: None,
        // Same proven values as instagram — TikTok is aggressive with bot
        // detection; without throttling bulk downloads trigger challenges.
        rate_limit: Some(RateLimit {
            sleep: Some("3-6".to_string()),
            sleep_request: Some("8-15".to_string()),
            sleep_429: Some(120),
            limit_rate: None,
        }),
        archive: None,
        extractor,
        filename_template: None,
        directory_template: None,
    };
    let body = toml::to_string_pretty(&site).context("serialize tiktok site")?;
    let header = r#"# scrapmf — site config — tiktok
# File: ~/.config/scrapmf/sites/tiktok.toml (0o600, dir 0o700)
#
# TikTok strategy (verified via gallery-dl 1.32.9 --list-extractors + source):
#   PERFIL/  (scrapmf_root = profile name; output default ~/scrapmf)
#   └── tiktok/
#       └── CUENTA/  ({user} uniqueId)
#           ├── videos/    → {date}_{id}{num}.{ext}   (post_type != image — conditional directory; DATE first so alphabetical == chronological)
#           ├── photos/    → {date}_{id}_{num}.{ext}  (post_type == 'image')
#           ├── stories/   → {id}_{date}{num}.{ext}   (/@USER/stories, ephemeral 24h)
#           └── profile/   → {user}_{file_id}.{ext}    (/@USER/avatar; {file_id} changes per avatar version)
#   AUDIO: music tracks are NOT downloaded (audio=false on posts+stories) — soundtracks, not content.
#   NOTE: extension-based split does NOT work for tiktok videos (URLs lack extensions at
#     directory time) — post_type keyword is used instead.
#   HIGHLIGHTS: NOT SUPPORTED by gallery-dl 1.32.9 for tiktok (no extractor) — intentionally omitted.
#
# Variables verified (gallery-dl 1.32.9 source): {user} (=uniqueId, NOT {username}),
#   {id}, {date}, {num}, {title}, {file_id}. Avatar filename: {user}_{file_id}.
#
# Auth: tiktok returns 403 (JS challenge) without a valid session — open a tiktok.com
#   session in the configured browser before scraping, or set cookies = "/path/file".
# PRIVATE ACCOUNTS (stories included): /@USER/stories dies fetching the profile
#   page (statusCode 10222 — "Login required to access this profile, or this
#   profile has no videos posted") even when your session follows the account.
#   Workarounds that DO work (verified):
#     1. Scrape the story's direct link — copy it while viewing the story:
#          scrapmf scrape "https://www.tiktok.com/@USER/video/<id>"
#     2. Numeric-authorId form skips the broken profile fetch entirely
#        (find the id once via: gallery-dl -K --cookies-from-browser brave
#         "https://www.tiktok.com/@USER/video/<id>" → author['id']):
#          https://www.tiktok.com/@<authorId>/stories
#   Note: story content APIs may still serve empty lists to non-browser
#   sessions (upstream gallery-dl limitation, pinned v1.32.9).
# FALLBACK WARNING: -o ytdl=true routes extraction through yt-dlp, which CANNOT
#   download TikTok photo carousels (it saves only the background audio track).
#   Do NOT enable it here — you would silently lose every slideshow post.
# QUALITY: already maximal — gallery-dl always picks the highest-resolution
#   variant TikTok offers (sorted by width×height, no watermark). There is no
#   quality/codec option to raise. Note: top-resolution variants are
#   increasingly served as HEVC/H.265 — your player needs an HEVC decoder.
# Robustness: --retries 10 + --http-timeout 120 — TikTok MP4s are large files from an
#   expiring CDN; with gallery-dl defaults (4 retries / 30s timeout) flaky connections
#   truncate videos. If a file ever comes out incomplete: delete it and re-run (the
#   deterministic {date}_{id} filename re-downloads only what is missing).
#

"#;
    let content = format!("{header}{body}");
    write_config_file(&target, &content)
}

/// Ensure sites/twitter.toml exists with 0o600 (no clobber).
///
/// Variables VERIFIED against gallery-dl 1.32.9 source (`extractor/twitter.py`):
///   - Auth REQUIRED: cookie `auth_token` from a logged-in x.com session
///   - media items carry `{type}` = "photo" | "video" (conditional directory split)
///   - tweet keywords: `{tweet_id}`, `{date}` (upload datetime), `{num}`,
///     `{media_id}`, `{author[screen_name]}`, counts as keywords
///   - avatar: synthetic tweet per profile-pic version (`{tweet_id}` changes with it)
///   - info/statistics: metadata-only extractors — download 0 files; omitted
pub fn ensure_twitter_site() -> anyhow::Result<()> {
    let Some(dir) = sites_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    crate::config::fs::restrict_perms(&dir, true);
    let target = dir.join("twitter.toml");
    if target.exists() {
        return Ok(());
    }

    let mut extractor: std::collections::HashMap<String, toml::Value> =
        std::collections::HashMap::new();

    // twitter:media — static default directory (videos). The photos/videos
    // split does NOT use a conditional directory: {type} is only available on
    // file dicts AFTER the Directory message, so conditions never match.
    // scrapmf's interactive flows emit TWO passes with file-filter +
    // static directory overrides instead. This default catches CLI/manual use.
    let mut media_table = toml::map::Map::new();
    let seg = |s: &str| toml::Value::String(s.to_string());
    let dir4 = |last: &str| {
        toml::Value::Array(vec![
            seg("{scrapmf_root}"),
            seg("{category}"),
            seg("{user[name]}"),
            seg(last),
        ])
    };
    media_table.insert("directory".to_string(), dir4("videos"));
    media_table.insert(
        "filename".to_string(),
        toml::Value::String("{tweet_id}_{date:%Y-%m-%d}_{num:02d}.{extension}".to_string()),
    );
    extractor.insert("twitter:media".to_string(), toml::Value::Table(media_table));

    // twitter:avatar — profile pic; {tweet_id} changes per avatar version
    let mut avatar_table = toml::map::Map::new();
    avatar_table.insert("directory".to_string(), dir4("profile"));
    avatar_table.insert(
        "filename".to_string(),
        toml::Value::String("avatar_{tweet_id}.{extension}".to_string()),
    );
    extractor.insert(
        "twitter:avatar".to_string(),
        toml::Value::Table(avatar_table),
    );

    // twitter:background — header/banner into the same profile folder
    let mut background_table = toml::map::Map::new();
    background_table.insert("directory".to_string(), dir4("profile"));
    background_table.insert(
        "filename".to_string(),
        toml::Value::String("background_{tweet_id}.{extension}".to_string()),
    );
    extractor.insert(
        "twitter:background".to_string(),
        toml::Value::Table(background_table),
    );

    let site = Site {
        site: Some("twitter".to_string()),
        pattern: Some("x.com".to_string()),
        patterns: vec!["x.com".to_string(), "twitter.com".to_string()],
        output_dir: None,
        extra_args: vec!["--restrict-filenames".to_string(), "auto".to_string()],
        cookies: None,
        // X requires auth_token from a logged-in session — browser cookies mandatory
        cookies_from_browser: Some("brave".to_string()),
        cookie_profile: None,
        rate_limit: Some(RateLimit {
            sleep: Some("3-6".to_string()),
            sleep_request: Some("8-15".to_string()),
            sleep_429: Some(120),
            limit_rate: None,
        }),
        archive: None,
        extractor,
        filename_template: None,
        directory_template: None,
    };
    let body = toml::to_string_pretty(&site).context("serialize twitter site")?;
    let header = r#"# scrapmf — site config — twitter / X
# File: ~/.config/scrapmf/sites/twitter.toml (0o600, dir 0o700)
#
# Twitter/X strategy (verified via gallery-dl 1.32.9 --list-extractors + source):
#   PERFIL/  (scrapmf_root = profile name; output default ~/scrapmf)
#   └── twitter/
#       └── CUENTA/  ({user[name]})
#           ├── videos/    → {tweet_id}_{date}_{num:02d}.{ext}   (/@USER/media default)
#           ├── photos/    → {tweet_id}_{date}_{num:02d}.{ext}   (interactive mode: second pass with file-filter type=='photo')
#           └── profile/   → avatar_{tweet_id}.{ext}  (/@USER/photo; id changes per pic version)
#                            background_{tweet_id}.{ext}  (/@USER/header_photo — banner)
#   AUTH: X requires the auth_token cookie of a logged-in session — open
#     x.com in the configured browser before scraping.
#
#   INFO/STATISTICS: metadata-only extractors — download 0 files; omitted.
#   Counts (favorite_count, retweet_count, view_count) exist as keywords
#     usable in filenames/filters if ever needed.
#
"#;
    let content = format!("{header}{body}");
    write_config_file(&target, &content)
}

/// Ensure sites/vsco.toml exists with 0o600 (no clobber).
///
/// Variables VERIFIED against gallery-dl 1.32.9 source (`extractor/vsco.py`)
/// and live tests:
///   - NO auth required: API token is auto-extracted from the page's
///     __PRELOADED_STATE__ as an anonymous visitor
///   - gallery items: `{id}` (hex media id), `{date}` (REAL upload date),
///     `{user}`, `{video}` bool, width/height/description/tags
///   - avatar: `{id}` = profileImageId (changes per pic version)
///   - Single Directory upfront → per-FILE conditional directories don't
///     work; photos+videos share the gallery folder (single pass)
pub fn ensure_vsco_site() -> anyhow::Result<()> {
    let Some(dir) = sites_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    crate::config::fs::restrict_perms(&dir, true);
    let target = dir.join("vsco.toml");
    if target.exists() {
        return Ok(());
    }

    let mut extractor: std::collections::HashMap<String, toml::Value> =
        std::collections::HashMap::new();

    let seg = |s: &str| toml::Value::String(s.to_string());
    let dir4 = |last: &str| {
        toml::Value::Array(vec![
            seg("{scrapmf_root}"),
            seg("{category}"),
            seg("{user}"),
            seg(last),
        ])
    };

    // vsco:gallery — ALL media (photos + videos together, single pass)
    let mut gallery_table = toml::map::Map::new();
    gallery_table.insert("directory".to_string(), dir4("gallery"));
    gallery_table.insert(
        "filename".to_string(),
        toml::Value::String("{id}_{date:%Y-%m-%d}.{extension}".to_string()),
    );
    extractor.insert(
        "vsco:gallery".to_string(),
        toml::Value::Table(gallery_table),
    );

    // vsco:avatar — profile pic; {id} (= profileImageId) changes per version
    let mut avatar_table = toml::map::Map::new();
    avatar_table.insert("directory".to_string(), dir4("profile"));
    avatar_table.insert(
        "filename".to_string(),
        toml::Value::String("avatar_{id}.{extension}".to_string()),
    );
    extractor.insert("vsco:avatar".to_string(), toml::Value::Table(avatar_table));

    let site = Site {
        site: Some("vsco".to_string()),
        pattern: Some("vsco.co".to_string()),
        patterns: vec!["vsco.co".to_string()],
        output_dir: None,
        extra_args: vec!["--restrict-filenames".to_string(), "auto".to_string()],
        cookies: None,
        // NO auth needed — VSCO works anonymously (token from page state)
        cookie_profile: None,
        cookies_from_browser: None,
        rate_limit: Some(RateLimit {
            sleep: Some("3-6".to_string()),
            sleep_request: Some("8-15".to_string()),
            sleep_429: Some(120),
            limit_rate: None,
        }),
        archive: None,
        extractor,
        filename_template: None,
        directory_template: None,
    };
    let body = toml::to_string_pretty(&site).context("serialize vsco site")?;
    let header = r#"# scrapmf — site config — vsco
# File: ~/.config/scrapmf/sites/vsco.toml (0o600, dir 0o700)
#
# VSCO strategy (verified via gallery-dl 1.32.9 --list-extractors + source):
#   PERFIL/  (scrapmf_root = profile name; output default ~/scrapmf)
#   └── vsco/
#       └── CUENTA/  ({user} handle)
#           ├── gallery/   → {id}_{date:%Y-%m-%d}.{ext}  (/@USER/gallery — photos AND videos together, single pass)
#           └── profile/   → avatar_{id}.{ext}           (/@USER/avatar; {id}=profileImageId changes per pic version)
#
#   AUTH: NOT required — VSCO works anonymously (API token auto-extracted
#     from each page's __PRELOADED_STATE__). No cookies configured.
#
#   INFO: collection/spaces/image/video extractors exist but are out of scope v1.
#
"#;
    let content = format!("{header}{body}");
    write_config_file(&target, &content)
}

/// Ensure sites/threads.toml exists with 0o600 (no clobber).
/// Threads strategy — via `threadstractor` (gallery-dl has no Threads support).
/// Naming mirrors instagram: date first for chronological order, but configurable
/// via `filename_template` like gallery-dl (user can reorder to {post_id}_{date}...).
pub fn ensure_threads_site() -> anyhow::Result<()> {
    let Some(dir) = sites_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    crate::config::fs::restrict_perms(&dir, true);
    let target = dir.join("threads.toml");
    if target.exists() {
        return Ok(());
    }
    let site = Site {
        site: Some("threads".to_string()),
        pattern: Some("threads.com".to_string()),
        patterns: vec!["threads.com".to_string(), "threads.net".to_string()],
        output_dir: None,
        extra_args: vec!["--restrict-filenames".to_string(), "auto".to_string()],
        cookies: None,
        cookies_from_browser: Some("brave".to_string()),
        cookie_profile: None,
        rate_limit: Some(RateLimit {
            sleep: Some("3-6".to_string()),
            sleep_request: Some("8-15".to_string()),
            sleep_429: Some(120),
            limit_rate: None,
        }),
        archive: None,
        extractor: std::collections::HashMap::new(),
        // Default chronological: date first (like instagram) — user can override
        // to "{post_id}_{num:02d}.{extension}" or "{post_id}_{date:%Y-%m-%d}_{num:02d}..."
        // via sites/threads.toml: filename_template = "..."
        filename_template: Some("{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}".to_string()),
        directory_template: Some(vec![
            "{scrapmf_root}".to_string(),
            "{category}".to_string(),
            "{username}".to_string(),
            "{subcategory}".to_string(),
        ]),
    };
    let body = toml::to_string_pretty(&site).context("serialize threads site")?;
    let header = r#"# scrapmf — site config — threads (via threadstractor)
# File: ~/.config/scrapmf/sites/threads.toml (0o600, dir 0o700)
# Gallery-dl has no Threads support — scrapmf routes Threads URLs to the
# `threadstractor` provider (pip install threadstractor).
#
# Threads strategy (via threadstractor, verified 2025-08 with Brave cookies):
#   PERFIL/  (scrapmf_root = profile name; output default ~/scrapmf)
#   └── threads/
#       └── CUENTA/  ({username} handle)
#           ├── posts/    → {date:%Y-%m-%d}_{post_id}_{num:02d}.{ext}  (DATE first so alphabetical == chronological; _num:02d for carrousel 01,02,03)
#           └── profile/  → {username}_{media_id}.{ext}  (avatar; media_id changes per avatar version)
#   Variables (same as instagram for compat): {date:%Y-%m-%d}, {post_id}, {media_id} (=post_id_1), {num:02d}, {username}, {category}=threads, {subcategory}=posts/profile, {extension}
#   Chronological order: keep date first. To mimic gallery-dl reordering, edit filename_template:
#     filename_template = "{post_id}_{date:%Y-%m-%d}_{num:02d}.{extension}"  # post_id first
#     filename_template = "{post_id}_{num:02d}.{extension}"                  # no date, post_id only
#   Directory is also configurable: directory_template = ["{scrapmf_root}","{category}","{username}"]
#   Archive: deferred — threads archive JSONL not yet implemented (re-run skips via filename dedup for now).
#
"#;
    let content = format!("{header}{body}");
    write_config_file(&target, &content)
}

/// Ensure sites/facebook.toml exists with 0o600 (no clobber).
/// Facebook strategy — via gallery-dl (verified 1.32.9 via -K).
/// Separates publicaciones/álbumes/videos/perfil + historias/destacadas (featured)
/// like instagram:highlights (placeholders until upstream stories/highlights).
pub fn ensure_facebook_site() -> anyhow::Result<()> {
    let Some(dir) = sites_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    crate::config::fs::restrict_perms(&dir, true);
    let target = dir.join("facebook.toml");
    if target.exists() {
        // Migrate outdated facebook.toml (missing facebook:user/set/photo or with highlights/stories)
        if let Ok(existing) = std::fs::read_to_string(&target) {
            let has_user = existing.contains("facebook:user");
            let has_set = existing.contains("facebook:set");
            let has_photo = existing.contains("facebook:photo")
                && existing.contains("include")
                && existing.contains("\"photo\"");
            let has_avatar =
                existing.contains("facebook:avatar") && existing.contains("{username}_{id}");
            let has_highlights = existing.contains("facebook:highlights");
            let has_stories = existing.contains("facebook:stories");
            if has_user && has_set && has_photo && has_avatar && !has_highlights && !has_stories {
                return Ok(());
            }
            // Outdated — will be overwritten below with correct 4-table site
            let _ = std::fs::remove_file(&target);
        } else {
            return Ok(());
        }
    }
    let mut extractor: std::collections::HashMap<String, toml::Value> =
        std::collections::HashMap::new();

    // facebook:user — publicaciones (user dispatch, same as photos feed)
    let mut user_table = toml::map::Map::new();
    user_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("posts".to_string()),
        ]),
    );
    user_table.insert(
        "filename".to_string(),
        toml::Value::String("{date:%Y-%m-%d}_{id}_{num:02d}.{extension}".to_string()),
    );
    extractor.insert("facebook:user".to_string(), toml::Value::Table(user_table));

    // facebook:photos — publicaciones (feed)
    let mut photos_table = toml::map::Map::new();
    photos_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("photos".to_string())]),
    );
    photos_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("photos".to_string()),
        ]),
    );
    photos_table.insert(
        "filename".to_string(),
        toml::Value::String("{date:%Y-%m-%d}_{id}_{num:02d}.{extension}".to_string()),
    );
    extractor.insert(
        "facebook:photos".to_string(),
        toml::Value::Table(photos_table),
    );

    // facebook:albums — álbumes (set_id + title)
    let mut albums_table = toml::map::Map::new();
    albums_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("albums".to_string())]),
    );
    albums_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("albums".to_string()),
            toml::Value::String("{title[:220]}{set_id:? (/)/}".to_string()),
        ]),
    );
    albums_table.insert(
        "filename".to_string(),
        toml::Value::String("{id}.{extension}".to_string()),
    );
    extractor.insert(
        "facebook:albums".to_string(),
        toml::Value::Table(albums_table),
    );

    // facebook:video — videos
    let mut video_table = toml::map::Map::new();
    video_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("videos".to_string()),
        ]),
    );
    video_table.insert(
        "filename".to_string(),
        toml::Value::String("{date:%Y-%m-%d}_{id}.{extension}".to_string()),
    );
    extractor.insert(
        "facebook:video".to_string(),
        toml::Value::Table(video_table),
    );

    // facebook:avatar — perfil (con username, sin set_id)
    let mut avatar_table = toml::map::Map::new();
    avatar_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("avatar".to_string())]),
    );
    avatar_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("profile".to_string()),
        ]),
    );
    avatar_table.insert(
        "filename".to_string(),
        toml::Value::String("{username}_{id}.{extension}".to_string()),
    );
    extractor.insert(
        "facebook:avatar".to_string(),
        toml::Value::Table(avatar_table),
    );

    // facebook:set — single set (e.g., Fotos del perfil, Fotos de portada) - ensure quick without facebook/
    let mut set_table = toml::map::Map::new();
    set_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("albums".to_string()),
            toml::Value::String("{title[:220]}{set_id:? (/)/}".to_string()),
        ]),
    );
    set_table.insert(
        "filename".to_string(),
        toml::Value::String("{id}.{extension}".to_string()),
    );
    extractor.insert("facebook:set".to_string(), toml::Value::Table(set_table));

    // facebook:photo — single photo (photo/?fbid=ID without set_id)
    let mut photo_table = toml::map::Map::new();
    photo_table.insert(
        "include".to_string(),
        toml::Value::Array(vec![toml::Value::String("photo".to_string())]),
    );
    photo_table.insert(
        "directory".to_string(),
        toml::Value::Array(vec![
            toml::Value::String("{scrapmf_root}".to_string()),
            toml::Value::String("{category}".to_string()),
            toml::Value::String("{username}".to_string()),
            toml::Value::String("photos".to_string()),
        ]),
    );
    photo_table.insert(
        "filename".to_string(),
        toml::Value::String("{id}.{extension}".to_string()),
    );
    extractor.insert(
        "facebook:photo".to_string(),
        toml::Value::Table(photo_table),
    );

    let site = Site {
        site: Some("facebook".to_string()),
        pattern: Some("facebook.com".to_string()),
        patterns: vec![
            "facebook.com".to_string(),
            "fb.com".to_string(),
            "m.facebook.com".to_string(),
        ],
        output_dir: None,
        extra_args: vec!["--restrict-filenames".to_string(), "auto".to_string()],
        cookies: None,
        cookies_from_browser: Some("brave".to_string()),
        cookie_profile: None,
        rate_limit: Some(RateLimit {
            sleep: Some("3-6".to_string()),
            sleep_request: Some("8-15".to_string()),
            sleep_429: Some(120),
            limit_rate: None,
        }),
        archive: None,
        extractor,
        filename_template: None,
        directory_template: None,
    };
    let body = toml::to_string_pretty(&site).context("serialize facebook site")?;
    let header = r#"# scrapmf — site config — facebook
# File: ~/.config/scrapmf/sites/facebook.toml (0o600, dir 0o700)
# Gallery-dl 1.32.9: facebook:user dispatches photos/albums/avatar/info; set/video handle albums/videos.
# Auth: siempre requiere sesión (c_user/xs) — brave cookies o --cookies file (no anon como vsco).
# Árbol: PERFIL/facebook/CUENTA/{photos,albums/<title (set)/>,videos,profile}
#   Publicaciones → photos/ {date}_{id}_{num}.{ext}
#   Álbumes → albums/{title (set)}/{id}.{ext}
#   Videos → videos/ {date}_{id}.{ext}
#   Perfil → profile/ {username}_{id}.{ext}
"#;
    let content = format!("{header}{body}");
    write_config_file(&target, &content)
}

/// Serialize a profile to `path` with doc header and 0o600 perms.
pub fn write_profile_file(path: &Path, profile: &Profile) -> anyhow::Result<()> {
    let body = toml::to_string_pretty(profile).context("serialize profile")?;
    let header = format!(
        "# scrapmf profile — {}\n# See profiles/example_person.toml for all options\n\n",
        profile.profile.as_deref().unwrap_or("profile")
    );
    let content = format!("{header}{body}\n");
    write_config_file(path, &content)
}
