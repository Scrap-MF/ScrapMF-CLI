//! Unified decorated menu API — every selection goes through `Browser`
//! with the fixed chrome `╭ SCRAPMF vX.Y.Z ─ {context} ─╮`.
//! Adding a new network/site/content kind requires no UI change: just
//! extend `sites::registry` and the menu inherits the chrome.

use crate::cli::interactive::browser::{Browser, Mode, Outcome};

/// Fixed chrome prefix — always SCRAPMF version.
fn chrome_title(context: &str) -> String {
    let ver = env!("CARGO_PKG_VERSION");
    let base = format!("SCRAPMF v{ver}");
    if context.is_empty() || context == base {
        base
    } else if context.starts_with("SCRAPMF") {
        context.to_string()
    } else {
        format!("{base} ─ {context}")
    }
}

/// Single-select decorated menu (Browser Single). `context` is shown in the
/// border title, e.g. "Download content" → `╭ SCRAPMF v1.7.0 ─ Download content ─╮`.
/// Returns picked index or None on cancel.
pub fn pick_single(context: &str, options: Vec<(String, Vec<String>)>) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).mode(Mode::Single);
    for (label, details) in options {
        b = b.entry(label, details);
    }
    match b.run() {
        Outcome::Picked(i) => Some(i),
        _ => None,
    }
}

/// Multi-select decorated menu (Browser Multi). Returns picked indices.
pub fn pick_multi(
    context: &str,
    options: Vec<(String, Vec<String>)>,
    prechecked: &[usize],
) -> Option<Vec<usize>> {
    if options.is_empty() {
        return None;
    }
    let title = chrome_title(context);
    let mut b = Browser::new(title).mode(Mode::Multi).checked(prechecked);
    for (label, details) in options {
        b = b.entry(label, details);
    }
    match b.run() {
        Outcome::Toggled(v) => Some(v),
        _ => None,
    }
}

/// Convenience for simple string options without details pane.
pub fn pick_single_labels(context: &str, labels: Vec<String>) -> Option<usize> {
    let opts = labels.into_iter().map(|l| (l, Vec::new())).collect();
    pick_single(context, opts)
}

#[cfg(test)]
mod tests {
    use super::chrome_title;

    #[test]
    fn chrome_title_formats() {
        let v = env!("CARGO_PKG_VERSION");
        assert_eq!(chrome_title(""), format!("SCRAPMF v{v}"));
        assert_eq!(
            chrome_title("Download content"),
            format!("SCRAPMF v{v} ─ Download content")
        );
        assert_eq!(
            chrome_title(&format!("SCRAPMF v{v}")),
            format!("SCRAPMF v{v}")
        );
    }
}
