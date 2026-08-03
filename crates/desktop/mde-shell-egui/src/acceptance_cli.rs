//! Hidden, headless five-seat acceptance entry points.
//!
//! These verbs exist so release automation can drive the same signed
//! Communications and native clipboard paths as the visible root DRM shell.
//! They neither mint in `mackesd` nor bypass the session clipboard toggle.

use std::io::Read as _;

const MAX_INPUT_BYTES: usize = 64 * 1024;

pub(crate) fn maybe_run() -> Option<Result<(), String>> {
    let verb = std::env::args().nth(1)?;
    match verb.as_str() {
        "--acceptance-collab-command" => Some(run_collab()),
        "--acceptance-clipboard" => Some(run_clipboard()),
        "--acceptance-read-clipboard" => Some(read_clipboard()),
        "--acceptance-link-file" => Some(run_link_file()),
        _ => None,
    }
}

fn read_stdin() -> Result<String, String> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_INPUT_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("acceptance input read failed: {error}"))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "acceptance input exceeds the {MAX_INPUT_BYTES}-byte limit"
        ));
    }
    String::from_utf8(bytes).map_err(|_| "acceptance input is not UTF-8".to_owned())
}

fn run_collab() -> Result<(), String> {
    let body = read_stdin()?;
    let command: mde_collab_types::CollabCommand =
        serde_json::from_str(&body).map_err(|error| format!("invalid CollabCommand: {error}"))?;
    let command_verb = command.verb();
    crate::communications::publish_acceptance_command(&command)?;
    println!("{{\"ok\":true,\"lane\":\"collab\",\"verb\":\"{command_verb}\"}}");
    Ok(())
}

fn run_clipboard() -> Result<(), String> {
    let text = read_stdin()?;
    let published = crate::communications::publish_acceptance_clipboard(&text)?;
    println!("{{\"ok\":true,\"lane\":\"clipboard\",\"published\":{published}}}");
    Ok(())
}

fn read_clipboard() -> Result<(), String> {
    let (sha256, len) = crate::communications::read_acceptance_clipboard()?
        .ok_or_else(|| "native clipboard provider has no materialized value".to_owned())?;
    println!("{{\"ok\":true,\"lane\":\"clipboard-read\",\"sha256\":\"{sha256}\",\"len\":{len}}}");
    Ok(())
}

fn run_link_file() -> Result<(), String> {
    let space = std::env::args()
        .nth(2)
        .ok_or_else(|| "--acceptance-link-file requires a space UUID".to_owned())?
        .parse::<mde_collab_types::SpaceId>()
        .map_err(|_| "acceptance file space UUID is invalid".to_owned())?;
    let path = read_stdin()?;
    let path = std::path::Path::new(path.trim());
    let mut surface = mde_collab_egui::CommunicationsSurface::new();
    let mut sink = mde_collab_egui::CommandSink::new();
    surface
        .link_file_from_path(&mut sink, space, path)
        .map_err(|error| format!("acceptance file link failed: {error}"))?;
    let link = sink
        .drain()
        .into_iter()
        .next()
        .ok_or_else(|| "acceptance file link emitted no command".to_owned())?;
    let file = match &link {
        mde_collab_types::CollabCommand::LinkFile { file, .. } => *file,
        _ => return Err("acceptance file link emitted the wrong command".to_owned()),
    };
    crate::communications::publish_acceptance_command(&link)?;
    surface.start_transfer_to_members(&mut sink, space, file);
    let start = sink
        .drain()
        .into_iter()
        .next()
        .ok_or_else(|| "acceptance transfer emitted no command".to_owned())?;
    crate::communications::publish_acceptance_command(&start)?;
    println!("{{\"ok\":true,\"lane\":\"file\",\"file\":\"{file}\",\"space\":\"{space}\"}}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acceptance_input_cap_matches_the_privileged_bus_cap() {
        assert_eq!(MAX_INPUT_BYTES, 64 * 1024);
    }
}
