//! `mde-voice-egui` binary — stands the Voice surface up as a Wayland client on
//! the shared harness. All the surface's logic lives in the library
//! ([`mde_voice_egui`]); this entry point only wires it into [`run_client`].

use mde_egui::{eframe, run_client};

fn main() -> eframe::Result<()> {
    run_client(
        "org.magicmesh.Voice",
        "MCNF Voice",
        mde_voice_egui::VoiceApp::new,
    )
}
