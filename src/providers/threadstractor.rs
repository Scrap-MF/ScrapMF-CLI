use std::ffi::OsString;
use std::path::PathBuf;

use super::{Provider, ScrapeRequest};

/// Provider for Threads via the `threadstractor` Python package.
/// Mirrors gallery-dl provider but calls the threadstractor binary
/// which handles `post_id` naming, `date` sorting and anti rate-limit.
pub struct Threadstractor;

impl Threadstractor {
    pub fn binary() -> anyhow::Result<PathBuf> {
        // Try threadstractormf in PATH (pipx / uv / pip --user)
        // Falls back to python -m threadstractormf if binary not found
        if let Ok(path) = which::which("threadstractormf") {
            return Ok(path);
        }
        anyhow::bail!(
            "threadstractormf binary not found in PATH. Install with: pipx install threadstractormf  or  pip install threadstractormf  or  uv pip install -e ../python-proyect"
        )
    }

    fn binary_with_fallback() -> (String, Vec<OsString>) {
        if let Ok(path) = which::which("threadstractormf") {
            return (path.to_string_lossy().into_owned(), vec![]);
        }
        // Fallback: python -m threadstractormf
        (
            String::from("python"),
            vec![OsString::from("-m"), OsString::from("threadstractormf")],
        )
    }
}

impl Provider for Threadstractor {
    fn name(&self) -> &str {
        "threadstractormf"
    }

    fn is_available(&self) -> bool {
        which::which("threadstractormf").is_ok()
    }

    fn version(&self) -> anyhow::Result<String> {
        let (bin, prefix) = Self::binary_with_fallback();
        let mut args = prefix;
        args.push(OsString::from("--help"));
        let output = crate::process::Executor::run_capturing(&bin, &args)?;
        // threadstractor --help prints usage, no --version yet, return first line
        let out = String::from_utf8_lossy(&output.stdout);
        Ok(out
            .lines()
            .next()
            .unwrap_or("threadstractor")
            .trim()
            .to_string())
    }

    #[allow(clippy::collapsible_if)]
    fn build_args(&self, req: &ScrapeRequest) -> anyhow::Result<Vec<OsString>> {
        let mut args = Vec::new();

        // Cookies (same as gallery-dl)
        if let Some(ref file) = req.cookies_file {
            args.push(OsString::from("--cookies"));
            args.push(file.as_os_str().to_owned());
        }
        if let Some(ref browser) = req.cookies_from_browser {
            args.push(OsString::from("--cookies-from-browser"));
            args.push(OsString::from(browser));
        }

        if req.profile_pic_only {
            args.push(OsString::from("--profile-pic-only"));
        }

        // Archive: threadstractor doesn't have its own archive yet (deferred), but
        // scrapmf will manage dedup via its JSONL if we pass --download-archive in future.
        // For now ignore archive param (scrapmf's archive still seeds/skips via its own logic).

        // Rate limit -> threadstractor flags
        if let Some(ref rl) = req.rate_limit {
            if let Some(ref s) = rl.sleep {
                // sleep "3-6" range -> take lower bound for cooldown ms
                if let Some(ms) = parse_sleep_to_ms(s) {
                    args.push(OsString::from("--cooldown"));
                    args.push(OsString::from(ms.to_string()));
                }
            }
            if let Some(ref sr) = rl.sleep_request {
                if let Some(ms) = parse_sleep_to_ms(sr) {
                    // map sleep_request to rps ~ 1000/ms
                    let rps = if ms > 0 { 1000.0 / ms as f64 } else { 0.5 };
                    args.push(OsString::from("--rps"));
                    args.push(OsString::from(format!("{rps:.2}")));
                }
            }
            if let Some(s429) = rl.sleep_429 {
                // batch cooldown approx
                let ms = s429 as u64 * 1000;
                args.push(OsString::from("--batch-cooldown"));
                args.push(OsString::from(ms.to_string()));
            }
            if let Some(ref lr) = rl.limit_rate {
                // not yet mapped, ignore
                let _ = lr;
            }
        }

        if let Some(out) = &req.output {
            let expanded = crate::config::expand_output_dir(out);
            args.push(OsString::from("--dest"));
            args.push(expanded.as_os_str().to_owned());
        }

        // Filename / directory templates: threadstractor supports templating via
        // --filename-template and --directory-template (f-string style).
        if req.profile_pic_only {
            args.push(OsString::from("--filename-template"));
            args.push(OsString::from("{username}_profile.{extension}"));
            let use_caller = req
                .directory_template
                .as_ref()
                .is_some_and(|d| d.iter().any(|s| s.contains("profile")));
            if use_caller {
                if let Some(ref dirs) = req.directory_template {
                    let joined = dirs.join("/");
                    args.push(OsString::from("--directory-template"));
                    args.push(OsString::from(joined));
                }
            } else {
                args.push(OsString::from("--directory-template"));
                args.push(OsString::from(
                    "{scrapmf_root}/{category}/{username}/profile",
                ));
            }
        } else {
            if let Some(ref tmpl) = req.filename_template {
                args.push(OsString::from("--filename-template"));
                args.push(OsString::from(tmpl));
            }
            if let Some(ref dirs) = req.directory_template {
                let joined = dirs.join("/");
                args.push(OsString::from("--directory-template"));
                args.push(OsString::from(joined));
            }
        }

        // Extractor options for threads: allow overriding filename_template via -o
        // For now, extractor_options are ignored except filename/directory which are already handled.

        // Extra args (allow-list validated) — filter gallery-dl-only flags
        let mut skip_next = false;
        for extra in &req.extra_args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if extra == "--restrict-filenames" {
                skip_next = true; // skip its value (auto)
                continue;
            }
            // gallery-dl flag that threadstractor doesn't need (already handled via rate_limit mapping)
            if extra == "--sleep"
                || extra == "--sleep-request"
                || extra == "--sleep-429"
                || extra == "--limit-rate"
            {
                skip_next = true;
                continue;
            }
            args.push(OsString::from(extra));
        }

        // Target URL / @username — threadstractor accepts @user or URL
        args.push(OsString::from(&req.url));
        Ok(args)
    }
}

fn parse_sleep_to_ms(s: &str) -> Option<u64> {
    // "3-6" -> take average or lower bound (we use lower for safety)
    // "5" -> 5000
    let first = s.split('-').next()?.trim();
    let secs: f64 = first.parse().ok()?;
    Some((secs * 1000.0) as u64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::Threadstractor;
    use crate::application::ScrapeRequest;
    use crate::providers::Provider;

    fn req(url: &str) -> ScrapeRequest {
        ScrapeRequest {
            url: url.to_string(),
            output: None,
            preset: None,
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
            no_archive: false,
            profile_pic_only: false,
        }
    }

    #[test]
    fn build_args_with_templates() {
        let mut r = req("https://www.threads.com/@user");
        r.filename_template = Some("{date:%Y-%m-%d}_{post_id}_{num:02d}.{extension}".to_string());
        r.directory_template = Some(vec!["{scrapmf_root}".to_string(), "{category}".to_string()]);
        let args = Threadstractor.build_args(&r).unwrap();
        assert!(args.iter().any(|a| a == "--filename-template"));
        assert!(args.iter().any(|a| a.to_string_lossy().contains("{date:")));
        assert!(args.iter().any(|a| a == "--directory-template"));
    }

    #[test]
    fn build_args_no_templates() {
        let r = req("https://www.threads.com/@user");
        let args = Threadstractor.build_args(&r).unwrap();
        assert!(!args.iter().any(|a| a == "--filename-template"));
    }
}
