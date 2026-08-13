use std::io::{self, Write};
use std::path::PathBuf;

use mcnf_cuttlefish_guest::{
    handle_agent_request, read_json_bounded, AdbBackend, AgentConfig, BUILD_SOURCE_REVISION,
};

fn value(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.as_slice() == ["--build-identity"] {
        println!("{BUILD_SOURCE_REVISION}");
        return Ok(());
    }
    if !arguments.iter().any(|argument| argument == "--stdio") {
        return Err("--stdio is required".into());
    }
    let adb = PathBuf::from(value(&arguments, "--adb").ok_or("--adb is required")?);
    let config = AgentConfig {
        adb: adb.clone(),
        mesh_host: value(&arguments, "--mesh-host").ok_or("--mesh-host is required")?,
        webrtc_port: value(&arguments, "--webrtc-port")
            .ok_or("--webrtc-port is required")?
            .parse()?,
        session_id: value(&arguments, "--session-id").ok_or("--session-id is required")?,
    };
    let request = read_json_bounded(&mut io::stdin().lock())?;
    let response = handle_agent_request(&request, &AdbBackend { executable: &adb }, &config)?;
    let body = serde_json::to_vec(&response)?;
    io::stdout().lock().write_all(&body)?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("mcnf-cuttlefish-vdi-agent: {error}");
        std::process::exit(1);
    }
}
