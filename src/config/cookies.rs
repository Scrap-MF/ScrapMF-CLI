//! Cookie profiles: named, shareable Netscape-format credential files.
//!
//! Storage lives at `~/.config/scrapmf/cookies/<name>.txt` in the classic
//! Mozilla/Netscape format that gallery-dl consumes via `--cookies`. Files are
//! credentials (account access!) — always written 0600 and never logged.
//!
//! Chromium-family browsers encrypt cookie values (v10 legacy / v11 current):
//! the decryption key comes from the desktop keyring (schema v2 stores it
//! base64-encoded; v1/legacy use "peanuts") and is derived with PBKDF2-SHA1.
//! `capture_chromium` tries every candidate key against the first encrypted
//! cookie and keeps whichever validates.

use std::path::{Path, PathBuf};

// ─── Site domains ───────────────────────────────────────────────────────────

/// Domains (as they appear in cookie files / moz_cookies.host_key) per site key.
pub fn domains_for_site(site_key: &str) -> &'static [&'static str] {
    match site_key {
        "instagram" => &["instagram.com"],
        "tiktok" => &["tiktok.com"],
        "twitter" | "x" => &["twitter.com", "x.com"],
        "vsco" => &["vsco.co"],
        "threads" => &["threads.com", "threads.net"],
        _ => &[],
    }
}

// ─── Storage ────────────────────────────────────────────────────────────────

/// Directory holding every cookie profile.
pub fn cookies_dir() -> Option<PathBuf> {
    crate::config::config_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|base| base.join("cookies"))
}

fn sanitize_name(name: &str) -> String {
    crate::util::sanitize_component(name.trim(), 48, "unnamed")
}

pub fn profile_path(name: &str) -> Option<PathBuf> {
    cookies_dir().map(|dir| dir.join(format!("{}.txt", sanitize_name(name))))
}

pub fn list_profiles() -> Vec<String> {
    let Some(dir) = cookies_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "txt"))
        .filter_map(|e| {
            e.path()
                .file_stem()
                .and_then(|s| s.to_str().map(String::from))
        })
        .collect();
    names.sort();
    names
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ─── Netscape format ────────────────────────────────────────────────────────

// ─── Source metadata (embedded as a comment line) ───────────────────────────

const SOURCE_PREFIX: &str = "# scrapmf-source: ";

#[derive(Debug, Clone, PartialEq)]
pub struct SourceMeta {
    pub browser: String,
    pub networks: Vec<String>,
}

/// Read embedded origin metadata ("# scrapmf-source: browser=X networks=a,b").
/// None when the profile was created by manual paste/import.
pub fn parse_source_metadata(content: &str) -> Option<SourceMeta> {
    let line = content.lines().find(|l| l.starts_with(SOURCE_PREFIX))?;
    let mut browser = None;
    let mut networks = Vec::new();
    for part in line.strip_prefix(SOURCE_PREFIX)?.split_whitespace() {
        if let Some(v) = part.strip_prefix("browser=") {
            browser = Some(v.to_string());
        } else if let Some(v) = part.strip_prefix("networks=") {
            networks = v.split(',').map(String::from).collect();
        }
    }
    Some(SourceMeta {
        browser: browser?,
        networks,
    })
}

pub fn source_metadata_line(browser: &str, networks: &[String]) -> String {
    format!(
        "{SOURCE_PREFIX}browser={browser} networks={}\n",
        networks.join(",")
    )
}

/// One cookie row in Netscape format.
#[derive(Clone, Debug)]
pub struct StoredCookie {
    /// Raw Chromium-encrypted blob — used only during capture, never serialized.
    pub encrypted_value: Vec<u8>,
    pub domain: String,
    pub include_subdomains: bool,
    pub path: String,
    pub secure: bool,
    /// Unix seconds; `0` = session cookie.
    pub expires: i64,
    pub name: String,
    pub value: String,
    pub http_only: bool,
}

const NETSCAPE_HEADER: &str =
    "# Netscape HTTP Cookie File\n# This is a generated file! Do not edit.\n\n";

fn cookie_to_netscape_line(c: &StoredCookie) -> String {
    format!(
        "{}{}\t{}\t{}\t{}\t{}\t{}\t{}",
        if c.http_only { "#HttpOnly_" } else { "" },
        c.domain,
        if c.include_subdomains {
            "TRUE"
        } else {
            "FALSE"
        },
        c.path,
        if c.secure { "TRUE" } else { "FALSE" },
        c.expires,
        c.name,
        c.value
    )
}

