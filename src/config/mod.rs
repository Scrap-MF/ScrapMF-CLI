pub mod cookies;
pub mod model;

pub(crate) use fs::restrict_perms;
pub use model::{Account, Config, Preset, Profile, RateLimit, Site};

use std::path::{Path, PathBuf};

use anyhow::Context;

pub(crate) mod fs;
mod migrations;
mod templates;
use fs::write_config_file;
pub use migrations::{
    migrate_all_sites_filenames, migrate_all_sites_highlights, migrate_inline_config_to_files,
    migrate_legacy_placeholders,
};
pub use templates::{
    ensure_example_sites, ensure_tiktok_site, ensure_twitter_site, ensure_vsco_site,
    write_profile_file,
};

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    path.to_path_buf()
}

pub fn sites_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/sites"))
}

pub fn profiles_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/profiles"))
}

/// Site matches URL if `pattern` or any of `patterns` is contained in it.
fn site_matches(site: &Site, url: &str) -> bool {
    if let Some(ref pat) = site.pattern
        && url.contains(pat.as_str())
    {
        return true;
    }
    site.patterns.iter().any(|p| url.contains(p.as_str()))
}

pub fn resolve_site<'a>(
    url: &str,
    preset_name: Option<&str>,
    config: &'a Config,
) -> Option<(&'a String, &'a Site)> {
    if let Some(name) = preset_name {
        if let Some((k, site)) = config.sites.get_key_value(name) {
            return Some((k, site));
        }
        return None;
    }
    config
        .sites
        .iter()
        .find(|(_, site)| site_matches(site, url))
}

/// Load config from XDG `~/.config/scrapmf/config.toml` and presets from `presets/**/*.toml`.
pub fn load() -> anyhow::Result<Config> {
    // One-time migration: inline [sites]/[presets]/[profiles] → separate files.
    // Idempotent and cheap (early return if no inline tables).
    let _ = migrations::migrate_inline_config_to_files();
    let Some(path) = config_path() else {
        return Ok(Config::default());
    };
    let mut cfg = if path.exists() {
        let s = std::fs::read_to_string(&path)
            .with_context(|| format!("read config {}", path.display()))?;
        let c: Config =
            toml::from_str(&s).map_err(|e| crate::error::ScrapmfError::ConfigParse {
                path: path.clone(),
                source: e,
            })?;
        // Validate extra_args early
        for (name, preset) in &c.presets {
            crate::application::scraper::validate_extra_args(&preset.extra_args)
                .with_context(|| format!("preset '{name}' extra_args"))?;
        }
        c
    } else {
        Config::default()
    };

    // Expand tilde for general output_dir for consistency (store expanded)
    // Keep original tilde in file, but in-memory expand for use
    // Do not mutate file, just in-memory
    // Layered presets: sites/persons/*.toml override inline (legacy)
    if let Some(dir) = presets_dir() {
        load_layered_presets(&mut cfg, &dir);
        if let Some(sites) = presets_sites_dir() {
            load_layered_presets(&mut cfg, &sites);
        }
        if let Some(persons) = presets_persons_dir() {
            load_layered_presets(&mut cfg, &persons);
        }
    }
    // New 3-layer: sites/ and profiles/ (user requested structure)
    if let Some(dir) = sites_dir() {
        load_layered_sites(&mut cfg, &dir);
    }
    if let Some(dir) = profiles_dir() {
        load_layered_profiles(&mut cfg, &dir);
    }

    Ok(cfg)
}

fn load_layered_presets(cfg: &mut Config, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Recurse one level for sites/persons subdirs
            load_layered_presets(cfg, &path);
            continue;
        }
        if path.extension().is_some_and(|e| e == "toml")
            && let Some(stem) = path.file_stem().and_then(|n| n.to_str())
            && let Ok(s) = std::fs::read_to_string(&path)
            && let Ok(preset) = toml::from_str::<Preset>(&s)
        {
            // Validate before insert
            if crate::application::scraper::validate_extra_args(&preset.extra_args).is_ok() {
                cfg.presets.insert(stem.to_string(), preset);
            }
        }
    }
}

fn load_layered_sites(cfg: &mut Config, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_layered_sites(cfg, &path);
            continue;
        }
        if path.extension().is_some_and(|e| e == "toml")
            && let Some(stem) = path.file_stem().and_then(|n| n.to_str())
            && let Ok(s) = std::fs::read_to_string(&path)
            && let Ok(site) = toml::from_str::<Site>(&s)
            && crate::application::scraper::validate_extra_args(&site.extra_args).is_ok()
        {
            cfg.sites.insert(stem.to_string(), site);
        }
    }
}

fn load_layered_profiles(cfg: &mut Config, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_layered_profiles(cfg, &path);
            continue;
        }
        if path.extension().is_some_and(|e| e == "toml")
            && let Some(stem) = path.file_stem().and_then(|n| n.to_str())
            && let Ok(s) = std::fs::read_to_string(&path)
            && let Ok(mut profile) = toml::from_str::<Profile>(&s)
        {
            // Legacy migration: sites = ["instagram"] -> accounts
            if profile.accounts.is_empty() && !profile.sites.is_empty() {
                for site in profile.sites.clone() {
                    profile.accounts.entry(site).or_default().push(Account {
                        username: None,
                        ..Default::default()
                    });
                }
                profile.sites.clear();
            }
            cfg.profiles.insert(stem.to_string(), profile);
        }
    }
}

/// Resolve preset by explicit name or auto-match URL pattern.
pub fn resolve_preset(url: &str, preset_name: Option<&str>, config: &Config) -> Option<Preset> {
    if let Some(name) = preset_name {
        return config.presets.get(name).cloned();
    }
    // Auto-match by pattern contains
    config
        .presets
        .values()
        .find(|preset| {
            preset
                .pattern
                .as_ref()
                .is_some_and(|pat| url.contains(pat.as_str()))
        })
        .cloned()
}

