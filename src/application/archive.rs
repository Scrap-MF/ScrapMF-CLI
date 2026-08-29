//! Download archive — scrapmf's own dedup record, independent from
//! gallery-dl's format.
//!
//! Canonical store (ours): `~/.config/scrapmf/archive/<site>/<account>.jsonl`,
//! append-only, one JSON object per line:
//!
//! ```json
//! {"k":"instagram 3182345678","t":1756000000}
//! ```
//!
//! `k` is the exact key gallery-dl writes into its own archive table
//! (`<category> <id>`), so the record can always be re-fed to gallery-dl.
//! `t` is a unix timestamp of first download. Extra fields may be added later
//! without breaking readers (unknown fields are ignored on load).
//!
//! Working cache (disposable): `$XDG_DATA_HOME/scrapmf/cache/archive/<site>/<account>.sqlite`
//! with gallery-dl's schema (`CREATE TABLE archive (entry TEXT PRIMARY KEY)`,
//! column renamed from `file` to `entry` in newer gallery-dl versions; see
//! `gallery_dl/archive.py` of v1.32.9).
//! Before each scrape we seed it with our known keys; after each scrape we
//! drain rows gallery-dl inserted and append them to the JSONL. The cache is
//! regenerable garbage — deleting it only costs one re-seed.

use std::collections::HashSet;
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Canonical JSONL entry. Unknown extra fields in stored lines are ignored.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    /// gallery-dl archive key: `"<category> <id>"`.
    pub k: String,
    /// Unix timestamp (seconds) of first download.
    pub t: i64,
}

// ─── Paths ──────────────────────────────────────────────────────────────────

fn sanitize_component(name: &str) -> String {
    crate::util::sanitize_component(name, 64, "misc")
}

/// Canonical archive file: `<config>/archive/<site>/<account>.jsonl`.
pub fn entries_path(site: &str, account: &str) -> Option<PathBuf> {
    crate::config::config_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|base| {
            base.join("archive")
                .join(sanitize_component(site))
                .join(format!("{}.jsonl", sanitize_component(account)))
        })
}

/// Disposable sqlite cache: `<data>/cache/archive/<site>/<account>.sqlite`.
pub fn cache_path(site: &str, account: &str) -> Option<PathBuf> {
    dirs::data_dir().map(|p| {
        p.join("scrapmf/cache/archive")
            .join(sanitize_component(site))
            .join(format!("{}.sqlite", sanitize_component(account)))
    })
}

// ─── URL → (site, account) ─────────────────────────────────────────────────

/// Derive `(site, account)` for supported sites; `None` when the URL does not
/// belong to a known site shape (v1: unknown URLs are not archived).
pub fn site_account_from_url(url: &str) -> Option<(String, String)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let (host, full_path) = rest.split_once('/')?;
    let path = full_path.split(['?', '#']).next().unwrap_or(full_path);
    let seg = |i: usize| path.split('/').nth(i).filter(|s| !s.is_empty());
    match host {
        "instagram.com" => seg(0).map(|u| ("instagram".into(), u.trim_end_matches('/').into())),
        "tiktok.com" => seg(0)
            .and_then(|u| u.strip_prefix('@'))
            .map(|u| ("tiktok".into(), u.into())),
        "twitter.com" | "x.com" => seg(0)
            .filter(|u| !matches!(*u, "i" | "home" | "explore" | "search"))
            .map(|u| ("twitter".into(), u.into())),
        "vsco.co" => seg(0).map(|u| ("vsco".into(), u.into())),
        "threads.com" | "threads.net" => seg(0)
            .and_then(|u| u.strip_prefix('@'))
            .map(|u| ("threads".into(), u.into())),
        h if h == "facebook.com"
            || h == "fb.com"
            || h == "m.facebook.com"
            || h.ends_with(".facebook.com") =>
        {
            // Use full_path (with query) for profile.php?id= and people/Name/ID
            if let Some(pos) = full_path.find("profile.php?id=") {
                let after = &full_path[pos + "profile.php?id=".len()..];
                let id = after.split(['&', '/', '?', '#']).next().unwrap_or(after);
                if !id.is_empty() {
                    return Some(("facebook".into(), id.to_string()));
                }
            }
            if full_path.starts_with("people/") {
                let parts: Vec<&str> = full_path.split('/').collect();
                // people/Name/ID or people/ID
                if parts.len() >= 3 {
                    let id = parts[2].split(['?', '#', '&']).next().unwrap_or(parts[2]);
                    if !id.is_empty() {
                        return Some(("facebook".into(), id.to_string()));
                    }
                }
                if parts.len() >= 2 {
                    let id = parts[1].split(['?', '#', '&']).next().unwrap_or(parts[1]);
                    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
                        return Some(("facebook".into(), id.to_string()));
                    }
                }
            }
            seg(0).map(|u| ("facebook".into(), u.to_string()))
        }
        _ => None,
    }
}

// ─── JSONL canonical store ─────────────────────────────────────────────────

fn parse_line(line: &str) -> Option<Entry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let k = v.get("k")?.as_str()?.to_string();
    let t = v.get("t").and_then(serde_json::Value::as_i64).unwrap_or(0);
    Some(Entry { k, t })
}

/// Load all keys recorded for this scope. Missing file → empty set.
pub fn load_keys(path: &Path) -> std::io::Result<HashSet<String>> {
    let Ok(f) = std::fs::File::open(path) else {
        return Ok(HashSet::new());
    };
    let mut out = HashSet::new();
    for line in std::io::BufReader::new(f).lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(e) = parse_line(line) {
            out.insert(e.k);
        }
    }
    Ok(out)
}

