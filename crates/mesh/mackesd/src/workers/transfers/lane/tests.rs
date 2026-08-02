use super::*;
use crate::workers::transfers::job::{Method, TransferPolicy};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::process::Command as StdCommand;
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

#[tokio::test]
async fn gated_lane_fails_every_method_honestly() {
    let lane = GatedLaneRunner;
    for m in Method::ALL {
        let job = TransferJob::new("/a", "/b", m, TransferPolicy::default());
        let outcome = lane.run(&job, ProgressSink::noop()).await;
        // The gate must never fake success (\u{a7}7).
        assert!(
            matches!(outcome, LaneOutcome::Failed { .. }),
            "the gate returned Done for {m}"
        );
        if let LaneOutcome::Failed { error } = outcome {
            assert!(error.contains(m.as_str()), "names the lane: {error}");
            assert!(
                error.contains("TRANSFERS-2..6"),
                "points at the lanes: {error}"
            );
        }
    }
}

#[test]
fn http_plan_uses_resume_and_bwlimit() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let job = TransferJob::new(
        "https://example.invalid/file.bin",
        dest.display().to_string(),
        Method::Http,
        TransferPolicy {
            bwlimit: Some("256k".into()),
            verify: false,
        },
    );
    let plan = HttpWgetLane::plan(&job).unwrap();
    assert!(plan.args.iter().any(|a| a == "--continue"));
    assert!(plan.args.iter().any(|a| a == "--limit-rate=256k"));
    assert!(plan
        .args
        .windows(2)
        .any(|w| w == ["-O", &dest.display().to_string()]));
}

#[test]
fn http_plan_rejects_non_http_and_bad_bwlimit() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let mut job = TransferJob::new(
        "file:///tmp/x",
        dest.display().to_string(),
        Method::Http,
        TransferPolicy::default(),
    );
    assert!(HttpWgetLane::plan(&job).unwrap_err().contains("http://"));
    job.source = "http://example.invalid/x".into();
    job.policy.bwlimit = Some("1m;rm".into());
    assert!(HttpWgetLane::plan(&job)
        .unwrap_err()
        .contains("invalid wget --limit-rate"));
}

#[test]
fn wget_progress_parser_uses_real_percent_tokens_only() {
    assert_eq!(
        parse_wget_progress_percent(" 65536K .......... 42%  255K 1s"),
        Some(42)
    );
    assert_eq!(parse_wget_progress_percent("no percent here"), None);
    assert_eq!(parse_wget_progress_percent("101%"), None);
}

#[test]
fn rsync_plan_uses_partial_progress_and_bwlimit() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let dest = tmp.path().join("dest");
    let job = TransferJob::new(
        source.display().to_string(),
        dest.display().to_string(),
        Method::Rsync,
        TransferPolicy {
            bwlimit: Some("512".into()),
            verify: false,
        },
    );
    let plan = RsyncLane::plan(&job).unwrap();
    assert!(plan.args.iter().any(|a| a == "--archive"));
    assert!(plan.args.iter().any(|a| a == "--partial"));
    assert!(plan.args.iter().any(|a| a == "--info=progress2"));
    assert!(plan.args.iter().any(|a| a == "--bwlimit=512"));
    assert_eq!(plan.args[plan.args.len() - 2], source.display().to_string());
    assert_eq!(plan.args[plan.args.len() - 1], dest.display().to_string());
}

#[test]
fn rsync_plan_rejects_empty_paths_and_bad_bwlimit() {
    let mut job = TransferJob::new("", "/dest", Method::Rsync, TransferPolicy::default());
    assert!(RsyncLane::plan(&job).unwrap_err().contains("source"));
    job.source = "/source".into();
    job.policy.bwlimit = Some("1m;rm".into());
    assert!(RsyncLane::plan(&job)
        .unwrap_err()
        .contains("invalid rsync --bwlimit"));
}

#[test]
fn rsync_progress_parser_reads_progress2_percentages_only() {
    assert_eq!(
        parse_rsync_progress_percent("      65,536  42%  600.00kB/s    0:00:10"),
        Some(42)
    );
    assert_eq!(
        parse_rsync_progress_percent("sending incremental file list"),
        None
    );
    assert_eq!(parse_rsync_progress_percent("101%"), None);
}

#[test]

