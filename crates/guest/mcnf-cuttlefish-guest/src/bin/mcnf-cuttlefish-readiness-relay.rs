use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

use mcnf_cuttlefish_guest::{
    invoke_agent, read_framed, validate_peer, write_framed, BUILD_SOURCE_REVISION,
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
    let socket = PathBuf::from(value(&arguments, "--socket").ok_or("--socket is required")?);
    let agent = PathBuf::from(value(&arguments, "--agent").ok_or("--agent is required")?);
    let parent = socket.parent().ok_or("socket parent is required")?;
    let parent_meta = fs::symlink_metadata(parent)?;
    if fs::canonicalize(parent)? != parent
        || !parent_meta.is_dir()
        || parent_meta.permissions().mode() & 0o022 != 0
        || parent_meta.uid() != rustix::process::geteuid().as_raw()
        || socket.exists()
        || socket.is_symlink()
    {
        return Err("socket boundary is unsafe".into());
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let agent_arguments = ["--adb", "--mesh-host", "--webrtc-port", "--session-id"]
        .into_iter()
        .flat_map(|name| [name.to_owned(), value(&arguments, name).unwrap_or_default()])
        .collect::<Vec<_>>();
    if agent_arguments.iter().any(String::is_empty) {
        return Err("complete agent configuration is required".into());
    }
    let expected_uid = rustix::process::getuid().as_raw();
    loop {
        let (mut stream, _) = listener.accept()?;
        if validate_peer(&stream, expected_uid).is_err() {
            continue;
        }
        let Ok(request) = read_framed(&mut stream) else {
            continue;
        };
        let Ok(response) = invoke_agent(&agent, &request, &agent_arguments) else {
            continue;
        };
        write_framed(&mut stream, &response)?;
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("mcnf-cuttlefish-readiness-relay: {error}");
        std::process::exit(1);
    }
}
