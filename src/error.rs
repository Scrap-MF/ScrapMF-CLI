use std::path::PathBuf;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ScrapmfError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error(
        "invalid URL: {url}\n  help: use a valid http(s) URL, e.g. https://example.com/gallery/123\n  note: valid schemes are http/https and length ≤2048"
    )]
    InvalidUrl { url: String },

    #[error(
        "backend '{name}' not found in $PATH\n  help: install via 'pipx install {name}' or 'pacman -S {name}'\n  note: custom path can be set in ~/.config/scrapmf/config.toml"
    )]
    BackendNotFound { name: String },

    #[error(
        "backend '{name}' failed with exit code {code:?}\n  stderr: {stderr}\n  help: check {name} logs with --verbose\n  note: RUST_LOG=debug for verbose tracing"
    )]
    BackendFailed {
        name: String,
        code: Option<i32>,
        stderr: String,
    },

    #[error(
        "config parse error at {path}: {source}\n  help: check TOML syntax\n  note: see https://toml.io/en/v1.0.0"
    )]
    ConfigParse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("aborted by user")]
    Aborted,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::ScrapmfError;

    #[test]
    fn backend_not_found_display_contains_help() {
        let err = ScrapmfError::BackendNotFound {
            name: "gallery-dl".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("help:"));
        assert!(s.contains("gallery-dl"));
    }

    #[test]
    fn backend_failed_truncates() {
        let stderr = (0..30)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = ScrapmfError::BackendFailed {
            name: "gallery-dl".to_string(),
            code: Some(1),
            stderr: stderr.clone(),
        };
        let s = format!("{err}");
        assert!(s.contains("help:"));
        assert!(s.contains("note:"));
    }

    #[test]
    fn invalid_url_help() {
        let err = ScrapmfError::InvalidUrl {
            url: "javascript:alert(1)".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("help:"));
        assert!(s.contains("note:"));
    }
}