#[test]
fn sftp_plan_accepts_put_get_and_rejects_password_urls() {
    let tmp = tempfile::tempdir().unwrap();
    let local_source = tmp.path().join("upload.bin");
    let local_dest = tmp.path().join("download.bin");
    let remote_dest = tmp.path().join("remote-upload.bin");
    let put = TransferJob::new(
        local_source.display().to_string(),
        format!("alice@example.invalid:{}", remote_dest.display()),
        Method::Sftp,
        TransferPolicy::default(),
    );
    let plan = SftpLane::plan(&put).unwrap();
    match plan.direction {
        SftpDirection::Put { local, remote } => {
            assert_eq!(local, local_source);
            assert_eq!(remote.endpoint(), "alice@example.invalid");
            assert_eq!(remote.path, remote_dest.display().to_string());
            assert_eq!(remote.port, None);
        }
        other => panic!("expected put plan, got {other:?}"),
    }

    let get = TransferJob::new(
        "sftp://bob@example.invalid:2022/tmp/remote.bin",
        local_dest.display().to_string(),
        Method::Sftp,
        TransferPolicy::default(),
    );
    let plan = SftpLane::plan(&get).unwrap();
    match plan.direction {
        SftpDirection::Get { remote, local } => {
            assert_eq!(local, local_dest);
            assert_eq!(remote.endpoint(), "bob@example.invalid");
            assert_eq!(remote.path, "/tmp/remote.bin");
            assert_eq!(remote.port, Some(2022));
        }
        other => panic!("expected get plan, got {other:?}"),
    }

    let leaked = TransferJob::new(
        "sftp://bob:secret@example.invalid/tmp/remote.bin",
        tmp.path().join("out.bin").display().to_string(),
        Method::Sftp,
        TransferPolicy::default(),
    );
    let err = SftpLane::plan(&leaked).unwrap_err();
    assert!(err.contains("key/agent credentials"));
    assert!(!err.contains("secret"));
}

#[test]
fn sftp_progress_parser_reads_percent_tokens() {
    assert_eq!(
        parse_sftp_progress_percent("remote.bin 65536 42% 1.0MB/s 00:01"),
        Some(42)
    );
    assert_eq!(parse_sftp_progress_percent("Connected to host"), None);
    assert_eq!(parse_sftp_progress_percent("101%"), None);
}

#[tokio::test]
async fn sftp_lane_put_and_get_roundtrip_against_fixture_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = fixture_sftp_bin(tmp.path());

    let local_upload = tmp.path().join("upload.bin");
    let remote_upload = tmp.path().join("remote-upload.bin");
    std::fs::write(&local_upload, b"sftp upload").unwrap();
    let put = SftpLane::plan(&TransferJob::new(
        local_upload.display().to_string(),
        format!("fixture.invalid:{}", remote_upload.display()),
        Method::Sftp,
        TransferPolicy::default(),
    ))
    .unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_seen = Arc::clone(&seen);
    let outcome = run_sftp_plan(
        &put,
        ProgressSink::new(move |pct| sink_seen.lock().unwrap().push(pct)),
        &fake,
    )
    .await;
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(std::fs::read(&remote_upload).unwrap(), b"sftp upload");
    assert!(
        seen.lock().unwrap().contains(&42),
        "fixture sftp progress should be parsed"
    );

    let remote_download = tmp.path().join("remote-download.bin");
    let local_download = tmp.path().join("download.bin");
    std::fs::write(&remote_download, b"sftp download").unwrap();
    let get = SftpLane::plan(&TransferJob::new(
        format!("sftp://fixture.invalid{}", remote_download.display()),
        local_download.display().to_string(),
        Method::Sftp,
        TransferPolicy::default(),
    ))
    .unwrap();
    let outcome = run_sftp_plan(&get, ProgressSink::noop(), &fake).await;
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(std::fs::read(&local_download).unwrap(), b"sftp download");
}

