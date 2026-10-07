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
        direction,
        vec![source.to_string_lossy().into()],
        dest.to_string_lossy().into(),
        policy,
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
    let mut rec = Recorder::new(Side::Remote, Op::Delete, Some(&session), "e2e delete");
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
        let mut rec = Recorder::new(Side::Remote, Op::Delete, Some(&session), "ftp delete");
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
