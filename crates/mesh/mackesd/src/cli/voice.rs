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

/// Resolve the stable node id from `$MACKESD_NODE_ID` then
/// `$HOSTNAME` then the `hostname` syscall, falling back to
/// `peer:unknown` so the audit-log column is never empty.
/// VV-2 helper — load `VoiceDesired` from the operator's JSON
/// override file at `desired_json`, falling back to
/// `boot_default(node_id)` when the file is absent or `force_boot`
/// is set.
///
/// `force_boot=true` is the explicit `--boot-default` CLI flag —
/// useful for testing the bootstrap path without removing the
/// override file. A missing override file is the steady-state on a
/// fresh peer (no voice policies have been approved yet), so it's
/// a silent fall-through rather than a hard error. Parse errors
/// on a present file *are* hard errors — the operator's
/// hand-edited / worker-written file is bad and we should not
/// silently fall back to defaults that hide the bug.
fn load_voice_desired(
    desired_json: &std::path::Path,
    force_boot: bool,
    node_id: &str,
) -> anyhow::Result<mde_voice_config::VoiceDesired> {
    if force_boot {
        return Ok(mde_voice_config::VoiceDesired::boot_default(node_id));
    }
    match std::fs::read_to_string(desired_json) {
        Ok(body) => serde_json::from_str(&body).map_err(|e| {
            anyhow::anyhow!("voice render-config: parse {}: {e}", desired_json.display())
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(mde_voice_config::VoiceDesired::boot_default(node_id))
        }
        Err(e) => Err(anyhow::anyhow!(
            "voice render-config: read {}: {e}",
            desired_json.display()
        )),
    }
}

/// VV-1 helper — atomic write-and-rename of the generated voice
/// configs. The directory is `mkdir -p`'d; each file is written
/// to a hidden `.tmp` sibling and renamed into place so a
/// partial render never leaves Kamailio / `RTPengine` reading a
/// half-written file.
fn write_voice_config_files(
    out_dir: &std::path::Path,
    files: &[(&str, &String)],
) -> anyhow::Result<()> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| anyhow::anyhow!("voice render-config: mkdir {}: {e}", out_dir.display()))?;
    for (name, body) in files {
        let final_path = out_dir.join(name);
        let tmp_path = out_dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp_path, body.as_bytes()).map_err(|e| {
            anyhow::anyhow!("voice render-config: write {}: {e}", tmp_path.display())
        })?;
        std::fs::rename(&tmp_path, &final_path).map_err(|e| {
            anyhow::anyhow!(
                "voice render-config: rename {} → {}: {e}",
                tmp_path.display(),
                final_path.display()
            )
        })?;
    }
    Ok(())
}
