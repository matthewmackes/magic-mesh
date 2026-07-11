//! `Voice` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `voice` subcommand.
#[allow(unreachable_code)]
pub fn run(sub: VoiceCmd) -> anyhow::Result<()> {
    {
        // VV-1 / VV-1.5 / VV-2 (v4.1.0) — voice stack operator
        // surface. `render-config` is invoked by both
        // `kamailio-mde.service` and `rtpengine-mde.service` as
        // their ExecStartPre hook; the voice_config worker
        // writes the JSON input file when policy changes and
        // triggers `systemctl reload` to re-run this command.
        match sub {
            VoiceCmd::RenderConfig {
                kamailio_dir,
                rtpengine_dir,
                desired_json,
                boot_default,
                dry_run,
            } => {
                let desired = load_voice_desired(&desired_json, boot_default, &default_node_id())?;
                let set = mde_voice_config::generate(&desired);
                let kamailio_files = [
                    ("kamailio.cfg", &set.kamailio_cfg),
                    ("dispatcher.list", &set.dispatcher_list),
                    ("uacreg.list", &set.uacreg_list),
                ];
                let rtpengine_files = [("rtpengine.conf", &set.rtpengine_conf)];
                if dry_run {
                    for (name, body) in kamailio_files {
                        println!(
                            "# ---- {} (would write under {}) ----",
                            name,
                            kamailio_dir.display()
                        );
                        print!("{body}");
                    }
                    for (name, body) in rtpengine_files {
                        println!(
                            "# ---- {} (would write under {}) ----",
                            name,
                            rtpengine_dir.display()
                        );
                        print!("{body}");
                    }
                } else {
                    write_voice_config_files(&kamailio_dir, &kamailio_files)?;
                    write_voice_config_files(&rtpengine_dir, &rtpengine_files)?;
                    println!(
                        "voice render-config: wrote {} files under {} + {} under {}",
                        kamailio_files.len(),
                        kamailio_dir.display(),
                        rtpengine_files.len(),
                        rtpengine_dir.display(),
                    );
                }
            }
        }
    }
    Ok(())
}
