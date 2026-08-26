//! "Plugins" top-level submenu — install/update/disable/remove the threads
//! provider. Thin UI over [`crate::plugins`] logic.

use crate::cli::interactive::{clear_screen, select_menu};
use crate::plugins::{self, PluginState, THREADSTRACTOR_PIN};

pub(super) fn menu() {
    loop {
        clear_screen();
        let state = plugins::threads_state();
        let status = match &state {
            PluginState::NotInstalled => "not installed".to_string(),
            PluginState::Disabled => "disabled (files kept)".to_string(),
            PluginState::Enabled(v) => format!("enabled ({v})"),
        };
        let mut options: Vec<String> = Vec::new();
        match state {
            PluginState::NotInstalled => options.push(format!(
                "Enable threads — install threadstractormf {THREADSTRACTOR_PIN} (~150MB Chromium download)"
            )),
            PluginState::Disabled => {
                options.push("Enable threads".to_string());
                options.push("Remove threads plugin (deletes all files)".to_string());
            }
            PluginState::Enabled(_) => {
                options.push(format!("Update / reinstall at pin {THREADSTRACTOR_PIN}"));
                options.push("Disable threads (hide, keep files)".to_string());
                options.push("Remove threads plugin (deletes all files)".to_string());
            }
        }
        options.push("Back".to_string());

        println!("Plugins — threads: {status}");
        let choice = match select_menu("Plugin action:", options).prompt() {
            Ok(c) => c,
            Err(_) => return,
        };
        match choice.as_str() {
            "Back" => return,
            c if c.starts_with("Enable threads") || c.starts_with("Update / reinstall") => {
                if let Err(e) = plugins::install() {
                    eprintln!("✖ install failed: {e}");
                }
                pause();
            }
            "Disable threads (hide, keep files)" => match plugins::set_disabled(true) {
                Ok(()) => println!("✔ threads plugin disabled — options hidden, files kept"),
                Err(e) => eprintln!("✖ failed: {e}"),
            },
            "Remove threads plugin (deletes all files)" => match plugins::remove() {
                Ok(()) => println!("✔ threads plugin removed completely"),
                Err(e) => eprintln!("✖ remove failed: {e}"),
            },
            _ => {}
        }
    }
}

fn pause() {
    std::thread::sleep(std::time::Duration::from_millis(1500));
}
