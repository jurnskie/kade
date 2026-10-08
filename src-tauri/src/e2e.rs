//! End-to-end test against a real SSH server. Ignored by default; run with
//! `KADE_E2E_PORT=2222 KADE_E2E_KEY=/path/key cargo test -- --ignored e2e`
//! against an sshd on 127.0.0.1 that accepts that key for the current user.
//! "Remote" paths are then on this machine, which makes them easy to check.

use std::path::Path;
use std::sync::Arc;

use crate::backup::{self, Op, Recorder, Side};
use crate::error::AppError;
use crate::profiles::{Auth, Protocol, ServerProfile};
use crate::ssh::{self, Session};
use crate::transfer::{self, Conflict, Direction, Emit, JobState, Progress, Transfers};

async fn run(session: &Arc<Session>, direction: Direction, source: &Path, dest: &Path, policy: Conflict) -> Progress {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let emit: Emit = Arc::new(move |p: &Progress| {
        let _ = tx.send(p.clone());
    });
    transfer::start(
        emit,
        Arc::new(Transfers::default()),
        session.clone(),
        "e2e".into(),
        transfer::JobSpec { direction, sources: vec![source.to_string_lossy().into()], dest_dir: dest.to_string_lossy().into(), policy },
    );
    while let Some(p) = rx.recv().await {
        if matches!(p.state, JobState::Done | JobState::Failed | JobState::Cancelled) {
            return p;
        }
    }
    panic!("transfer ended without a final state");
}

fn read(p: impl AsRef<Path>) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[tokio::test]
#[ignore]
async fn e2e_transfer_backup_restore() {
    // Same filesystem as ~/.cache (backups are renamed) and /tmp, which is
    // often a separate tmpfs (backups fall back to a local download).
    let home_base = dirs::home_dir().unwrap().join(".cache/kade-e2e");
    for base in [home_base.clone(), std::env::temp_dir()] {
        scenario(&base).await;
    }
    let _ = std::fs::remove_dir(home_base);
    let leftovers: Vec<_> = std::fs::read_dir(dirs::home_dir().unwrap().join(".cache/kade/backups"))
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "server-side backups left behind: {leftovers:?}");
}