/// Parse Netscape content into stored cookies. Skips comments, blank lines
/// and malformed rows; errors only when no valid cookie line exists at all.
/// The `#HttpOnly_` prefix convention is understood (http_only = true).
pub fn parse_netscape(content: &str) -> Result<Vec<StoredCookie>, String> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line.starts_with("# ") {
            continue;
        }
        let (http_only, line) = match line.strip_prefix("#HttpOnly_") {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 7 || f[5].is_empty() {
            continue;
        }
        let Ok(expires) = f[4].trim().parse::<i64>() else {
            continue;
        };
        out.push(StoredCookie {
            encrypted_value: Vec::new(),
            domain: f[0].to_string(),
            include_subdomains: f[1] == "TRUE",
            path: f[2].to_string(),
            secure: f[3] == "TRUE",
            expires,
            name: f[5].to_string(),
            value: f[6].to_string(),
            http_only,
        });
    }
    if out.is_empty() {
        return Err("no valid cookie lines found — expected Netscape format \
                    (7 tab-separated fields per line, header '# Netscape HTTP Cookie File')"
            .to_string());
    }
    Ok(out)
}

/// Serialize cookies into gallery-dl-ready Netscape content.
pub fn to_netscape(cookies: &[StoredCookie]) -> String {
    let mut out = String::from(NETSCAPE_HEADER);
    for c in cookies {
        out.push_str(&cookie_to_netscape_line(c));
        out.push('\n');
    }
    out
}

// ─── Profile CRUD ───────────────────────────────────────────────────────────

/// Load + validate a profile file. Returns the parsed cookies.
pub fn load_profile(name: &str) -> Result<Vec<StoredCookie>, String> {
    let path = profile_path(name).ok_or("cannot resolve cookies dir")?;
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read cookie profile '{}': {e}", path.display()))?;
    parse_netscape(&content)
}

/// Save a profile atomically with 0600 permissions. Content is validated first.
pub fn save_profile(name: &str, netscape_content: &str) -> Result<PathBuf, String> {
    parse_netscape(netscape_content)?;
    let dir = cookies_dir().ok_or("cannot resolve cookies dir")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create cookies dir: {e}"))?;
    crate::config::restrict_perms(&dir, true);
    let path = profile_path(name).ok_or("invalid profile name")?;
    std::fs::write(&path, netscape_content).map_err(|e| format!("write profile: {e}"))?;
    crate::config::restrict_perms(&path, false);
    Ok(path)
}

pub fn delete_profile(name: &str) -> Result<bool, String> {
    let path = profile_path(name).ok_or("invalid profile name")?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("delete profile: {e}"))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Human summary of a profile: total/expired cookies and domains covered.
/// Domains beyond 2 are collapsed to keep the line within the menu width.
pub fn profile_summary(name: &str) -> Result<String, String> {
    let cookies = load_profile(name)?;
    let now = now_secs();
    let total = cookies.len();
    let expired = cookies
        .iter()
        .filter(|c| c.expires != 0 && c.expires <= now)
        .count();
    let mut domains: Vec<&str> = cookies.iter().map(|c| c.domain.as_str()).collect();
    domains.sort_unstable();
    domains.dedup();
    let domains_str = if domains.len() <= 2 {
        domains.join(", ")
    } else {
        format!("{}, +{} more", domains[..2].join(", "), domains.len() - 2)
    };
    Ok(format!(
        "{total} cookie(s), {expired} expired — domains: {domains_str}"
    ))
}

// ─── Firefox capture ────────────────────────────────────────────────────────

