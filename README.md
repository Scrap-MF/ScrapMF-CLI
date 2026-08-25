# scrapmf — Safe, interactive archiver for social media galleries

> Fast, safe, interactive wrapper for archiving galleries. **Does not reimplement download logic** — it delegates securely to a bundled, version-pinned `gallery-dl` via `std::process::Command` (no shell, no injection).

[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue)](LICENSE)
[![CI](https://github.com/Scrap-MF/ScrapMF-CLI/actions/workflows/ci.yml/badge.svg)](https://github.com/Scrap-MF/ScrapMF-CLI/actions/workflows/ci.yml)

---

## What is scrapmf?

`scrapmf` is a single-binary CLI that orchestrates external scrapers for **social and media networks** (TikTok, Instagram, Twitter/X, VSCO, …).

- **For:** anyone who wants to archive personal galleries — backup, offline viewing or curation.
- **Backends:** [gallery-dl](https://github.com/mikf/gallery-dl) does the heavy lifting (300+ sites); `yt-dlp` is detected by `doctor` and reserved for future use.
- **Two modes:**
  - **Automation (CI/scripts):** `scrapmf scrape <URL> --output ./out`
  - **Interactive:** run `scrapmf` with no args on a TTY → guided menus (profiles, per-account content selection, live dashboard)
- **Universal Linux:** static musl builds for x86_64 and aarch64 — runs on Arch, Fedora, Debian, Ubuntu, openSUSE, NixOS, WSL, Raspberry Pi…

## Highlights

| Feature | Detail |
|---|---|
| XDG TOML config | `~/.config/scrapmf/config.toml` + layered `sites/*.toml`, `profiles/*.toml` |
| Per-site presets | Auto-matched by URL pattern; rate limits, cookies, extractor options |
| Interactive profiles | One person → multiple accounts across sites, content-type selection |
| Live TUI dashboard | Progress per job, log feed, Ctrl+C cancels in <1s even mid-download |
| Integrity checks | Post-scrape audit: truncated MP4s (moov scan), codec/resolution summary, `.part` orphans |
| Challenge awareness | Detects anti-bot losses and tells you to refresh your browser session |
| Safe orchestration | argv[]-only invocation, allow-listed extra args, path traversal rejection |
| Good errors | `help:` / `note:` hints like rustc, `NO_COLOR` support |

## Scope & Disclaimer

- **Respect ToS & copyright:** this tool is for **personal archiving** of content you have the right to access. You are responsible for complying with each site's Terms of Service and local copyright law.
- **Security:** credentials (cookies) should be passed via file (`--cookies path`) or browser extraction — never logged. See below.
- **No paywall bypass:** scrapmf implements no extractors; it calls gallery-dl as shipped upstream.

---

## Installation

### Step 1 — Install scrapmf

All methods put `scrapmf` in your `$PATH`. Pick one.

**One-liner (recommended):**

```bash
curl -fsSL https://raw.githubusercontent.com/Scrap-MF/ScrapMF-CLI/main/install.sh | sh
```

Downloads the latest static release for your architecture (musl build — no dependencies), verifies its checksum, and installs to `~/.local/bin`.

Options: pass a version tag (`install.sh v1.0.0`), an install dir (2nd arg), or `--gnu` for the dynamically-linked build.

**Cargo** (requires Rust 1.88+):

```bash
cargo install --path .                 # from this repo
cargo binstall scrapmf                 # prebuilt binary via cargo-binstall
```

**AUR** (Arch / Garuda / Manjaro):

```bash
paru -S scrapmf-bin   # prebuilt static binary
# or
paru -S scrapmf       # build from source
```

```bash
scrapmf --version      # verify any method
```

### Step 2 — Set up the gallery-dl backend (recommended)

scrapmf ships with its **own pinned copy of gallery-dl**, independent from any system installation:

```bash
scrapmf setup        # downloads official gallery-dl v1.32.9 (~23 MB) + SHA256 verification
scrapmf doctor       # shows: gallery-dl 1.32.9 found [bundled (pinned)] pinned v1.32.9
```

- The managed binary lives in `~/.local/share/scrapmf/bin/` and **never updates on its own** — new pins ship with scrapmf releases, so upstream changes can never break your setup mid-archive.
- Your own gallery-dl (if any) is left untouched; scrapmf just won't use it.
- x86_64: standalone build, no Python needed. aarch64: `setup` prints the equivalent pinned pipx command (`pipx install gallery-dl==1.32.9`).

<details>
<summary>Manual backend alternative (without the bundled copy)</summary>

If you prefer managing gallery-dl yourself, scrapmf will fall back to it from `$PATH` (labelled <em>system, NOT pinned</em> by <code>doctor</code>):

```bash
pipx install gallery-dl==1.32.9   # or your distro's package manager
```

</details>

---

## Quick Start

```bash
# dry-run — list what would be downloaded (makes real network requests)
scrapmf scrape "https://www.tiktok.com/@user/video/123" --output ./out --dry-run

# real scrape
scrapmf scrape "https://twitter.com/username/media" --output ./downloads

# verbose/debug (-v info, -vv debug incl. provider args)
scrapmf -vv scrape "https://example.com/gallery/123"

# interactive mode (TTY only): profiles, quick scrape, site/profile management
scrapmf

# check backends, browser cookies, system
scrapmf doctor
```

After each scrape you get an integrity report: incomplete downloads, orphaned `.part` files, and a quality line like `quality: 1080x1920 avc1 ×14, 720x1280 hvc1 ×2`. If TikTok's anti-bot ate some posts, you'll see exactly how many and how to fix it (refresh your browser session).

> **Instagram note:** reels live only in `reels/` — scrapmf filters them out of
> the profile feed pass (`posts/`), so nothing is downloaded twice even though
> Instagram mixes reels into the regular posts feed.

Non-interactive fallback (no TTY, e.g. CI):

```bash
echo "https://example.com" | scrapmf   # exits 2: help shown
```

Exit codes: `0` success · `1` error · `2` missing subcommand · `130` cancelled by user (Ctrl+C).

> **Coming from scarpmf?** Your data migrates automatically: on first run,
> `~/.config/scarpmf` and `~/.local/share/scarpmf` (including the bundled
> gallery-dl) are renamed to their new `scrapmf` equivalents. Nothing to redo.

---

## Configuration

XDG-compliant: `$XDG_CONFIG_HOME/scrapmf/config.toml` (default `~/.config/scrapmf/config.toml`), created automatically on first use.

```toml
[general]
output_dir = "~/scrapmf"
```

Layered overrides in `~/.config/scrapmf/sites/*.toml` (per-site: patterns, rate limits, cookies, templates, extractor options) and `profiles/*.toml` (per-person: multiple accounts across sites). Example files are generated on first run — including a fully documented `sites/tiktok.toml`.

Backend overrides (rarely needed):

```toml
[backend]
# gallery_dl_path = "/custom/path/gallery-dl"  # override even the bundled copy
auto_install_backends = true                    # offer bundled install on first TTY run
```

File permissions: `0o600` files, `0o700` dirs. Config rewrites are atomic and leave a `.bak.<timestamp>` backup. scrapmf never logs secrets.

```bash
scrapmf config list    # show current config
scrapmf config path    # print config file path
scrapmf config edit    # open $EDITOR
```

## Security

Golden rule: **never `sh -c`**. Backends run as `Command::new("gallery-dl").arg(...).arg(url)` — args are `argv[]` literals; `;|&$()` are never interpreted.

- URLs validated: http/https only, length ≤2048.
- Output paths validated against protected system trees (`/etc`, `/usr`, `/var`, `/boot`, … — anything inside them is rejected too); `..` rejected.
- Preset `extra_args` are allow-list validated; `--exec` is always refused.
- Config writes are atomic (`.tmp` + rename) with automatic pre-rewrite backups.

---

## Contributing

```bash
git clone https://github.com/Scrap-MF/ScrapMF-CLI
cd ScrapMF-CLI
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo run -- --help
```

### Validate before pushing (automatic)

Git hooks are installed automatically the first time you run `cargo test`
(via [cargo-husky](https://github.com/rhysd/cargo-husky) — no manual setup):

- **`.hooks/commit-msg`** → enforces Conventional Commits at commit time
  (requires `cargo install convco`; merge/revert commits exempt)
- **`.hooks/pre-push`** → blocks the push if validation fails:
  - pushing to `main`: full CI parity (`just validate`, ~5 min)
  - any other branch: fast gate (`just quick`, ~40 s)
  - requires `cargo install just --locked`

The validation pipeline mirrors the professional standard:

```
cargo fmt → cargo check → cargo clippy → cargo nextest run
          → cargo audit → cargo deny check → cargo build --release
```

All recipes live in the `justfile` — CI runs the exact same commands, so
anything green locally is green on GitHub.

```bash
just quick      # fmt + check + clippy + nextest (~40s)
just validate   # full parity: quick + audit + deny + release build (~5 min)
just tools      # one-time install of just/convco/cargo-nextest/cargo-deny/cargo-audit
```

Notes:
- tests run through [cargo-nextest](https://nexte.st) (per-test process
  isolation); use `just legacy-test` for the classic runner (doc-tests).
- Security policy lives in `.cargo/deny.toml` (advisories, licenses,
  bans, sources — single gate via `cargo deny check`). Consciously
  tolerated advisories are documented in its `[advisories] ignore` list.

Commits must follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(cli): add --preset flag
fix(providers): handle gallery-dl not found
chore(config): rotate migration backups
```

Releases: push a `v*` tag — CI builds static binaries for x86_64 and aarch64 (gnu + musl) and attaches them to the GitHub Release with SHA-256 checksums.

### Version bumps are automated

Versions are **derived from Conventional Commits** — never chosen by hand:

| Commits since last release | Next version |
|---|---|
| `fix:` | patch (1.0.1) |
| `feat:` | minor (1.1.0) |
| `feat!:` / `BREAKING CHANGE` | major (2.0.0) |

Flow ([release-plz](https://release-plz.dev)):

1. Work lands on `main` via merges from `develop`.
2. The release-plz workflow opens a **"chore(release): vX.Y.Z" PR** with the
   version bump already applied to `Cargo.toml` / `Cargo.lock`.
3. Merging that PR makes release-plz push the `vX.Y.Z` tag.
4. CI builds the 4 static binaries and publishes the GitHub Release.

Check what version would come next at any time:

```bash
just release-preview
```

---

## License

`GPL-3.0-only` — see `LICENSE`.

---

## Acknowledgements

Built on [gallery-dl](https://github.com/mikf/gallery-dl). Not affiliated.