#[tokio::test]
async fn sftp_lane_put_and_get_roundtrip_against_fixture_sshd() {
    let Some(sshd_bin) = command_path("sshd") else {
        return;
    };
    let Some(sftp_bin) = command_path("sftp") else {
        return;
    };
    let Some(ssh_keygen_bin) = command_path("ssh-keygen") else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let host_key = tmp.path().join("ssh_host_ed25519_key");
    let client_key = tmp.path().join("client_ed25519");
    let authorized_keys = tmp.path().join("authorized_keys");
    let known_hosts = tmp.path().join("known_hosts");
    run_ok(
        StdCommand::new(&ssh_keygen_bin)
            .arg("-q")
            .arg("-t")
            .arg("ed25519")
            .arg("-N")
            .arg("")
            .arg("-f")
            .arg(&host_key),
    );
    run_ok(
        StdCommand::new(&ssh_keygen_bin)
            .arg("-q")
            .arg("-t")
            .arg("ed25519")
            .arg("-N")
            .arg("")
            .arg("-f")
            .arg(&client_key),
    );
    std::fs::copy(client_key.with_extension("pub"), &authorized_keys).unwrap();
    std::fs::write(&known_hosts, "").unwrap();

    let port = free_local_port();
    let config = tmp.path().join("sshd_config");
    std::fs::write(
        &config,
        format!(
            "\
Port {port}
ListenAddress 127.0.0.1
HostKey {}
AuthorizedKeysFile {}
PidFile {}
PasswordAuthentication no
KbdInteractiveAuthentication no
ChallengeResponseAuthentication no
PubkeyAuthentication yes
PermitRootLogin no
UsePAM no
StrictModes no
LogLevel ERROR
Subsystem sftp internal-sftp
",
            host_key.display(),
            authorized_keys.display(),
            tmp.path().join("sshd.pid").display()
        ),
    )
    .unwrap();
    let sshd_log = tmp.path().join("sshd.log");
    let sshd_log_file = std::fs::File::create(&sshd_log).unwrap();
    let mut sshd = StdCommand::new(&sshd_bin)
        .arg("-D")
        .arg("-e")
        .arg("-f")
        .arg(&config)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(sshd_log_file))
        .spawn()
        .unwrap();
    if !wait_for_tcp("127.0.0.1", port, Duration::from_secs(5)) {
        let exited = sshd.try_wait().unwrap();
        let log = std::fs::read_to_string(&sshd_log).unwrap_or_default();
        let _ = sshd.kill();
        let _ = sshd.wait();
        panic!(
            "fixture sshd did not accept connections on port {port}; exited={exited:?}; log={log}"
        );
    }

    let user = std::env::var("USER").unwrap_or_else(|_| "mm".into());
    let remote_upload = tmp.path().join("sshd-upload.bin");
    let local_upload = tmp.path().join("local-upload.bin");
    std::fs::write(&local_upload, b"sshd upload").unwrap();
    let put = SftpLane::plan(&TransferJob::new(
        local_upload.display().to_string(),
        format!("sftp://{user}@127.0.0.1:{port}{}", remote_upload.display()),
        Method::Sftp,
        TransferPolicy::default(),
    ))
    .unwrap();
    let outcome = run_sftp_plan_with_auth(
        &put,
        ProgressSink::noop(),
        &sftp_bin,
        Some(&client_key),
        Some(&known_hosts),
    )
    .await;
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(std::fs::read(&remote_upload).unwrap(), b"sshd upload");

    let remote_download = tmp.path().join("sshd-download.bin");
    let local_download = tmp.path().join("local-download.bin");
    std::fs::write(&remote_download, b"sshd download").unwrap();
    let get = SftpLane::plan(&TransferJob::new(
        format!(
            "sftp://{user}@127.0.0.1:{port}{}",
            remote_download.display()
        ),
        local_download.display().to_string(),
        Method::Sftp,
        TransferPolicy::default(),
    ))
    .unwrap();
    let outcome = run_sftp_plan_with_auth(
        &get,
        ProgressSink::noop(),
        &sftp_bin,
        Some(&client_key),
        Some(&known_hosts),
    )
    .await;
    let _ = sshd.kill();
    let _ = sshd.wait();
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(std::fs::read(&local_download).unwrap(), b"sshd download");
}

#[test]
fn music_plan_lands_source_basename_in_library_dir() {
    let job = TransferJob::new(
        "/music/input/song.wav",
        "/mnt/mesh-storage/music-library",
        Method::Music,
        TransferPolicy::default(),
    );
    let plan = MusicLibraryLane::plan(&job).unwrap();
    assert_eq!(plan.source, PathBuf::from("/music/input/song.wav"));
    assert_eq!(
        plan.dest,
        PathBuf::from("/mnt/mesh-storage/music-library/song.wav")
    );
}

#[test]
fn music_plan_rejects_remote_sources() {
    let job = TransferJob::new(
        "host:/srv/song.wav",
        "/mnt/mesh-storage/music-library",
        Method::Music,
        TransferPolicy::default(),
    );
    assert!(MusicLibraryLane::plan(&job)
        .unwrap_err()
        .contains("local filesystem source"));
}

