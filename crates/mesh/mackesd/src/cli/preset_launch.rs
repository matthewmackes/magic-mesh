//! `PresetLaunch` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `preset-launch` subcommand.
#[allow(unreachable_code)]
pub fn run(tag: String) -> anyhow::Result<()> {
    {
        // Portal-18.d (v6.0 R12, 2026-05-27) — preset launch-
        // bundle expansion. Loads the tag store, finds the
        // named preset, fires `swaymsg exec <cmd>` for each
        // entry in `launch_bundle`. Prints a one-line summary;
        // non-zero exit when any exec fails.
        let store = mackes_mesh_types::TagStore::load_default()
            .with_context(|| "loading tag store for preset-launch")?;
        let Some(tag_entry) = store.find_by_name(&tag) else {
            eprintln!("error: tag '{tag}' not found in tag store");
            std::process::exit(1);
        };
        let launch_bundle = match &tag_entry.flavor {
            mackes_mesh_types::TagFlavor::Preset { launch_bundle } => launch_bundle.clone(),
            other => {
                eprintln!("error: tag '{tag}' is not a preset (flavor: {:?})", other);
                std::process::exit(1);
            }
        };
        if launch_bundle.is_empty() {
            eprintln!("error: tag '{tag}' has an empty launch_bundle");
            std::process::exit(1);
        }
        let total = launch_bundle.len();
        let mut launched = 0usize;
        for cmd_str in &launch_bundle {
            let escaped = cmd_str.replace('\\', "\\\\").replace('"', "\\\"");
            let swayipc_cmd = format!("exec \"{escaped}\"");
            let status = std::process::Command::new("swaymsg")
                .arg(&swayipc_cmd)
                .status();
            match status {
                Ok(s) if s.success() => launched += 1,
                Ok(s) => {
                    eprintln!("warn: swaymsg exit {s} for '{cmd_str}'");
                }
                Err(e) => {
                    eprintln!("warn: swaymsg spawn failed for '{cmd_str}': {e}");
                }
            }
        }
        println!("launched {launched}/{total} from preset '{tag}'");
        if launched != total {
            std::process::exit(1);
        }
    }
    Ok(())
}
