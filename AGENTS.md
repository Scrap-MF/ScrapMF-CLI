# AGENTS.md — rules for agents working on scrapmf

Rust CLI for downloading media from Threads, Instagram, TikTok, Twitter/X, VSCO and
Facebook. `gallery-dl` is the backend for every site except Threads, which uses the
`threadstractormf` plugin resolved through `crate::plugins`.

## Before any change

- Work on `develop`. `main` is what gets published.
- `just quick` is the fast gate and mirrors the `ci` job exactly:
  `cargo fmt --check` → `cargo check` → `cargo clippy --all-targets -- -D warnings` →
  `cargo nextest run`.
- `just validate` is full CI parity (`quick` + release build + `cargo deny`).
- Clippy is enforced with `-D warnings`. A new warning fails the build.
- A `pre-push` hook (cargo-husky) runs the gate automatically on every push.

## Shipping: `develop` → `main` goes through a PR

**Never push straight to `main`.** Merge locally and push it, and the code lands in
`main` before anything has verified it — GitHub Actions only runs afterwards, on a
commit that is already public. Open a PR, wait for the `ci` job to go green, then
merge.

There are two different PRs and they are not interchangeable:

| PR | What it does | When |
|---|---|---|
| `develop` → `main` | gates the code; `ci` runs on it | **before** the merge |
| `release-plz/*` | bumps the version and pushes the tag | **after**, by design |

The release PR cannot come first: release-plz computes the bump from commits that are
already in `main`.

## Versions

- Do not hand-edit `version` in `Cargo.toml`. release-plz owns it and reads the
  current version from the git tag (`git_only = true`).
- Conventional Commits drive the bump: `feat` → minor, `fix` → patch. The commit
  subject is the input, so a `feat` merged after the last tag means a minor bump even
  when a previous release already included the rest of that work.

## Layout worth knowing

- `~/.config/scrapmf/sites/*.toml` — per-site output rules, rate limits, templates.
- `~/.config/scrapmf/profiles/*.toml` — saved scraping profiles: site, accounts,
  cookies, output overrides. Content is chosen per run.
- `~/.config/scrapmf/tasks/*.toml` — saved content batches: content and accounts are
  fixed. `Account` fields are flattened into each account, so `output_dir` and
  `extra_args` written there are honoured (`output_dir` precedence: account → site →
  global).
- `~/.config/state/scrapmf/logs/` — per-run logs, which is where a provider's own
  warnings end up.

## Writing tests

- Interaction that needs a TTY cannot be unit-tested; test the decision instead and
  keep it in a pure function. Several real bugs here were a decision inlined into a
  control-flow branch.
- Test the *composition*, not just each half. A field that is set and a helper that
  is correct can still compose into something that never reaches the wire.
- When adding a file a caller must not have to pre-create, cover that path. A test
  whose helper creates the directory is hiding the bug.

## Interactive conventions

- `Esc` means "step back one question" and never "confirm" or "save". `Ctrl+C` leaves.
  Keep the two apart — `Option<T>` conflates them and `Ctrl+C` becomes a dead key.
- `Enter` is the fast path to the expected answer, except on destructive questions,
  where `Enter` answers *no* and an explicit letter is required.
- A committed value should say which keystroke acts on it. `Enter` answering "yes"
  everywhere except deletes is a trap, not a shortcut.