#[test]
fn node_plan_stages_peer_destinations_under_mesh_share() {
    let mesh_root = PathBuf::from("/mnt/mesh-storage");
    let job = TransferJob::new(
        "/home/user/file.iso",
        "node:Oak-01",
        Method::Node,
        TransferPolicy::default(),
    );
    let plan = NodeLane::plan_with_root(&job, mesh_root).unwrap();
    assert_eq!(plan.source, PathBuf::from("/home/user/file.iso"));
    assert_eq!(
        plan.dest,
        PathBuf::from("/mnt/mesh-storage/.transfers/node/Oak-01/file.iso")
    );
    assert_eq!(plan.target_peer.as_deref(), Some("Oak-01"));
}

#[test]
fn node_plan_rejects_remote_sources_and_ad_hoc_destinations() {
    let root = PathBuf::from("/mnt/mesh-storage");
    let mut job = TransferJob::new(
        "host:/srv/file.iso",
        "node:oak",
        Method::Node,
        TransferPolicy::default(),
    );
    assert!(NodeLane::plan_with_root(&job, root.clone())
        .unwrap_err()
        .contains("local filesystem source"));
    job.source = "/home/user/file.iso".into();
    job.dest = "sftp.example.com:/drop".into();
    assert!(NodeLane::plan_with_root(&job, root)
        .unwrap_err()
        .contains("node:<peer>"));
}

#[tokio::test]
async fn http_lane_resumes_partial_download_against_fixture_server() {
    if StdCommand::new("wget").arg("--version").output().is_err() {
        return;
    }
    let body = (0..131_072).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let partial_len = 4096usize;
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("payload.bin");
    std::fs::write(&dest, &body[..partial_len]).unwrap();

    let (url, range_rx, join) = fixture_http_server(body.clone());
    let job = TransferJob::new(
        url,
        dest.display().to_string(),
        Method::Http,
        TransferPolicy {
            bwlimit: Some("1m".into()),
            verify: false,
        },
    );
    let outcome = HttpWgetLane.run(&job, ProgressSink::noop()).await;
    join.join().unwrap();
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    let range = range_rx.recv().unwrap_or_default();
    assert!(
        range.contains(&format!("bytes={partial_len}-")),
        "wget must resume from the partial file, got Range header `{range}`"
    );
}

#[tokio::test]
async fn http_lane_bwlimit_caps_fixture_download_throughput() {
    if StdCommand::new("wget").arg("--version").output().is_err() {
        return;
    }
    let body = (0..32_768).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("limited.bin");
    let (url, _range_rx, join) = fixture_http_server(body.clone());
    let job = TransferJob::new(
        url,
        dest.display().to_string(),
        Method::Http,
        TransferPolicy {
            bwlimit: Some("8k".into()),
            verify: false,
        },
    );
    let start = Instant::now();
    let outcome = HttpWgetLane.run(&job, ProgressSink::noop()).await;
    let elapsed = start.elapsed();
    join.join().unwrap();
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(
            elapsed >= Duration::from_secs(2),
            "32 KiB at --limit-rate=8k should not complete as an uncapped local transfer; elapsed={elapsed:?}"
        );
}

#[tokio::test]
async fn rsync_lane_round_trips_local_fixture_and_reports_progress() {
    if StdCommand::new("rsync").arg("--version").output().is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let source_dir = tmp.path().join("source");
    let dest_dir = tmp.path().join("dest");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(source_dir.join("payload.bin"), vec![7u8; 64 * 1024]).unwrap();
    let job = TransferJob::new(
        format!("{}/", source_dir.display()),
        dest_dir.display().to_string(),
        Method::Rsync,
        TransferPolicy {
            bwlimit: Some("2048".into()),
            verify: false,
        },
    );
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_seen = Arc::clone(&seen);
    let outcome = RsyncLane
        .run(
            &job,
            ProgressSink::new(move |pct| sink_seen.lock().unwrap().push(pct)),
        )
        .await;
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(
        std::fs::read(dest_dir.join("payload.bin")).unwrap(),
        vec![7u8; 64 * 1024]
    );
    assert!(
        seen.lock().unwrap().iter().any(|pct| *pct > 0),
        "rsync --info=progress2 should publish real progress"
    );
}

#[tokio::test]
async fn music_lane_copies_track_into_shared_library() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("track.flac");
    let library = tmp.path().join("mesh-share").join("music-library");
    std::fs::write(&source, b"fake flac bytes").unwrap();
    let job = TransferJob::new(
        source.display().to_string(),
        library.display().to_string(),
        Method::Music,
        TransferPolicy::default(),
    );
    let outcome = MusicLibraryLane.run(&job, ProgressSink::noop()).await;
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(
        std::fs::read(library.join("track.flac")).unwrap(),
        b"fake flac bytes"
    );
}