async fn scenario(base: &Path) {
    let port: u16 = std::env::var("KADE_E2E_PORT").expect("KADE_E2E_PORT").parse().unwrap();
    let key = std::env::var("KADE_E2E_KEY").expect("KADE_E2E_KEY");
    let tmp = base.join(format!("kade-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("KADE_DATA_HOME", tmp.join("data"));
    std::env::set_var("KADE_KNOWN_HOSTS", tmp.join("known_hosts"));

    let profile = ServerProfile {
        id: "e2e".into(),
        name: "e2e".into(),
        protocol: Protocol::Sftp,
        host: "127.0.0.1".into(),
        port,
        user: std::env::var("USER").unwrap(),
        group: String::new(),
        auth: Auth::KeyFile { path: key },
        remote_path: None,
        local_path: None,
        workspace: String::new(),
        tunnels: Vec::new(),
        updated_at: 0,
    };

    // Unknown host first, then trust the fingerprint we were shown.
    let fp = match ssh::connect(&profile, None, None).await {
        Err(AppError::HostKeyUnknown { fingerprint, .. }) => fingerprint,
        Err(e) => panic!("expected unknown host key, got {e}"),
        Ok(_) => panic!("expected unknown host key, got a connection"),
    };
    let (session, _) = ssh::connect(&profile, None, Some(fp)).await.unwrap();
    let session = Arc::new(session);
    // Now known: a plain connect must succeed.
    ssh::connect(&profile, None, None).await.unwrap();

    let site = tmp.join("src/site");
    std::fs::create_dir_all(site.join("sub")).unwrap();
    std::fs::write(site.join("a.txt"), "v1").unwrap();
    std::fs::write(site.join("sub/b.txt"), "b").unwrap();
    let remote = tmp.join("remote");
    std::fs::create_dir_all(&remote).unwrap();

    // Upload a folder.
    let p = run(&session, Direction::Upload, &site, &remote, Conflict::Overwrite).await;
    assert_eq!(p.state, JobState::Done, "{:?}", p.error);
    assert_eq!((p.files_done, p.backup_id.is_none()), (2, true));
    assert_eq!(read(remote.join("site/a.txt")), "v1");
    assert_eq!(read(remote.join("site/sub/b.txt")), "b");

    // "Only newer" skips unchanged files.
    let p = run(&session, Direction::Upload, &site, &remote, Conflict::Newer).await;
    assert_eq!((p.files_done, p.skipped), (0, 2));

    // Overwrite with new content: the old file goes into a backup…
    std::fs::write(site.join("a.txt"), "v2").unwrap();
    let p = run(&session, Direction::Upload, &site, &remote, Conflict::Overwrite).await;
    assert_eq!(p.state, JobState::Done, "{:?}", p.error);
    assert_eq!(read(remote.join("site/a.txt")), "v2");
    let backup_id = p.backup_id.expect("overwrite should create a backup");
    let tx = backup::list().unwrap().into_iter().find(|t| t.id == backup_id).unwrap();
    let same_fs = base.starts_with(dirs::home_dir().unwrap());
    let expected = if same_fs { Side::Remote } else { Side::Local };
    assert!(
        tx.entries.iter().all(|e| e.stored_on == expected) || !same_fs,
        "{base:?}: expected backups renamed on the server, got {:?}",
        tx.entries.iter().map(|e| e.stored_on).collect::<Vec<_>>()
    );

    // …and restoring it brings v1 back.
    backup::restore(&backup_id, Some(&session)).await.unwrap();
    assert_eq!(read(remote.join("site/a.txt")), "v1");

    // Deleting on the server moves the folder aside; restore puts it back.
    let target = remote.join("site").to_string_lossy().into_owned();
    let rec = Recorder::new(Side::Remote, Op::Delete, Some(&session), "e2e delete");
    rec.stash(Some(&session), &target).await.unwrap();
    let tx = rec.commit().unwrap().unwrap();
    assert!(!remote.join("site").exists());
    backup::restore(&tx.id, Some(&session)).await.unwrap();
    assert_eq!(read(remote.join("site/sub/b.txt")), "b");

    // Download the folder back.
    let dl = tmp.join("dl");
    std::fs::create_dir_all(&dl).unwrap();
    let p = run(&session, Direction::Download, &remote.join("site"), &dl, Conflict::Overwrite).await;
    assert_eq!(p.state, JobState::Done, "{:?}", p.error);
    assert_eq!(read(dl.join("site/sub/b.txt")), "b");

    // Clean up every backup, including the server-side copies in ~/.cache.
    for tx in backup::list().unwrap() {
        backup::delete(&tx.id, Some(&session)).await.unwrap();
    }
    assert!(backup::list().unwrap().is_empty());
    std::fs::remove_dir_all(tmp).unwrap();
}

/// Rough throughput check: `cargo test --release e2e_speed -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn e2e_speed() {
    let port: u16 = std::env::var("KADE_E2E_PORT").expect("KADE_E2E_PORT").parse().unwrap();
    let key = std::env::var("KADE_E2E_KEY").expect("KADE_E2E_KEY");
    let tmp = std::env::temp_dir().join(format!("kade-speed-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(tmp.join("remote")).unwrap();
    std::env::set_var("KADE_KNOWN_HOSTS", tmp.join("known_hosts"));
    let profile = ServerProfile {
        id: "speed".into(),
        name: "speed".into(),
        protocol: Protocol::Sftp,
        host: "127.0.0.1".into(),
        port,
        user: std::env::var("USER").unwrap(),
        group: String::new(),
        auth: Auth::KeyFile { path: key },
        remote_path: None,
        local_path: None,
        workspace: String::new(),
        tunnels: Vec::new(),
        updated_at: 0,
    };
    let fp = match ssh::connect(&profile, None, None).await {
        Err(AppError::HostKeyUnknown { fingerprint, .. }) => fingerprint,
        _ => panic!("expected unknown host"),
    };
    let session = Arc::new(ssh::connect(&profile, None, Some(fp)).await.unwrap().0);

    let file = tmp.join("big.bin");
    std::fs::write(&file, vec![7u8; 200 * 1024 * 1024]).unwrap();
    for direction in [Direction::Upload, Direction::Download] {
        let (src, dst) = match direction {
            Direction::Upload => (file.clone(), tmp.join("remote")),
            Direction::Download => (tmp.join("remote/big.bin"), tmp.join("back")),
        };
        std::fs::create_dir_all(&dst).unwrap();
        let t = std::time::Instant::now();
        let p = run(&session, direction, &src, &dst, Conflict::Overwrite).await;
        assert_eq!(p.state, JobState::Done);
        println!("{direction:?}: 200 MB in {:.2?} = {:.0} MB/s", t.elapsed(), 200.0 / t.elapsed().as_secs_f64());
    }
    std::fs::remove_dir_all(tmp).unwrap();
}

/// FTP and FTPS against a local test server. Run with
/// `KADE_E2E_FTP_ROOT=<dir> KADE_E2E_FTPS_ROOT=<dir> KADE_EXTRA_CA=<ca.pem> cargo test -- --ignored e2e_ftp`
/// against FTP servers on 127.0.0.1:2121 (plain) and :2122 (explicit TLS),
/// user `kade`, password `secret`, each serving the given directory.
#[tokio::test]
#[ignore]
async fn e2e_ftp() {
    let tmp = std::env::temp_dir().join(format!("kade-ftp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("KADE_DATA_HOME", tmp.join("data"));

    for (protocol, port, root_var) in [(Protocol::Ftp, 2121, "KADE_E2E_FTP_ROOT"), (Protocol::Ftps, 2122, "KADE_E2E_FTPS_ROOT")] {
        let root = std::path::PathBuf::from(std::env::var(root_var).expect(root_var));
        let profile = ServerProfile {
            id: format!("{protocol:?}"),
            name: format!("{protocol:?}"),
            protocol,
            host: "127.0.0.1".into(),
            port,
            user: "kade".into(),
            group: String::new(),
            auth: Auth::Password,
            remote_path: None,
            local_path: None,
            workspace: String::new(),
            tunnels: Vec::new(),
            updated_at: 0,
        };
        // Without a password Kade must ask for one.
        assert!(matches!(ssh::connect(&profile, None, None).await, Err(AppError::PasswordRequired)));
        assert!(matches!(ssh::connect(&profile, Some("wrong".into()), None).await, Err(AppError::AuthFailed { .. })));
        let (session, connected) = ssh::connect(&profile, Some("secret".into()), None).await.unwrap();
        assert!(connected.has_files && !connected.has_terminal);
        let session = Arc::new(session);
        let fs = session.fs().unwrap();

        // Work in the login directory; `on_disk` maps server paths to the host dir.
        let home = connected.home.trim_end_matches('/').to_string();
        let dir = format!("{home}/run-{protocol:?}-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let on_disk = |p: &str| root.join(p.strip_prefix(&home).unwrap_or(p).trim_start_matches('/'));
        fs.mkdir(&dir).await.unwrap();

        let site = tmp.join(format!("src-{protocol:?}/site"));
        std::fs::create_dir_all(site.join("sub")).unwrap();
        std::fs::write(site.join("a.txt"), "v1").unwrap();
        std::fs::write(site.join("sub/b.txt"), "b").unwrap();

        // Upload a folder, then list it back.
        let p = run(&session, Direction::Upload, &site, Path::new(&dir), Conflict::Overwrite).await;
        assert_eq!(p.state, JobState::Done, "{protocol:?}: {:?}", p.error);
        assert_eq!(read(on_disk(&format!("{dir}/site/a.txt"))), "v1");
        let listed = fs.list(&format!("{dir}/site")).await.unwrap();
        let a = listed.iter().find(|e| e.name == "a.txt").expect("a.txt listed");
        assert_eq!((a.size, a.is_dir), (2, false));
        assert!(listed.iter().any(|e| e.name == "sub" && e.is_dir));

        // Modification times survive (MFMT), so "only newer" skips everything.
        let p = run(&session, Direction::Upload, &site, Path::new(&dir), Conflict::Newer).await;
        assert_eq!((p.files_done, p.skipped), (0, 2), "{protocol:?}");

        // Overwrite → backup kept on this computer (never on an FTP server) → restore.
        std::fs::write(site.join("a.txt"), "v2").unwrap();
        let p = run(&session, Direction::Upload, &site, Path::new(&dir), Conflict::Overwrite).await;
        assert_eq!(read(on_disk(&format!("{dir}/site/a.txt"))), "v2");
        let id = p.backup_id.expect("overwrite should create a backup");
        let tx = backup::list().unwrap().into_iter().find(|t| t.id == id).unwrap();
        assert!(tx.entries.iter().all(|e| e.stored_on == Side::Local));
        backup::restore(&id, Some(&session)).await.unwrap();
        assert_eq!(read(on_disk(&format!("{dir}/site/a.txt"))), "v1");

        // Delete a folder into a backup and bring it back.
        let rec = Recorder::new(Side::Remote, Op::Delete, Some(&session), "ftp delete");
        rec.stash(Some(&session), &format!("{dir}/site")).await.unwrap();
        let tx = rec.commit().unwrap().unwrap();
        assert!(!on_disk(&format!("{dir}/site")).exists());
        backup::restore(&tx.id, Some(&session)).await.unwrap();
        assert_eq!(read(on_disk(&format!("{dir}/site/sub/b.txt"))), "b");

        // Rename, and exclusive create refuses to clobber.
        ssh::rename(&session, &format!("{dir}/site/a.txt"), &format!("{dir}/site/c.txt")).await.unwrap();
        assert!(on_disk(&format!("{dir}/site/c.txt")).exists());
        ssh::create_file(&session, &format!("{dir}/leeg.txt")).await.unwrap();
        assert!(ssh::create_file(&session, &format!("{dir}/leeg.txt")).await.is_err());

        // Download the folder.
        let dl = tmp.join(format!("dl-{protocol:?}"));
        std::fs::create_dir_all(&dl).unwrap();
        let p = run(&session, Direction::Download, Path::new(&format!("{dir}/site")), &dl, Conflict::Overwrite).await;
        assert_eq!(p.state, JobState::Done, "{protocol:?}: {:?}", p.error);
        assert_eq!(read(dl.join("site/sub/b.txt")), "b");

        for tx in backup::list().unwrap() {
            backup::delete(&tx.id, Some(&session)).await.unwrap();
        }
        ssh::delete(&session, &dir).await.unwrap();
        assert!(!on_disk(&dir).exists());
    }
    std::fs::remove_dir_all(tmp).unwrap();
}

/// Read-only check against a real server: connect and list the login folder.
/// `KADE_PROBE_HOST=… KADE_PROBE_USER=… KADE_PROBE_PASS=… KADE_PROBE_PROTOCOL=ftps cargo test -- --ignored probe_live`
#[tokio::test]
#[ignore]
async fn probe_live() {
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
    let protocol = match var("KADE_PROBE_PROTOCOL").as_str() {
        "ftp" => Protocol::Ftp,
        _ => Protocol::Ftps,
    };
    let profile = ServerProfile {
        id: "probe".into(),
        name: "probe".into(),
        protocol,
        host: var("KADE_PROBE_HOST"),
        port: std::env::var("KADE_PROBE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(21),
        user: var("KADE_PROBE_USER"),
        group: String::new(),
        auth: Auth::Password,
        remote_path: None,
        local_path: None,
        workspace: String::new(),
        tunnels: Vec::new(),
        updated_at: 0,
    };
    let t = std::time::Instant::now();
    let (session, connected) = ssh::connect(&profile, Some(var("KADE_PROBE_PASS")), None).await.unwrap();
    eprintln!("PROBE connected in {:.2?} · {} · home {}", t.elapsed(), connected.auth_label, connected.home);
    let t = std::time::Instant::now();
    let entries = session.fs().unwrap().list(&connected.home).await.unwrap();
    eprintln!("PROBE listed {} entries in {:.2?}", entries.len(), t.elapsed());
    for e in entries.iter().take(15) {
        eprintln!("PROBE   {} {:>10} {:?} {}", if e.is_dir { "d" } else { "-" }, e.size, e.permissions, e.name);
    }
}

/// Editing a remote file: saves upload (with backups), atomic saves work,
/// a changed server copy becomes a conflict. Same setup as e2e_transfer.
#[tokio::test]
#[ignore]
async fn e2e_edit() {
    use crate::edit::{EditInfo, EditState, Edits};
    crate::i18n::set("en");

    let port: u16 = std::env::var("KADE_E2E_PORT").expect("KADE_E2E_PORT").parse().unwrap();
    let key = std::env::var("KADE_E2E_KEY").expect("KADE_E2E_KEY");
    let tmp = dirs::home_dir().unwrap().join(format!(".cache/kade-e2e-edit-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("KADE_DATA_HOME", tmp.join("data"));
    std::env::set_var("KADE_KNOWN_HOSTS", tmp.join("known_hosts"));
    let profile = ServerProfile {
        id: "edit".into(),
        name: "edit".into(),
        protocol: Protocol::Sftp,
        host: "127.0.0.1".into(),
        port,
        user: std::env::var("USER").unwrap(),
        group: String::new(),
        auth: Auth::KeyFile { path: key },
        remote_path: None,
        local_path: None,
        workspace: String::new(),
        tunnels: Vec::new(),
        updated_at: 0,
    };
    let fp = match ssh::connect(&profile, None, None).await {
        Err(AppError::HostKeyUnknown { fingerprint, .. }) => fingerprint,
        _ => panic!("expected unknown host"),
    };
    let session = Arc::new(ssh::connect(&profile, None, Some(fp)).await.unwrap().0);

    let remote = tmp.join("site/index.blade.php");
    std::fs::create_dir_all(remote.parent().unwrap()).unwrap();
    std::fs::write(&remote, "v1").unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EditInfo>();
    let emit: crate::edit::Emit = Arc::new(move |e: &EditInfo| {
        let _ = tx.send(e.clone());
    });
    // Wait for the next event in one of the given states.
    async fn next(rx: &mut tokio::sync::mpsc::UnboundedReceiver<EditInfo>, states: &[EditState]) -> EditInfo {
        loop {
            let e = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
                .await
                .expect("timed out waiting for edit event")
                .unwrap();
            if states.contains(&e.state) {
                return e;
            }
        }
    }

    let edits = Edits::default();
    // `true` stands in for the editor: it accepts the path and exits.
    let info = edits.open(emit, session.clone(), "s".into(), remote.to_string_lossy().into(), "true").await.unwrap();
    let local = std::path::PathBuf::from(&info.local_path);
    assert_eq!(read(&local), "v1");
    assert_eq!(local.file_name().unwrap(), "index.blade.php", "editor must see the real name");
    next(&mut rx, &[EditState::Watching]).await;

    // A plain save uploads, keeping the old version in a backup.
    std::fs::write(&local, "v2").unwrap();
    let e = next(&mut rx, &[EditState::Uploaded, EditState::Error, EditState::Conflict]).await;
    assert_eq!(e.state, EditState::Uploaded, "{:?}", e.message);
    assert_eq!(read(&remote), "v2");
    assert!(backup::list().unwrap().iter().any(|t| t.summary.starts_with("Edited")));

    // An atomic save (temp file + rename, like vim/VS Code) is noticed too.
    let tmp_save = local.with_extension("swp");
    std::fs::write(&tmp_save, "v3").unwrap();
    std::fs::rename(&tmp_save, &local).unwrap();
    let e = next(&mut rx, &[EditState::Uploaded, EditState::Error, EditState::Conflict]).await;
    assert_eq!(e.state, EditState::Uploaded, "{:?}", e.message);
    assert_eq!(read(&remote), "v3");

    // Someone else changes the server copy: the next save must not clobber it.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await; // new mtime second
    std::fs::write(&remote, "from a colleague").unwrap();
    std::fs::write(&local, "v4").unwrap();
    let e = next(&mut rx, &[EditState::Uploaded, EditState::Error, EditState::Conflict]).await;
    assert_eq!(e.state, EditState::Conflict);
    assert_eq!(read(&remote), "from a colleague");

    // "Toch uploaden" overrides it (and backs the colleague's version up).
    edits.force_upload(&info.id).await.unwrap();
    assert_eq!(read(&remote), "v4");

    // "Serverversie ophalen" replaces the working copy.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    std::fs::write(&remote, "nieuwste").unwrap();
    edits.reload(&info.id).await.unwrap();
    assert_eq!(read(&local), "nieuwste");

    // Stopping removes the working copy.
    edits.stop(&info.id);
    assert!(!local.exists());

    for tx in backup::list().unwrap() {
        backup::delete(&tx.id, Some(&session)).await.unwrap();
    }
    std::fs::remove_dir_all(tmp).unwrap();
}

/// Real connection failures produce the friendly messages.
#[tokio::test]
#[ignore]
async fn e2e_network_errors() {
    let mut profile = ServerProfile {
        id: "net".into(),
        name: "net".into(),
        protocol: Protocol::Sftp,
        host: "127.0.0.1".into(),
        port: 1,
        user: "x".into(),
        group: String::new(),
        auth: Auth::Password,
        remote_path: None,
        local_path: None,
        workspace: String::new(),
        tunnels: Vec::new(),
        updated_at: 0,
    };
    let err = |r: Result<(Session, ssh::Connected), AppError>| match r {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected an error"),
    };
    let refused = err(ssh::connect(&profile, None, None).await);
    eprintln!("NET refused: {refused}");
    assert!(refused.contains("127.0.0.1") && refused.contains(" 1"));
    profile.host = "does-not-exist.invalid".into();
    let dns = err(ssh::connect(&profile, None, None).await);
    eprintln!("NET dns: {dns}");
    assert!(dns.contains("does-not-exist.invalid") && dns.contains("Tailscale"));
    profile.protocol = Protocol::Ftp;
    profile.port = 21;
    let ftp = err(ssh::connect(&profile, Some("x".into()), None).await);
    eprintln!("NET ftp dns: {ftp}");
    assert!(ftp.contains("does-not-exist.invalid") && ftp.contains("Tailscale"));
}

/// Status and tunnels over a real SSH connection. Same setup as e2e_transfer;
/// the server is this machine, so a local echo server stands in for a database.
#[tokio::test]
#[ignore]
async fn e2e_status_and_tunnel() {
    use crate::tunnel::{Tunnel, Tunnels};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port: u16 = std::env::var("KADE_E2E_PORT").expect("KADE_E2E_PORT").parse().unwrap();
    let key = std::env::var("KADE_E2E_KEY").expect("KADE_E2E_KEY");
    let tmp = std::env::temp_dir().join(format!("kade-e2e-tun-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    std::env::set_var("KADE_KNOWN_HOSTS", tmp.join("known_hosts"));
    let profile = ServerProfile {
        id: "tun".into(),
        name: "tun".into(),
        protocol: Protocol::Ssh,
        host: "127.0.0.1".into(),
        port,
        user: std::env::var("USER").unwrap(),
        group: String::new(),
        auth: Auth::KeyFile { path: key },
        remote_path: None,
        local_path: None,
        workspace: String::new(),
        tunnels: Vec::new(),
        updated_at: 0,
    };
    let fp = match ssh::connect(&profile, None, None).await {
        Err(AppError::HostKeyUnknown { fingerprint, .. }) => fingerprint,
        _ => panic!("expected unknown host"),
    };
    let session = Arc::new(ssh::connect(&profile, None, Some(fp)).await.unwrap().0);

    let status = crate::status::fetch(&session).await.unwrap();
    assert!(status.hostname.is_some(), "{status:?}");
    assert!(status.mem_total.unwrap() > 0 && status.cpus.unwrap() > 0, "{status:?}");
    assert!(status.cpu_percent.is_some() && !status.disks.is_empty(), "{status:?}");

    // "Remote" echo server on a free port.
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    let local_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let tunnel = Tunnel {
        id: "db".into(),
        name: "echo".into(),
        local_port,
        remote_host: "127.0.0.1".into(),
        remote_port: echo_port,
        auto_start: false,
    };
    let tunnels = Tunnels::default();
    let notify: crate::tunnel::Notify = Arc::new(|_| {});
    tunnels.start(session.clone(), "s", &tunnel, notify.clone()).await.unwrap();

    // Two connections at once through the tunnel, each echoed back.
    let mut a = tokio::net::TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
    let mut b = tokio::net::TcpStream::connect(("127.0.0.1", local_port)).await.unwrap();
    for (conn, msg) in [(&mut a, b"hello a".as_slice()), (&mut b, b"hello b".as_slice())] {
        conn.write_all(msg).await.unwrap();
        let mut buf = vec![0; msg.len()];
        tokio::time::timeout(std::time::Duration::from_secs(5), conn.read_exact(&mut buf)).await.unwrap().unwrap();
        assert_eq!(buf, msg);
    }
    let state = tunnels.list("s");
    assert_eq!((state.len(), state[0].open, state[0].total), (1, 2, 2));

    // The same local port can't be used twice.
    let clash = Tunnel { id: "other".into(), ..tunnel.clone() };
    assert!(tunnels.start(session.clone(), "s", &clash, notify).await.unwrap_err().to_string().contains(&local_port.to_string()));

    // Stopping frees the port.
    tunnels.stop("s", "db");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    std::net::TcpListener::bind(("127.0.0.1", local_port)).expect("port freed after stop");
    std::fs::remove_dir_all(tmp).unwrap();
}

// ---- "Sync folder" over rsync -------------------------------------------------
//
// Same sshd setup as above, plus a built app binary for the relay: run
// `cargo build` first (under `cargo test` this binary is the test harness).
// `KADE_RSYNC=/path/to/rsync` picks the local client; by default every rsync
// found here is tried (openrsync, and Homebrew GNU when installed).

mod sync {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use super::read;
    use crate::backup;
    use crate::error::AppError;
    use crate::profiles::{Auth, Protocol, ServerProfile};
    use crate::rsync::detect::{self, Detected};
    use crate::rsync::preview::{self, Change, SyncPreview};
    use crate::rsync::{run, Direction, Planned, SyncRequest};
    use crate::ssh::{self, Session};
    use crate::transfer::{Emit, JobState, Progress, Transfers};

    fn use_built_relay() {
        let exe = std::env::current_exe().unwrap();
        let bin = exe.parent().unwrap().parent().unwrap().join("kade");
        assert!(bin.exists(), "{} missing: run `cargo build` first", bin.display());
        std::env::set_var("KADE_RSYNC_RELAY", bin);
    }

    async fn connect(tmp: &Path) -> Arc<Session> {
        let port: u16 = std::env::var("KADE_E2E_PORT").expect("KADE_E2E_PORT").parse().unwrap();
        let key = std::env::var("KADE_E2E_KEY").expect("KADE_E2E_KEY");
        std::env::set_var("KADE_KNOWN_HOSTS", tmp.join("known_hosts"));
        let profile = ServerProfile {
            id: "sync".into(),
            name: "sync".into(),
            protocol: Protocol::Sftp,
            host: "127.0.0.1".into(),
            port,
            user: std::env::var("USER").unwrap(),
            group: String::new(),
            auth: Auth::KeyFile { path: key },
            remote_path: None,
            local_path: None,
            workspace: String::new(),
            tunnels: Vec::new(),
            updated_at: 0,
        };
        let fp = match ssh::connect(&profile, None, None).await {
            Err(AppError::HostKeyUnknown { fingerprint, .. }) => fingerprint,
            Err(e) => panic!("expected unknown host, got {e}"),
            Ok(_) => panic!("expected unknown host, got a connection"),
        };
        Arc::new(ssh::connect(&profile, None, Some(fp)).await.unwrap().0)
    }

    struct Env {
        session: Arc<Session>,
        detected: Arc<Detected>,
        transfers: Arc<Transfers>,
        tmp: PathBuf,
    }

    impl Env {
        fn path(&self, rel: &str) -> String {
            self.tmp.join(rel).to_string_lossy().into_owned()
        }

        fn request(&self, direction: Direction, local: &str, remote: &str, delete: bool) -> SyncRequest {
            SyncRequest {
                direction,
                local: self.path(local),
                remote: self.path(remote),
                delete,
                checksum: false,
                excludes: vec![".DS_Store".into()],
            }
        }

        async fn preview(&self, req: SyncRequest) -> (SyncPreview, Planned) {
            let cancel = tokio_util::sync::CancellationToken::new();
            preview::preview(&self.session, "e2e", &self.detected, "p".into(), req, &cancel).await.unwrap()
        }

        /// Run a plan; `on_progress` sees every update (and may cancel by id).
        async fn run(&self, plan: Planned, mut on_progress: impl FnMut(&Progress, &Transfers)) -> Progress {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let emit: Emit = Arc::new(move |p: &Progress| {
                let _ = tx.send(p.clone());
            });
            run::start(emit, self.transfers.clone(), self.session.clone(), self.detected.clone(), plan);
            while let Some(p) = rx.recv().await {
                on_progress(&p, &self.transfers);
                if matches!(p.state, JobState::Done | JobState::Failed | JobState::Cancelled) {
                    return p;
                }
            }
            panic!("sync ended without a final state");
        }

        async fn sync(&self, req: SyncRequest) -> (SyncPreview, Progress) {
            let (preview, plan) = self.preview(req).await;
            let p = self.run(plan, |_, _| {}).await;
            (preview, p)
        }
    }

    fn write(path: impl AsRef<Path>, text: &str) {
        let path = path.as_ref();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn names(p: &SyncPreview, change: Change) -> Vec<String> {
        let mut n: Vec<String> = p.items.iter().filter(|i| i.change == change).map(|i| i.path.clone()).collect();
        n.sort();
        n
    }

    fn mode(path: impl AsRef<Path>) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// Every rsync client on this machine, or just `KADE_RSYNC`.
    fn clients() -> Vec<Option<String>> {
        if std::env::var_os("KADE_RSYNC").is_some() {
            return vec![None];
        }
        let mut out = vec![Some("/usr/bin/rsync".to_string())];
        for gnu in ["/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"] {
            if Path::new(gnu).exists() {
                out.push(Some(gnu.to_string()));
            }
        }
        out
    }

    fn server_backups() -> Vec<std::ffi::OsString> {
        std::fs::read_dir(dirs::home_dir().unwrap().join(".cache/kade/backups"))
            .map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name()).collect())
            .unwrap_or_default()
    }

    #[tokio::test]
    #[ignore]
    async fn e2e_rsync_sync() {
        let _env = backup::TEST_ENV.lock().await;
        crate::i18n::set("en");
        use_built_relay();
        let home_base = dirs::home_dir().unwrap().join(".cache/kade-e2e");
        // Other runs (or the app itself) may keep backups there; only new ones count.
        let before = server_backups();
        for client in clients() {
            if let Some(c) = &client {
                std::env::set_var("KADE_RSYNC", c);
            }
            for base in [home_base.clone(), std::env::temp_dir()] {
                let tmp = base.join(format!("kade-e2e-sync-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&tmp).unwrap();
                std::env::set_var("KADE_DATA_HOME", tmp.join("data"));
                let session = connect(&tmp).await;
                let detected = Arc::new(detect::detect(&session).await);
                assert!(detected.support.available, "{:?}", detected.support);
                eprintln!("SYNC client {:?} → server {:?} in {}", detected.support.local, detected.support.remote, base.display());
                let env = Env { session, detected, transfers: Arc::new(Transfers::default()), tmp: tmp.clone() };

                to_server(&env).await;
                from_server(&env).await;
                modes(&env).await;
                symlinks(&env).await;
                hostile_names(&env).await;
                cancel_mid_run(&env).await;
                source_changed_after_preview(&env).await;

                for tx in backup::list().unwrap() {
                    backup::delete(&tx.id, Some(&env.session)).await.unwrap();
                }
                std::fs::remove_dir_all(&tmp).unwrap();
            }
            std::env::remove_var("KADE_RSYNC");
        }
        let _ = std::fs::remove_dir(home_base);
        let leftovers: Vec<_> = server_backups().into_iter().filter(|n| !before.contains(n)).collect();
        assert!(leftovers.is_empty(), "server-side backups left behind: {leftovers:?}");
    }

    /// Scenarios 1 and 3: edit 2, add 1, delete 1; restore; preview matches the run.
    async fn to_server(env: &Env) {
        let (l, r) = (env.tmp.join("s1/local"), env.tmp.join("s1/remote"));
        write(l.join("a.txt"), "a v1");
        write(l.join("b.txt"), "b");
        write(l.join("sub/c.txt"), "c v1");
        write(l.join(".DS_Store"), "x");
        // A first sync into a folder that doesn't exist yet: all new, no backup.
        let (pre, p) = env.sync(env.request(Direction::ToServer, "s1/local", "s1/remote", true)).await;
        assert_eq!((pre.new_files, pre.updated_files, pre.deleted), (3, 0, 0), "{pre:?}");
        assert_eq!(p.state, JobState::Done, "{:?}", p.error);
        assert_eq!((p.files_done, p.files_total), (3, 3));
        // Nothing was replaced, but what the sync added can be undone.
        let first = backup::list().unwrap().into_iter().find(|t| Some(&t.id) == p.backup_id.as_ref()).expect("additions are recorded");
        assert!(first.entries.iter().all(|e| e.created && e.stored.is_empty()), "{:?}", first.entries);
        assert_eq!(read(r.join("sub/c.txt")), "c v1");
        assert!(!r.join(".DS_Store").exists(), "excluded");

        write(l.join("a.txt"), "a version 2");
        write(l.join("sub/c.txt"), "c version 2");
        write(l.join("d.txt"), "d");
        std::fs::remove_file(l.join("b.txt")).unwrap();
        let (pre, p) = env.sync(env.request(Direction::ToServer, "s1/local", "s1/remote", true)).await;
        assert_eq!(names(&pre, Change::New), ["d.txt"]);
        assert_eq!(names(&pre, Change::Updated), ["a.txt", "sub/c.txt"]);
        assert_eq!(names(&pre, Change::Deleted), ["b.txt"]);
        assert_eq!(p.state, JobState::Done, "{:?}", p.error);
        assert_eq!((p.files_done, p.files_total), (3, 3), "the run did what the preview showed");
        assert_eq!(read(r.join("a.txt")), "a version 2");
        assert_eq!(read(r.join("d.txt")), "d");
        assert!(!r.join("b.txt").exists());
        let id = p.backup_id.expect("replaced and deleted files are backed up");
        let tx = backup::list().unwrap().into_iter().find(|t| t.id == id).unwrap();
        assert_eq!((tx.side, tx.op, tx.entries.len()), (backup::Side::Remote, backup::Op::Overwrite, 4), "{:?}", tx.entries);
        assert!(tx.entries.iter().all(|e| e.stored_on == backup::Side::Remote));
        assert_eq!(tx.entries.iter().filter(|e| e.created).count(), 1, "d.txt was added");

        // Nothing changed: nothing to do, and no transaction.
        let (pre, p) = env.sync(env.request(Direction::ToServer, "s1/local", "s1/remote", true)).await;
        assert!(pre.nothing_to_do, "{pre:?}");
        assert_eq!((p.state, p.backup_id), (JobState::Done, None));

        let undo = backup::restore(&id, Some(&env.session)).await.unwrap().expect("the restore is itself undoable");
        assert_eq!(read(r.join("a.txt")), "a v1");
        assert_eq!(read(r.join("sub/c.txt")), "c v1");
        assert_eq!(read(r.join("b.txt")), "b");
        assert!(!r.join("d.txt").exists(), "the added file is gone: the tree is as before the sync");
        assert!(backup::restore(&id, Some(&env.session)).await.is_err(), "a second restore is refused");
        // Undoing the restore brings the synced state back, the added file included.
        backup::restore(&undo.id, Some(&env.session)).await.unwrap();
        assert_eq!((read(r.join("a.txt")), read(r.join("d.txt")), r.join("b.txt").exists()), ("a version 2".into(), "d".into(), false));
    }

    /// Scenario 2: the same, mirrored.
    async fn from_server(env: &Env) {
        let (r, l) = (env.tmp.join("s2/remote"), env.tmp.join("s2/local"));
        write(r.join("a.txt"), "a v1");
        write(r.join("b.txt"), "b");
        write(r.join("sub/c.txt"), "c v1");
        let (pre, p) = env.sync(env.request(Direction::FromServer, "s2/local", "s2/remote", true)).await;
        assert_eq!((pre.new_files, p.state), (3, JobState::Done), "{:?}", p.error);
        assert_eq!(read(l.join("sub/c.txt")), "c v1");

        write(r.join("a.txt"), "a version 2");
        write(r.join("sub/c.txt"), "c version 2");
        write(r.join("d.txt"), "d");
        std::fs::remove_file(r.join("b.txt")).unwrap();
        let (pre, p) = env.sync(env.request(Direction::FromServer, "s2/local", "s2/remote", true)).await;
        assert_eq!(
            (names(&pre, Change::Updated), names(&pre, Change::Deleted)),
            (vec!["a.txt".into(), "sub/c.txt".into()], vec!["b.txt".to_string()])
        );
        assert_eq!(p.state, JobState::Done, "{:?}", p.error);
        assert_eq!((read(l.join("a.txt")), l.join("b.txt").exists()), ("a version 2".into(), false));
        let id = p.backup_id.expect("backup");
        let tx = backup::list().unwrap().into_iter().find(|t| t.id == id).unwrap();
        assert_eq!((tx.side, tx.entries.len()), (backup::Side::Local, 4));
        backup::restore(&id, None).await.unwrap();
        assert_eq!((read(l.join("a.txt")), read(l.join("b.txt"))), ("a v1".into(), "b".into()));
        assert!(!l.join("d.txt").exists(), "the added file is gone too");
        assert!(backup::restore(&id, None).await.is_err());
    }

    /// Scenario 4: destination modes survive an update; new executables keep +x.
    async fn modes(env: &Env) {
        let (l, r) = (env.tmp.join("s4/local"), env.tmp.join("s4/remote"));
        for d in [&l, &r] {
            write(d.join("private.txt"), if d == &l { "new content" } else { "old" });
            write(d.join("tool.sh"), if d == &l { "#!/bin/sh\necho new\n" } else { "#!/bin/sh\n" });
        }
        std::fs::set_permissions(r.join("private.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(r.join("tool.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(l.join("tool.sh"), std::fs::Permissions::from_mode(0o644)).unwrap();
        write(l.join("new.sh"), "#!/bin/sh\n");
        std::fs::set_permissions(l.join("new.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let (_, p) = env.sync(env.request(Direction::ToServer, "s4/local", "s4/remote", false)).await;
        assert_eq!(p.state, JobState::Done, "{:?}", p.error);
        assert_eq!(read(r.join("private.txt")), "new content");
        assert_eq!((mode(r.join("private.txt")), mode(r.join("tool.sh")), mode(r.join("new.sh"))), (0o600, 0o755, 0o755));
    }

    /// Scenario 5: a folder symlink at the destination survives (-K); a file
    /// symlink is skipped, reported, and its target untouched.
    async fn symlinks(env: &Env) {
        let (l, r) = (env.tmp.join("s5/local"), env.tmp.join("s5/remote"));
        write(l.join("shared/inner.txt"), "inner");
        write(l.join("config.php"), "from local");
        write(env.tmp.join("s5/real/keep.txt"), "kept");
        write(env.tmp.join("s5/target.php"), "target");
        std::fs::create_dir_all(&r).unwrap();
        std::os::unix::fs::symlink(env.tmp.join("s5/real"), r.join("shared")).unwrap();
        std::os::unix::fs::symlink(env.tmp.join("s5/target.php"), r.join("config.php")).unwrap();
        // GNU rsync 3.4+ as the *receiver* can't write into a symlinked folder even
        // with -K (ELOOP: its symlink-race fix opens every path component with
        // O_NOFOLLOW), so the dry run already fails and nothing is touched.
        let req = env.request(Direction::ToServer, "s5/local", "s5/remote", false);
        let gnu_receiver = env.detected.support.remote.as_ref().is_some_and(|v| v.is_gnu3() && v.version.as_deref() >= Some("3.4"));
        if gnu_receiver {
            let cancel = tokio_util::sync::CancellationToken::new();
            let err = preview::preview(&env.session, "e2e", &env.detected, "p".into(), req, &cancel).await.unwrap_err().to_string();
            eprintln!("SYNC known GNU limitation: dir symlink under -K: {err}");
            assert!(err.contains("symbolic links"), "{err}");
            assert!(!env.tmp.join("s5/real/inner.txt").exists());
        } else {
            let (pre, p) = env.sync(req).await;
            assert_eq!(names(&pre, Change::Skipped), ["config.php"]);
            assert_eq!(p.state, JobState::Done, "{:?}", p.error);
            assert_eq!(read(env.tmp.join("s5/real/inner.txt")), "inner");
        }
        assert!(std::fs::symlink_metadata(r.join("shared")).unwrap().file_type().is_symlink());
        assert!(std::fs::symlink_metadata(r.join("config.php")).unwrap().file_type().is_symlink());
        assert_eq!(read(env.tmp.join("s5/target.php")), "target");
    }

    /// Scenario 6: names and folders a shell would trip over.
    async fn hostile_names(env: &Env) {
        let names = ["a b", "it's", "$(touch pwned-kade)", "`touch pwned-kade`", "-rf", "üñí", "new\nline", "back\\slash"];
        let dir = "s6/remote $(touch pwned-kade) it's";
        for n in names {
            write(env.tmp.join("s6/local").join(n), n);
        }
        let (pre, p) = env.sync(env.request(Direction::ToServer, "s6/local", dir, true)).await;
        assert_eq!(pre.new_files, names.len() as u64, "{pre:?}");
        assert_eq!(p.state, JobState::Done, "{:?}", p.error);
        for n in names {
            assert_eq!(read(env.tmp.join(dir).join(n)), n);
        }
        let (_, p) = env.sync(env.request(Direction::FromServer, "s6/back", dir, false)).await;
        assert_eq!(p.state, JobState::Done, "{:?}", p.error);
        assert_eq!(read(env.tmp.join("s6/back/it's")), "it's");
        let home = dirs::home_dir().unwrap();
        for place in [home.as_path(), env.tmp.as_path(), Path::new("/tmp")] {
            assert!(!place.join("pwned-kade").exists(), "{place:?}");
        }
    }

    /// Scenario 7: cancel during a big file. No temp files on either side, and
    /// what was replaced before the cancel is restorable.
    async fn cancel_mid_run(env: &Env) {
        let (l, r) = (env.tmp.join("s7/local"), env.tmp.join("s7/remote"));
        write(r.join("a.txt"), "old");
        write(l.join("a.txt"), "new version");
        let mut data = vec![0u8; 600 * 1024 * 1024];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i.wrapping_mul(2654435761) >> 13) as u8;
        }
        std::fs::write(l.join("z.bin"), data).unwrap();
        let (_, plan) = env.preview(env.request(Direction::ToServer, "s7/local", "s7/remote", false)).await;
        let p = env
            .run(plan, |p, transfers| {
                if p.state == JobState::Running {
                    assert!(p.bytes_done < p.bytes_total, "the bar must not be full while a file is still being sent: {p:?}");
                }
                if p.files_done >= 1 {
                    transfers.cancel(&p.id);
                }
            })
            .await;
        assert_eq!(p.state, JobState::Cancelled, "{:?}", p.error);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        for d in [&l, &r] {
            let temp: Vec<_> = std::fs::read_dir(d)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .filter(|n| n.to_string_lossy().starts_with(".z.bin"))
                .collect();
            assert!(temp.is_empty(), "temp files left in {d:?}: {temp:?}");
        }
        let id = p.backup_id.expect("a.txt was replaced before the cancel");
        backup::restore(&id, Some(&env.session)).await.unwrap();
        assert_eq!(read(r.join("a.txt")), "old");
    }

    /// Scenario 8: the destination gained files after the preview: the run
    /// fails and deletes nothing.
    async fn source_changed_after_preview(env: &Env) {
        let (l, r) = (env.tmp.join("s8/local"), env.tmp.join("s8/remote"));
        write(l.join("keep.txt"), "keep");
        write(r.join("keep.txt"), "keep");
        write(r.join("old.txt"), "old");
        let (pre, plan) = env.preview(env.request(Direction::ToServer, "s8/local", "s8/remote", true)).await;
        assert_eq!(names(&pre, Change::Deleted), ["old.txt"]);
        write(r.join("extra.txt"), "extra");
        let p = env.run(plan, |_, _| {}).await;
        assert_eq!(p.state, JobState::Failed);
        assert!(p.error.unwrap().contains("preview again"));
        assert!(r.join("old.txt").exists() && r.join("extra.txt").exists(), "nothing deleted");
    }

    /// Scenario 9: the relay socket takes one connection with the right token.
    /// A wrong token is refused and rsync fails cleanly instead of hanging.
    #[tokio::test]
    #[ignore]
    async fn e2e_rsync_relay_auth() {
        use crate::rsync::args::{self, Pass};
        use crate::rsync::bridge::{self, RemoteSpec};
        use crate::rsync::{relay, RunDir};
        use_built_relay();
        let tmp = std::env::temp_dir().join(format!("kade-e2e-relay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        let req = SyncRequest {
            direction: Direction::ToServer,
            local: tmp.join("src").to_string_lossy().into(),
            remote: "/nowhere".into(),
            delete: false,
            checksum: false,
            excludes: vec![],
        };
        let run = RunDir::create().unwrap();
        run.write_rsh().unwrap();
        run.write_excludes(&[]).unwrap();
        let sock = run.sock();
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let rsync = detect::local().await.expect("a local rsync");
        let mut child = tokio::process::Command::new(&rsync.path)
            .args(args::build(&req, Pass::Preview, None, &run.rsh(), &run.excludes()))
            .env(relay::TOKEN_VAR, "not-the-token")
            .env(relay::SOCK_VAR, &sock)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let spec = RemoteSpec { rsync: "rsync".into(), path: "/nowhere".into(), direction: Direction::ToServer, backup_dir: None };
        assert!(bridge::accept(listener, &sock, "the-real-token", &spec).await.is_err());
        assert!(std::os::unix::net::UnixStream::connect(&sock).is_err(), "a second connection is refused");
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await.expect("rsync hung").unwrap();
        assert!(!status.success());
        drop(run);
        std::fs::remove_dir_all(tmp).unwrap();
    }
}

/// Detection with a GUI launch's minimal PATH: the fixed Homebrew paths are
/// found, and GNU 3.x wins over `/usr/bin/rsync` (openrsync) when both exist.
#[tokio::test]
#[ignore]
async fn e2e_rsync_detection_minimal_path() {
    let _env = backup::TEST_ENV.lock().await;
    let (client, path) = (std::env::var_os("KADE_RSYNC"), std::env::var_os("PATH").unwrap_or_default());
    std::env::remove_var("KADE_RSYNC");
    std::env::set_var("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    let found = crate::rsync::detect::local().await.expect("an rsync");
    std::env::set_var("PATH", path);
    if let Some(client) = client {
        std::env::set_var("KADE_RSYNC", client);
    }
    let gnu = ["/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"].into_iter().find(|p| Path::new(p).exists());
    eprintln!("DETECT {found:?}");
    match gnu {
        Some(gnu) => assert_eq!((found.path.to_str(), found.version.is_gnu3()), (Some(gnu), true)),
        None => assert_eq!(found.path, Path::new("/usr/bin/rsync")),
    }
}