/// Save config to XDG path with 0o600.
pub fn save(cfg: &Config) -> anyhow::Result<()> {
    let Some(path) = config_path() else {
        anyhow::bail!("cannot determine config path");
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create config dir")?;
        fs::restrict_perms(parent, true);
    }
    let content = toml::to_string_pretty(cfg).context("serialize config")?;
    write_config_file(&path, &content)
}

/// Config file path (XDG)
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/config.toml"))
}

/// Presets directory
pub fn presets_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("scrapmf/presets"))
}

pub fn presets_sites_dir() -> Option<PathBuf> {
    presets_dir().map(|p| p.join("sites"))
}

pub fn presets_persons_dir() -> Option<PathBuf> {
    presets_dir().map(|p| p.join("persons"))
}

pub fn expand_output_dir(path: &Path) -> PathBuf {
    expand_tilde(path)
}

/// One-time migration from the pre-rename XDG directories
/// (`scarpmf` -> `scrapmf`). Idempotent: if the new directory already exists,
/// the legacy one is left untouched; nothing is ever deleted.
pub fn migrate_legacy_dirs() {
    if let Some(cfg) = dirs::config_dir() {
        migrate_one(&cfg.join("scarpmf"), &cfg.join("scrapmf"));
    }
    if let Some(data) = dirs::data_dir() {
        migrate_one(&data.join("scarpmf"), &data.join("scrapmf"));
    }
}

fn migrate_one(old: &Path, new: &Path) {
    if !old.is_dir() || new.exists() {
        return;
    }
    match std::fs::rename(old, new) {
        Ok(()) => tracing::info!(
            from = %old.display(),
            to = %new.display(),
            "migrated legacy data directory to new app name"
        ),
        Err(e) => tracing::warn!(
            error = %e,
            from = %old.display(),
            to = %new.display(),
            "could not migrate legacy data directory"
        ),
    }
}

/// Ensure base config directories exist with 0o700 (idempotent).
pub fn ensure_config_dirs() -> anyhow::Result<()> {
    if let Some(base) = dirs::config_dir().map(|p| p.join("scrapmf")) {
        std::fs::create_dir_all(&base).context("create scrapmf config dir")?;
        crate::config::fs::restrict_perms(&base, true);
    }
    for d in [sites_dir(), profiles_dir()].into_iter().flatten() {
        std::fs::create_dir_all(&d).with_context(|| format!("create {}", d.display()))?;
        crate::config::fs::restrict_perms(&d, true);
    }
    Ok(())
}

/// Ensure ~/.config/scrapmf/config.toml exists with 0o600 (no clobber).
pub fn ensure_default_config() -> anyhow::Result<()> {
    let Some(path) = config_path() else {
        return Ok(());
    };
    if path.exists() {
        // Migrate old default output_dir = "~" to "~/scrapmf"
        if let Ok(s) = std::fs::read_to_string(&path)
            && s.contains("output_dir = \"~\"")
            && !s.contains("output_dir = \"~/scrapmf\"")
        {
            let new = s.replace("output_dir = \"~\"", "output_dir = \"~/scrapmf\"");
            match write_config_file(&path, &new) {
                Ok(()) => tracing::info!(
                    path = %path.display(),
                    "migrated output_dir \"~\" -> \"~/scrapmf\""
                ),
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "could not migrate default output_dir"
                ),
            }
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create config parent")?;
        crate::config::fs::restrict_perms(parent, true);
    }
    let cfg = Config::default();
    let body = toml::to_string_pretty(&cfg).context("serialize default config")?;
    let header = r#"# scrapmf — main config
# XDG: ~/.config/scrapmf/config.toml (0o600, dir 0o700)
# This file is the global defaults. Site and profile files override it.
#
# [general]
#   output_dir = "~/scrapmf"                    # base output dir (HOME/scrapmf; CLI --output overrides; tilde ~/ expanded)
#   archive = true                              # download archive (dedup per-account)
# See: sites/*.toml for per-site options

"#;
    let content = format!("{header}{body}");
    write_config_file(&path, &content)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod legacy_migration_tests {
    use super::migrate_one;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn migrates_legacy_dir_when_new_missing() {
        let base = TempDir::new().expect("tempdir");
        let old = base.path().join("scarpmf");
        let new = base.path().join("scrapmf");
        std::fs::create_dir_all(old.join("sites")).expect("seed");
        std::fs::write(old.join("config.toml"), "x = 1").expect("seed file");

        migrate_one(&old, &new);

        assert!(!old.exists(), "legacy dir should be moved, not copied");
        assert!(new.join("config.toml").is_file());
        assert!(new.join("sites").is_dir());
    }

    #[test]
    fn no_touch_when_new_already_exists() {
        let base = TempDir::new().expect("tempdir");
        let old = base.path().join("scarpmf");
        let new = base.path().join("scrapmf");
        std::fs::create_dir_all(&old).expect("seed old");
        std::fs::write(old.join("old.toml"), "o").expect("seed");
        std::fs::create_dir_all(&new).expect("seed new");
        std::fs::write(new.join("new.toml"), "n").expect("seed");

        migrate_one(&old, &new);

        // Both survive untouched
        assert!(old.exists());
        assert!(Path::new(&new).join("new.toml").is_file());
        assert!(!Path::new(&new).join("old.toml").exists());
    }

    #[test]
    fn no_op_when_nothing_exists() {
        let base = TempDir::new().expect("tempdir");
        migrate_one(&base.path().join("ghost"), &base.path().join("target"));
        assert!(!base.path().join("target").exists());
    }
}