#[tokio::test]
async fn node_lane_stages_file_in_mesh_share_for_target_peer() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("payload.bin");
    let mesh_root = tmp.path().join("mesh-share");
    std::fs::write(&source, b"node payload").unwrap();
    let job = TransferJob::new(
        source.display().to_string(),
        "node:oak",
        Method::Node,
        TransferPolicy::default(),
    );
    let plan = NodeLane::plan_with_root(&job, mesh_root.clone()).unwrap();
    let outcome = copy_node_plan(plan, ProgressSink::noop()).await;
    assert_eq!(outcome, LaneOutcome::Done);
    assert_eq!(
        std::fs::read(mesh_root.join(".transfers/node/oak/payload.bin")).unwrap(),
        b"node payload"
    );
}

#[tokio::test]
async fn surface_complete_gate_exercises_every_lane_against_fixtures() {
    if StdCommand::new("wget").arg("--version").output().is_err() {
        eprintln!("skipping surface-complete fixture: wget is not installed");
        return;
    }
    if StdCommand::new("rsync").arg("--version").output().is_err() {
        eprintln!("skipping surface-complete fixture: rsync is not installed");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let mut completed = Vec::new();

    // HTTP covers resume: the destination starts with a partial file and the
    // fixture asserts wget requests the remaining byte range.
    let http_body = (0..65_536).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let http_dest = tmp.path().join("http.bin");
    let partial_len = 2048usize;
    std::fs::write(&http_dest, &http_body[..partial_len]).unwrap();
    let (http_url, range_rx, http_join) = fixture_http_server(http_body.clone());
    let http_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let http_sink = Arc::clone(&http_seen);
    let http_job = TransferJob::new(
        http_url,
        http_dest.display().to_string(),
        Method::Http,
        TransferPolicy::default(),
    );
    assert_eq!(
        HttpWgetLane
            .run(
                &http_job,
                ProgressSink::new(move |pct| http_sink.lock().unwrap().push(pct)),
            )
            .await,
        LaneOutcome::Done
    );
    http_join.join().unwrap();
    assert_eq!(std::fs::read(&http_dest).unwrap(), http_body);
    assert!(
        range_rx
            .recv()
            .unwrap_or_default()
            .contains(&format!("bytes={partial_len}-")),
        "HTTP lane did not resume from the partial file"
    );
    completed.push(Method::Http);

    // SFTP uses the same batch-mode execution path, with a fixture binary that
    // performs put/get locally and emits a real percent token.
    let sftp_bin = fixture_sftp_bin(tmp.path());
    let sftp_local = tmp.path().join("sftp-local.bin");
    let sftp_remote = tmp.path().join("sftp-remote.bin");
    std::fs::write(&sftp_local, b"sftp surface bytes").unwrap();
    let sftp_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sftp_sink = Arc::clone(&sftp_seen);
    let sftp_plan = SftpLane::plan(&TransferJob::new(
        sftp_local.display().to_string(),
        format!("fixture.invalid:{}", sftp_remote.display()),
        Method::Sftp,
        TransferPolicy::default(),
    ))
    .unwrap();
    assert_eq!(
        run_sftp_plan(
            &sftp_plan,
            ProgressSink::new(move |pct| sftp_sink.lock().unwrap().push(pct)),
            &sftp_bin,
        )
        .await,
        LaneOutcome::Done
    );
    assert_eq!(std::fs::read(&sftp_remote).unwrap(), b"sftp surface bytes");
    assert!(sftp_seen.lock().unwrap().contains(&42));
    completed.push(Method::Sftp);

    // rsync covers the delta/mirror lane and native progress parser.
    let rsync_source = tmp.path().join("rsync-source");
    let rsync_dest = tmp.path().join("rsync-dest");
    std::fs::create_dir_all(&rsync_source).unwrap();
    std::fs::write(rsync_source.join("mirror.txt"), vec![9u8; 64 * 1024]).unwrap();
    let rsync_seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let rsync_sink = Arc::clone(&rsync_seen);
    let rsync_job = TransferJob::new(
        format!("{}/", rsync_source.display()),
        rsync_dest.display().to_string(),
        Method::Rsync,
        TransferPolicy::default(),
    );
    assert_eq!(
        RsyncLane
            .run(
                &rsync_job,
                ProgressSink::new(move |pct| rsync_sink.lock().unwrap().push(pct)),
            )
            .await,
        LaneOutcome::Done
    );
    assert_eq!(
        std::fs::read(rsync_dest.join("mirror.txt")).unwrap(),
        vec![9u8; 64 * 1024]
    );
    assert!(rsync_seen.lock().unwrap().iter().any(|pct| *pct > 0));
    completed.push(Method::Rsync);

    // Node stages through the shared mesh root.
    let node_source = tmp.path().join("node.bin");
    let mesh_root = tmp.path().join("mesh-share");
    std::fs::write(&node_source, b"node surface bytes").unwrap();
    let node_job = TransferJob::new(
        node_source.display().to_string(),
        "node:oak",
        Method::Node,
        TransferPolicy::default(),
    );
    let node_plan = NodeLane::plan_with_root(&node_job, mesh_root.clone()).unwrap();
    assert_eq!(
        copy_node_plan(node_plan, ProgressSink::noop()).await,
        LaneOutcome::Done
    );
    assert_eq!(
        std::fs::read(mesh_root.join(".transfers/node/oak/node.bin")).unwrap(),
        b"node surface bytes"
    );
    completed.push(Method::Node);

    // Music lands in the shared library path.
    let music_source = tmp.path().join("track.flac");
    let music_library = tmp.path().join("music-library");
    std::fs::write(&music_source, b"music surface bytes").unwrap();
    let music_job = TransferJob::new(
        music_source.display().to_string(),
        music_library.display().to_string(),
        Method::Music,
        TransferPolicy::default(),
    );
    assert_eq!(
        MusicLibraryLane.run(&music_job, ProgressSink::noop()).await,
        LaneOutcome::Done
    );
    assert_eq!(
        std::fs::read(music_library.join("track.flac")).unwrap(),
        b"music surface bytes"
    );
    completed.push(Method::Music);

    assert_eq!(completed.len(), Method::ALL.len());
    for method in Method::ALL {
        assert!(completed.contains(&method), "missing {method} from gate");
    }
}

fn fixture_http_server(body: Vec<u8>) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let join = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        let range = request
            .lines()
            .find_map(|line| line.strip_prefix("Range:"))
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let start = range
            .strip_prefix("bytes=")
            .and_then(|r| r.split_once('-').map(|(n, _)| n))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0)
            .min(body.len());
        let _ = tx.send(range);
        if start > 0 {
            let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                    body.len() - start,
                    start,
                    body.len() - 1,
                    body.len()
                );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body[start..]).unwrap();
        } else {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    (format!("http://{addr}/payload.bin"), rx, join)
}

