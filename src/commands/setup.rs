use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::application::backend::{self, GALLERY_DL_ASSET, GALLERY_DL_PIN};
use crate::{config, output};

/// `scrapmf setup` — install the bundled pinned gallery-dl.
///
/// The managed copy lives in `$XDG_DATA_HOME/scrapmf/bin/` and is frozen at
/// [`GALLERY_DL_PIN`]: it never updates on its own and never touches the
/// user's system gallery-dl. Upgrades happen only by upgrading scrapmf (which
/// ships a new pin) and re-running this command.
pub fn run(yes: bool) -> Result<()> {
    let source = backend::resolve(
        config::load()
            .unwrap_or_default()
            .backend
            .gallery_dl_path
            .clone(),
    );
    if let backend::Source::Managed(p) = &source {
        output::print_success(&format!(
            "bundled gallery-dl v{GALLERY_DL_PIN} already installed at {}",
            p.display()
        ));
        print_version(p);
        return Ok(());
    }
    output::print_info(&format!(
        "Current resolution: {}",
        match &source {
            backend::Source::Missing => "no gallery-dl found".to_string(),
            s => format!(
                "{} ({})",
                s.path().unwrap_or(Path::new("?")).display(),
                s.label()
            ),
        }
    ));

    // aarch64 has no official standalone build → pinned pipx fallback
    if !is_x86_64() {
        print_pipx_fallback();
        return Ok(());
    }

    install_managed(yes)
}

fn is_x86_64() -> bool {
    matches!(std::env::consts::ARCH, "x86_64")
}

fn install_managed(yes: bool) -> Result<()> {
    let dir = backend::managed_dir().context("cannot determine data directory")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let url = format!("{}/{}", backend::release_base_url(), GALLERY_DL_ASSET);
    println!("→ Installing bundled gallery-dl v{GALLERY_DL_PIN} (~23 MB download)");
    if !yes
        && !confirm(&format!(
            "Download and install gallery-dl v{GALLERY_DL_PIN} now?"
        ))
    {
        println!("cancelled");
        return Ok(());
    }

    let work = dir.join(format!(".setup-{}", std::process::id()));
    std::fs::create_dir_all(&work)?;
    let asset_path = work.join(GALLERY_DL_ASSET);
    let sums_path = work.join("SHA256SUMS");

    fetch(&asset_path, &url)?;
    fetch(
        &sums_path,
        &format!("{}/SHA256SUMS", backend::release_base_url()),
    )?;

    echo_step("Verifying SHA256 checksum");
    let sums = std::fs::read_to_string(&sums_path).context("read SHA256SUMS")?;
    let expected = backend::extract_sha256_for(&sums, GALLERY_DL_ASSET)
        .context("gallery-dl.bin not listed in official SHA256SUMS — aborting")?;
    verify_sha256(&asset_path, &expected)?;

    let target = dir.join(GALLERY_DL_ASSET);
    make_executable(&asset_path)?;
    std::fs::rename(&asset_path, &target)
        .with_context(|| format!("move into {}", target.display()))?;
    let _ = std::fs::remove_dir_all(&work);

    output::print_success(&format!(
        "bundled gallery-dl v{GALLERY_DL_PIN} ready at {}",
        target.display()
    ));
    output::print_info("  It never updates on its own; new pins ship with scrapmf releases.");
    print_version(&target);
    Ok(())
}

fn print_pipx_fallback() {
    output::print_note(&format!(
        "no standalone build for {} — install the pinned version via pipx instead:",
        std::env::consts::ARCH
    ));
    println!("  pipx install --system-site-packages gallery-dl=={GALLERY_DL_PIN}");
    output::print_info("  pipx never auto-upgrades it, so the version stays frozen.");
}

fn print_version(path: &Path) {
    match crate::process::Executor::run_capturing(
        &path.to_string_lossy(),
        &[std::ffi::OsString::from("--version")],
    ) {
        Ok(out) => {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            output::print_success(&format!("gallery-dl {v} verified working"));
        }
        Err(e) => tracing::warn!(error = %e, "could not verify installed binary"),
    }
}

/// First-run interactive offer: called from main before entering interactive
/// mode. Never prompts outside a TTY; respects auto_install_backends.
pub fn offer_if_missing() {
    use std::io::IsTerminal;
    let cfg = config::load().unwrap_or_default();
    if !cfg.backend.auto_install_backends || !std::io::stdin().is_terminal() {
        return;
    }
    if !matches!(
        backend::resolve(cfg.backend.gallery_dl_path.clone()),
        backend::Source::Managed(_)
    ) {
        let _ = run(false); // confirm prompt inside; cancellation is non-fatal
    }
}

// --- helpers -----------------------------------------------------------------

fn echo_step(msg: &str) {
    println!("→ {msg}");
}

fn fetch(dest: &Path, url: &str) -> Result<()> {
    echo_step(&format!("Downloading {url}"));
    let status = if command_exists("curl") {
        std::process::Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(dest)
            .arg(url)
            .status()
            .context("launch curl")?
    } else if command_exists("wget") {
        std::process::Command::new("wget")
            .arg("-qO")
            .arg(dest)
            .arg(url)
            .status()
            .context("launch wget")?
    } else {
        anyhow::bail!("curl or wget required to download the backend");
    };
    if !status.success() {
        anyhow::bail!("download failed ({url})");
    }
    Ok(())
}

fn verify_sha256(file: &Path, expected_hex: &str) -> Result<()> {
    let out = std::process::Command::new("sha256sum")
        .arg(file)
        .output()
        .context("run sha256sum")?;
    let text = String::from_utf8_lossy(&out.stdout);
    let got = text
        .split_whitespace()
        .next()
        .context("sha256sum produced no output")?
        .to_ascii_lowercase();
    if got != expected_hex.to_ascii_lowercase() {
        anyhow::bail!(
            "checksum mismatch for {}: expected {expected_hex}, got {got}",
            file.display()
        );
    }
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(path)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(path, perm)?;
    }
    Ok(())
}

fn command_exists(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

fn confirm(question: &str) -> bool {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        // Non-interactive: never escalate or download without explicit --yes
        output::print_note(&format!("{question} Use 'scrapmf setup --yes' to proceed."));
        return false;
    }
    print!("{question} [Y/n] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    matches!(answer.trim(), "" | "y" | "Y" | "yes" | "Yes")
}