/// Append entries (deduplicated against `existing`) as JSONL lines.
/// Creates parent dirs and the file with 0600 (it lists user content).
pub fn append_entries(
    path: &Path,
    existing: &HashSet<String>,
    keys: impl IntoIterator<Item = String>,
) -> std::io::Result<usize> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let fresh: Vec<String> = keys.into_iter().filter(|k| !existing.contains(k)).collect();
    if fresh.is_empty() {
        return Ok(0);
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    let mut w = BufWriter::new(&f);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut n = 0;
    for k in &fresh {
        serde_json::to_writer(
            &mut w,
            &Entry {
                k: k.clone(),
                t: now,
            },
        )?;
        w.write_all(b"\n")?;
        n += 1;
    }
    w.flush()?;
    Ok(n)
}

// ─── Sqlite working cache (gallery-dl compatible) ──────────────────────────

/// Seed the cache sqlite with `keys` using gallery-dl's exact schema (v1.32.9):
/// `CREATE TABLE IF NOT EXISTS archive (entry TEXT PRIMARY KEY) WITHOUT ROWID`.
/// The column is `entry` — older gallery-dl used `file`, but v1.32.9 queries
/// `WHERE entry=?` and fails with "no such column: entry" against the old
/// schema. Creates/overwrites the DB at `path` and its parent dirs.
pub fn seed_cache(path: &Path, keys: &HashSet<String>) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Start fresh: the JSONL is the source of truth, the cache is disposable.
    let _ = std::fs::remove_file(path);
    let conn = rusqlite::Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=OFF")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS \"archive\" (entry TEXT PRIMARY KEY) WITHOUT ROWID",
        [],
    )?;
    conn.execute_batch("BEGIN")?;
    {
        let mut stmt = conn.prepare("INSERT OR IGNORE INTO \"archive\" (entry) VALUES (?1)")?;
        for k in keys {
            stmt.execute([k])?;
        }
    }
    conn.execute_batch("COMMIT")?;
    Ok(())
}

/// Read every key currently in the cache's archive table.
pub fn drain_cache(path: &Path) -> anyhow::Result<HashSet<String>> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut out = HashSet::new();
    let mut stmt = conn.prepare("SELECT entry FROM \"archive\"")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for r in rows {
        out.insert(r?);
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tmpfile(name: &str) -> PathBuf {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(name);
        std::mem::forget(d); // keep alive for the test duration
        p
    }

    #[test]
    fn sanitize_components() {
        assert_eq!(sanitize_component("instagram"), "instagram");
        assert_eq!(sanitize_component("../etc/passwd"), "___etc_passwd");
        assert_eq!(sanitize_component(""), "misc");
        assert_eq!(sanitize_component(&"a".repeat(100)).len(), 64);
    }

    #[test]
    fn site_account_parsing() {
        assert_eq!(
            site_account_from_url("https://www.instagram.com/foo.bar/?hl=es"),
            Some(("instagram".into(), "foo.bar".into()))
        );
        assert_eq!(
            site_account_from_url("https://www.tiktok.com/@some.user/video/123"),
            Some(("tiktok".into(), "some.user".into()))
        );
        assert_eq!(
            site_account_from_url("https://x.com/person/media"),
            Some(("twitter".into(), "person".into()))
        );
        assert_eq!(
            site_account_from_url("https://vsco.co/artist/gallery"),
            Some(("vsco".into(), "artist".into()))
        );
        // structural paths are not accounts
        assert_eq!(site_account_from_url("https://x.com/i/status/1"), None);
        assert_eq!(site_account_from_url("https://example.com/a"), None);
    }

    #[test]
    fn jsonl_roundtrip_and_dedup() {
        let path = tmpfile("acc.jsonl");
        let mut known = load_keys(&path).unwrap();
        assert!(known.is_empty());

        let n = append_entries(
            &path,
            &known,
            vec!["instagram 1".into(), "instagram 2".into()],
        )
        .unwrap();
        assert_eq!(n, 2);
        known.extend(["instagram 1".into(), "instagram 2".into()]);

        // duplicate appends are skipped
        let n = append_entries(
            &path,
            &known,
            vec!["instagram 2".into(), "instagram 3".into()],
        )
        .unwrap();
        assert_eq!(n, 1);

        let keys = load_keys(&path).unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains("instagram 3"));
    }

    #[test]
    fn jsonl_tolerates_malformed_lines() {
        let path = tmpfile("bad.jsonl");
        std::fs::write(
            &path,
            "{\"k\":\"a 1\",\"t\":5}\nnot json\n{\"k\":\"b 2\"}\n\n",
        )
        .unwrap();
        let keys = load_keys(&path).unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn sqlite_seed_drain_roundtrip() {
        let path = tmpfile("cache.sqlite");
        let mut keys = HashSet::new();
        keys.insert("tiktok 111".to_string());
        keys.insert("tiktok 222".to_string());
        seed_cache(&path, &keys).unwrap();

        // gallery-dl adds one more during the "run" (v1.32.9 schema: entry column)
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("INSERT INTO \"archive\" (entry) VALUES ('tiktok 333')", [])
            .unwrap();
        drop(conn);

        let all = drain_cache(&path).unwrap();
        assert_eq!(all.len(), 3);
        let new: Vec<String> = all.difference(&keys).cloned().collect();
        assert_eq!(new, vec!["tiktok 333".to_string()]);
    }

    #[test]
    fn drain_missing_cache_is_error_free_empty_via_seed_first() {
        // seeding creates the db even with zero keys
        let path = tmpfile("empty.sqlite");
        seed_cache(&path, &HashSet::new()).unwrap();
        let all = drain_cache(&path).unwrap();
        assert!(all.is_empty());
    }
}