fn fixture_http_routes_server(
    routes: Vec<(&'static str, Vec<u8>)>,
) -> (String, mpsc::Receiver<Vec<String>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let routes = routes.into_iter().collect::<BTreeMap<_, _>>();
    let expected = routes.len();
    let (tx, rx) = mpsc::channel();
    let join = thread::spawn(move || {
        let mut seen = Vec::new();
        for _ in 0..expected {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/")
                .to_string();
            seen.push(path.clone());
            if let Some(body) = routes.get(path.as_str()) {
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            } else {
                let body = b"not found";
                let head = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        }
        let _ = tx.send(seen);
    });
    (format!("http://{addr}"), rx, join)
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut data = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if data.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

fn fixture_sftp_bin(root: &Path) -> PathBuf {
    let bin = root.join("fixture-sftp");
    std::fs::write(
        &bin,
        r#"#!/bin/sh
batch=
while [ "$#" -gt 0 ]; do
  case "$1" in
    -b) batch="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [ -z "$batch" ]; then
  echo "missing batch file" >&2
  exit 7
fi
eval "set -- $(cat "$batch")"
cmd="$1"
src="$2"
dst="$3"
echo "payload.bin 65536 42% 1.0MB/s 00:01" >&2
case "$cmd" in
  put|get) cp "$src" "$dst" ;;
  *) echo "unknown command: $cmd" >&2; exit 9 ;;
esac
"#,
    )
    .unwrap();
    let mut perms = std::fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin, perms).unwrap();
    bin
}

fn run_ok(cmd: &mut StdCommand) {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "command failed: status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn command_path(name: &str) -> Option<PathBuf> {
    let output = StdCommand::new("sh")
        .arg("-c")
        .arg(format!("command -v {name}"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    Some(PathBuf::from(path.trim()))
}

fn free_local_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn wait_for_tcp(host: &str, port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect((host, port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}
