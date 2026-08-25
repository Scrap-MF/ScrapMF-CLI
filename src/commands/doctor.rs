use anyhow::Result;

use crate::config;
use crate::output;
use crate::providers::gallery_dl::GalleryDl;
use crate::providers::{Provider, browser::detect_available_browsers};

pub fn run(verbose: u8) -> Result<()> {
    tracing::debug!(verbose = verbose, "doctor start");
    println!("scrapmf doctor — checking backends and system");
    println!("─────────────────────────────────────────────");

    let mut ok = true;

    // Check gallery-dl (resolved source: bundled pinned > overrides > system)
    let gallery = GalleryDl;
    let source = crate::application::backend::resolve(
        config::load()
            .unwrap_or_default()
            .backend
            .gallery_dl_path
            .clone(),
    );
    if gallery.is_available() {
        match gallery.version() {
            Ok(v) if !v.is_empty() => output::print_success(&format!(
                "gallery-dl {v} found [{}]{}",
                source.label(),
                if matches!(source, crate::application::backend::Source::Managed(_)) {
                    format!(" pinned v{}", crate::application::backend::GALLERY_DL_PIN)
                } else {
                    String::new()
                }
            )),
            Ok(_) | Err(_) => {
                // Binary exists but --version failed or printed nothing
                output::print_error("gallery-dl found but --version failed");
                ok = false;
            }
        }
    } else {
        output::print_error("gallery-dl not found in $PATH");
        output::print_help(
            "run scrapmf (interactive) and it will offer to install the pinned backend; \
             'scrapmf setup' also works",
        );
        ok = false;
    }

    // Check browsers for cookies
    let browsers = detect_available_browsers();
    let available: Vec<_> = browsers.iter().filter(|b| b.available).collect();
    if available.is_empty() {
        output::print_info(
            "No browser cookie DBs detected (checked: firefox, brave, chrome, chromium, edge, opera, vivaldi)",
        );
        for b in &browsers {
            tracing::debug!(browser = %b.id, display = %b.display, "browser check");
        }
    } else {
        output::print_success("Browsers with cookies:");
        for b in available {
            println!("  - {}", b.display);
        }
    }

    // Check resolved backend binary is reachable
    if crate::application::backend::gallery_dl_executable().is_ok() {
        tracing::debug!("backend resolution OK");
        if verbose > 0 {
            output::print_success("Backend resolution OK");
        }
    }

    // Check temp dir writable
    let test_dir = std::env::temp_dir().join("scrapmf_doctor_test");
    match std::fs::create_dir_all(&test_dir) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&test_dir);
            output::print_success(&format!("Temp dir writable: {}", test_dir.display()));
        }
        Err(e) => {
            output::print_error(&format!("Temp dir not writable: {e}"));
            ok = false;
        }
    }

    println!("─────────────────────────────────────────────");
    if ok {
        output::print_success("All checks passed");
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "some doctor checks failed (see details above)"
        ))
    }
}