fn newest_firefox_cookie_db() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let root = home.join(".mozilla").join("firefox");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&root).ok()?.flatten() {
        let db = entry.path().join("cookies.sqlite");
        if !db.is_file() {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&db) {
            let mtime = meta.modified().ok();
            match (mtime, &best) {
                (Some(t), Some((bt, _))) if t <= *bt => {}
                (Some(t), _) => best = Some((t, db)),
                _ => {}
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Extract cookies for `domains` from the most recent Firefox cookies.sqlite
/// (unencrypted SQLite, read from a copy so browser locks never matter).
///
/// Uses external readers instead of linking SQLite C code — keeps every
/// release target cross-compilable with zero C in the tree:
///   1. the `sqlite3` CLI, if present;
///   2. python3's stdlib `sqlite3` module otherwise.
pub fn capture_firefox(sites: &[String], profile_name: &str) -> Result<(PathBuf, usize), String> {
    if sites.is_empty() {
        return Err("no networks selected".to_string());
    }
    let domains: Vec<&str> = sites
        .iter()
        .flat_map(|s| domains_for_site(s))
        .copied()
        .collect();
    let db = newest_firefox_cookie_db()
        .ok_or("no Firefox cookies.sqlite found (~/.mozilla/firefox/*/cookies.sqlite)")?;

    let tmp = std::env::temp_dir().join(format!("scrapmf-ff-{}.sqlite", std::process::id()));
    std::fs::copy(&db, &tmp).map_err(|e| format!("copy cookies db: {e}"))?;

    let result = (|| {
        let rows = sqlite_read_rows(
            &tmp,
            "host,name,value,path,COALESCE(expiry,0),isSecure,isHttpOnly",
            "moz_cookies",
        )?;
        let mut cookies: Vec<StoredCookie> = Vec::new();
        for row in rows {
            if row.len() != 7 || row[1].is_empty() {
                continue;
            }
            let Ok(expires) = row[4].parse::<i64>() else {
                continue;
            };
            let cookie = StoredCookie {
                encrypted_value: Vec::new(),
                domain: row[0].clone(),
                include_subdomains: true,
                path: row[3].clone(),
                secure: row[5] == "1",
                expires,
                name: row[1].clone(),
                value: row[2].clone(),
                http_only: row[6] == "1",
            };
            if domains
                .iter()
                .any(|d| cookie.domain == *d || cookie.domain.ends_with(&format!(".{d}")))
            {
                cookies.push(cookie);
            }
        }
        if cookies.is_empty() {
            return Err(format!(
                "no cookies found for {} in this Firefox session — open the \
                 site while logged in, then retry",
                domains.join(", ")
            ));
        }
        let count = cookies.len();
        let content = to_netscape(&cookies);
        let content = format!("{}{content}", source_metadata_line("firefox", sites));
        let path = save_profile(profile_name, &content)?;
        Ok((path, count))
    })();

    let _ = std::fs::remove_file(&tmp);
    result
}

// ─── Refresh ────────────────────────────────────────────────────────────────

/// Result of refreshing an existing profile.
pub enum Refresh {
    /// Re-captured automatically from the stored origin.
    Done { path: PathBuf, count: usize },
    /// No source metadata (manual import) — user must paste a fresh export
    /// via $EDITOR.
    ManualImportRequired,
}

/// Re-capture an existing profile using its embedded origin metadata.
/// Keeps the SAME name, so accounts referencing it keep working untouched.
pub fn refresh_profile(name: &str) -> Result<Refresh, String> {
    let path = profile_path(name).ok_or("invalid profile name")?;
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("read profile '{name}': {e}"))?;

    match parse_source_metadata(&content) {
        Some(meta) if !meta.networks.is_empty() => {
            let res = if meta.browser.eq_ignore_ascii_case("firefox") {
                capture_firefox(&meta.networks, name)
            } else {
                capture_chromium(&meta.browser, &meta.networks, name)
            };
            res.map(|(path, count)| Refresh::Done { path, count })
        }
        _ => Ok(Refresh::ManualImportRequired),
    }
}

/// Shared external SQLite reader: runs `SELECT {columns} FROM {table}` and
/// returns TSV rows. Engine chain: sqlite3 CLI → python3 stdlib. Keeps the
/// tree C-free so every release target stays cross-compilable.
fn sqlite_read_rows(
    db_copy: &Path,
    columns_expr: &str,
    table: &str,
) -> Result<Vec<Vec<String>>, String> {
    let sql = format!("SELECT {columns_expr} FROM {table};");

    // 1) sqlite3 CLI
    if let Ok(out) = std::process::Command::new("sqlite3")
        .arg("-noheader")
        .arg("-nocolumn")
        .arg("-separator")
        .arg("\t")
        .arg(db_copy)
        .arg(&sql)
        .output()
        && out.status.success()
        && !out.stdout.is_empty()
    {
        return Ok(tsv_to_rows(&String::from_utf8_lossy(&out.stdout)));
    }

    // 2) python3 stdlib (sqlite3 ships with every distro's python3)
    let script = "import sqlite3,sys\n\
c = sqlite3.connect(sys.argv[1])\n\
[print(*r, sep='\t') for r in c.execute(sys.argv[2])]";
    if let Ok(out) = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(db_copy)
        .arg(&sql)
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if !text.trim().is_empty() {
                return Ok(tsv_to_rows(&text));
            }
        }
        return Err(format!(
            "python3 reader failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    Err(
        "neither 'sqlite3' nor 'python3' is available to read the cookies \
         database — install either one and retry"
            .to_string(),
    )
}

fn tsv_to_rows(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.split('\t').map(String::from).collect())
        .collect()
}

// ─── Chromium-family capture (Brave/Chrome/Chromium/Edge/Vivaldi/Opera) ─────

struct ChromiumPaths {
    cookies_db: PathBuf,
    /// Keyring service name (also used as the keyring item's user attribute).
    service: String,
}

fn chromium_paths(browser: &str) -> Option<ChromiumPaths> {
    let (base, service) = match browser.to_lowercase().as_str() {
        "brave" => ("BraveSoftware/Brave-Browser", "brave"),
        "chrome" | "google-chrome" => ("google-chrome", "chrome"),
        "chromium" => ("chromium", "chromium"),
        "edge" | "microsoft-edge" => ("microsoft-edge", "microsoft-edge"),
        "vivaldi" => ("vivaldi", "vivaldi-stable"),
        "opera" => ("opera", "opera"),
        _ => return None,
    };
    let home = dirs::home_dir()?;
    Some(ChromiumPaths {
        cookies_db: home
            .join(".config")
            .join(base)
            .join("Default")
            .join("Cookies"),
        service: service.to_string(),
    })
}

/// Candidate passwords for the browser's cookie key, most likely first:
/// Secret Service entry (attribute application=<browser>) — schema v2 wraps
/// it BASE64-ENCODED, so both decoded and raw forms are tried — then the
/// legacy hard-coded "peanuts".
fn chromium_candidate_passwords(service: &str) -> Vec<String> {
    use base64::Engine;
    let mut out = Vec::new();
    if let Ok(output) = std::process::Command::new("secret-tool")
        .args(["lookup", "application", service])
        .output()
        && output.status.success()
    {
        let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !secret.is_empty() {
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(secret.as_bytes())
                && let Ok(txt) = String::from_utf8(decoded)
            {
                out.push(txt);
            }
            out.push(secret);
        }
    }
    out.push("peanuts".to_string());
    out
}

/// Derive candidate AES-128 keys: PBKDF2-SHA1(password, salt "saltysalt",
/// 1 iteration, 16 bytes) — one key per distinct password candidate.
fn chromium_candidate_keys(service: &str) -> Vec<[u8; 16]> {
    let mut keys = Vec::new();
    for pw in chromium_candidate_passwords(service) {
        use pbkdf2::pbkdf2_hmac;
        use sha1::Sha1;
        let mut key = [0u8; 16];
        pbkdf2_hmac::<Sha1>(pw.as_bytes(), b"saltysalt", 1, &mut key);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Decrypt a v10/v11 blob: AES-128-CBC with IV of 16 spaces; v11 strips a
/// leading 32-byte SHA256(domain-without-leading-dot) hash after decryption.
fn decrypt_chromium_blob(blob: &[u8], key: &[u8; 16]) -> Option<String> {
    use aes::Aes128;
    use aes::cipher::{Array, BlockCipherDecrypt, KeyInit};

    if blob.len() < 19 {
        // "vNN" prefix + at least one 16-byte block
        return None;
    }
    let (version, rest) = blob.split_at(3);
    if version != b"v10" && version != b"v11" || rest.len() % 16 != 0 {
        return None;
    }

    // Manual CBC: AES-decode each block then XOR with the previous ciphertext.
    let cipher = Aes128::new(key.into());
    let mut plain = Vec::with_capacity(rest.len());
    let mut prev: [u8; 16] = [b' '; 16];
    for chunk in rest.as_chunks::<16>().0 {
        let mut block = Array::from(*chunk);
        cipher.decrypt_block(&mut block);
        for i in 0..16 {
            plain.push(block[i] ^ prev[i]);
        }
        prev.copy_from_slice(chunk);
    }

    // Strip PKCS7 padding.
    let pad = *plain.last()? as usize;
    if pad == 0 || pad > 16 || pad > plain.len() {
        return None;
    }
    let stripped = &plain[..plain.len() - pad];

    // v11 prepends SHA256(domain) — drop those 32 bytes.
    let stripped: &[u8] = match version {
        b"v11" => stripped.get(32..)?,
        _ => stripped,
    };
    String::from_utf8(stripped.to_vec()).ok()
}

/// Windows FILETIME (1601 epoch, microseconds) → Unix seconds. ≤0 → session.
fn chromium_time_to_unix(expires_utc_micros: i64) -> i64 {
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    if expires_utc_micros <= 0 {
        return 0;
    }
    expires_utc_micros / 1_000_000 - EPOCH_DIFF_SECS
}

/// Extract cookies for `domains` from a Chromium-family browser and save them
/// as a new profile. Returns (path, count).
pub fn capture_chromium(
    browser: &str,
    sites: &[String],
    profile_name: &str,
) -> Result<(PathBuf, usize), String> {
    if sites.is_empty() {
        return Err("no networks selected".to_string());
    }
    let domains: Vec<&str> = sites
        .iter()
        .flat_map(|s| domains_for_site(s))
        .copied()
        .collect();

    let paths =
        chromium_paths(browser).ok_or_else(|| format!("unsupported browser '{browser}'"))?;
    if !paths.cookies_db.is_file() {
        return Err(format!(
            "cookies database not found at {}",
            paths.cookies_db.display()
        ));
    }
    let candidate_keys = chromium_candidate_keys(&paths.service);

    let tmp = std::env::temp_dir().join(format!(
        "scrapmf-cookies-{}-{}.sqlite",
        browser,
        std::process::id()
    ));
    std::fs::copy(&paths.cookies_db, &tmp).map_err(|e| format!("copy cookies db: {e}"))?;

    let result = (|| {
        // encrypted_value read as HEX so it survives the TSV pipe.
        let rows: Vec<Vec<String>> = sqlite_read_rows(
            &tmp,
            "host_key,name,hex(COALESCE(encrypted_value,'')),value,path,expires_utc,is_secure,is_httponly",
            "cookies",
        )?;

        let mut filtered: Vec<StoredCookie> = Vec::new();
        let mut first_blob: Option<Vec<u8>> = None;
        for row in rows {
            if row.len() != 8 {
                continue;
            }
            let host_key = row[0].clone();
            let name = row[1].clone();
            let encrypted_hex = &row[2];
            let plain_value = row[3].clone();
            let path = row[4].clone();
            let expires_utc: i64 = row[5].parse().unwrap_or(0);
            let secure = row[6] == "1";
            let http_only = row[7] == "1";

            if !domains
                .iter()
                .any(|d| host_key == *d || host_key.ends_with(&format!(".{d}")))
            {
                continue;
            }
            let mut encrypted_value: Vec<u8> = Vec::new();
            if !encrypted_hex.is_empty() && encrypted_hex.len() % 2 == 0 {
                for b in encrypted_hex.as_bytes().chunks(2) {
                    let hex_str = std::str::from_utf8(b).unwrap_or("00");
                    if let Ok(byte) = u8::from_str_radix(hex_str, 16) {
                        encrypted_value.push(byte);
                    }
                }
            }
            if plain_value.is_empty() && first_blob.is_none() && !encrypted_value.is_empty() {
                first_blob = Some(encrypted_value.clone());
            }
            filtered.push(StoredCookie {
                domain: host_key.clone(),
                include_subdomains: host_key.starts_with('.'),
                path,
                secure,
                expires: chromium_time_to_unix(expires_utc),
                name,
                value: plain_value,
                http_only,
                encrypted_value,
            });
        }

        // Pick the candidate key whose PKCS7 padding validates on the first
        // encrypted cookie (keyring schema v1/v2/legacy derive different keys).
        let working_key: Option<[u8; 16]> = first_blob.as_ref().and_then(|blob| {
            candidate_keys
                .iter()
                .find(|k| decrypt_chromium_blob(blob, k).is_some())
                .copied()
        });

        let mut cookies = Vec::new();
        for mut cookie in filtered {
            if cookie.value.is_empty() && !cookie.encrypted_value.is_empty() {
                match working_key
                    .as_ref()
                    .and_then(|k| decrypt_chromium_blob(&cookie.encrypted_value, k))
                {
                    Some(v) => cookie.value = v,
                    None => continue,
                }
            }
            cookies.push(cookie);
        }

        if cookies.is_empty() {
            return Err(format!(
                "no cookies could be decrypted for {} — if your desktop \
                 keyring is locked, unlock it or use manual import instead",
                domains.join(", ")
            ));
        }

        let count = cookies.len();
        let mut content = to_netscape(&cookies);
        content = format!("{}{content}", source_metadata_line(browser, sites));
        let path = save_profile(profile_name, &content)?;
        Ok((path, count))
    })();

    let _ = std::fs::remove_file(&tmp);
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Netscape HTTP Cookie File\n\
.instagram.com\tTRUE\t/\tTRUE\t4102444800\tsessionid\tabc123\n\
.tiktok.com\tTRUE\t/\tTRUE\t0\tweb_session\txyz\n";

    #[test]
    fn parses_valid_netscape() {
        let cookies = parse_netscape(SAMPLE).expect("valid");
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].name, "sessionid");
        assert_eq!(cookies[0].domain, ".instagram.com");
        assert!(cookies[0].secure);
        assert_eq!(cookies[1].expires, 0); // session cookie
    }

    #[test]
    fn rejects_empty_or_garbage() {
        assert!(parse_netscape("").is_err());
        assert!(parse_netscape("garbage without tabs").is_err());
    }

    #[test]
    fn understands_httponly_prefix_and_skips_comments() {
        let content = "# Netscape HTTP Cookie File\n\
#HttpOnly_.instagram.com\tTRUE\t/\tTRUE\t4102444800\tsessionid\tz\n";
        let cookies = parse_netscape(content).expect("valid");
        assert_eq!(cookies.len(), 1);
        assert!(cookies[0].http_only);
        assert_eq!(cookies[0].value, "z");
    }

    #[test]
    fn roundtrip_netscape_preserves_fields() {
        let cookies = parse_netscape(SAMPLE).expect("parse");
        let out = to_netscape(&cookies);
        let reparsed = parse_netscape(&out).expect("re-parse");
        assert_eq!(cookies.len(), reparsed.len());
        for (a, b) in cookies.iter().zip(&reparsed) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.value, b.value);
            assert_eq!(a.domain, b.domain);
            assert_eq!(a.expires, b.expires);
            assert_eq!(a.http_only, b.http_only);
        }
    }

    /// Encrypt like Chromium does (v10): AES-128-CBC, IV of 16 spaces,
    /// PKCS7 padding — so decrypt_chromium_blob has a real roundtrip test.
    fn encrypt_v10(plain: &[u8], key: &[u8; 16]) -> Vec<u8> {
        use aes::Aes128;
        use aes::cipher::{Array, BlockCipherEncrypt, KeyInit};

        let pad = 16 - (plain.len() % 16);
        let mut data = plain.to_vec();
        data.extend(std::iter::repeat_n(pad as u8, pad));

        let cipher = Aes128::new(key.into());
        let mut out = Vec::with_capacity(data.len());
        let mut prev: [u8; 16] = [b' '; 16];
        for chunk in data.as_chunks::<16>().0 {
            let mut block: Array<u8, _> = (*chunk).into();
            for i in 0..16 {
                block[i] ^= prev[i];
            }
            cipher.encrypt_block(&mut block);
            prev.copy_from_slice(&block);
            out.extend_from_slice(&block);
        }
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&out);
        blob
    }

    #[test]
    fn decrypt_roundtrip_synthetic_v10() {
        let key = [0x2a_u8; 16];
        let secret = "session_id=ABC123; secure";
        let mut blob = encrypt_v10(secret.as_bytes(), &key);
        blob[..3].copy_from_slice(b"v10");
        assert_eq!(decrypt_chromium_blob(&blob, &key).as_deref(), Some(secret));
    }

    #[test]
    fn source_metadata_roundtrip() {
        let line = source_metadata_line("brave", &["instagram".into(), "tiktok".into()]);
        assert!(line.starts_with("# scrapmf-source: "));
        let meta = parse_source_metadata(&line).expect("parses");
        assert_eq!(meta.browser, "brave");
        assert_eq!(meta.networks, vec!["instagram", "tiktok"]);
    }

    #[test]
    fn source_metadata_absent_for_plain_content() {
        assert!(parse_source_metadata(SAMPLE).is_none());
    }

    #[test]
    fn windows_filetime_converts_to_unix() {
        assert_eq!(chromium_time_to_unix(0), 0);
        let unix = 1_767_225_600i64;
        let win = (unix + 11_644_473_600) * 1_000_000;
        assert_eq!(chromium_time_to_unix(win), unix);
    }

    #[test]
    fn domains_cover_known_sites() {
        assert_eq!(domains_for_site("tiktok"), &["tiktok.com"]);
        assert_eq!(domains_for_site("x"), &["twitter.com", "x.com"]);
        assert!(domains_for_site("nope").is_empty());
    }
}
