//! The app side of the relay: accept the one connection a run expects,
//! check who it is, turn rsync's remote command into one Kade builds itself,
//! and pipe it over an exec channel of the signed-in SSH session.
//!
//! The command is never run as rsync wrote it. Every argument must be one of
//! the flags Kade itself passes (see `args.rs`) in the shape openrsync or GNU
//! rsync sends them to the server; the paths come from the job spec and are
//! quoted here. So even a stolen token can only run this one sync again.

use std::path::Path;
use std::time::Duration;

use russh::ChannelMsg;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixListener;

use super::args::{HOST, REMOTE};
use super::relay::{frame, Hello, FRAME_EXIT, FRAME_STDERR, FRAME_STDOUT};
use super::Direction;
use crate::error::{AppError, AppResult};
use crate::ssh::Session;

/// The relay starts right after rsync; a run that hasn't connected by now never will.
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(30);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const HELLO_LIMIT: u64 = 64 * 1024;
/// Short flags Kade's own options turn into: `-rltKbinc8`.
const SHORT_FLAGS: &str = "rltKbinc8";

/// What the server side of one rsync run may be.
#[derive(Debug, Clone)]
pub struct RemoteSpec {
    /// The server's rsync, from detection.
    pub rsync: String,
    /// The folder on the server: the destination, or the source.
    pub path: String,
    pub direction: Direction,
    /// `--backup-dir` on the server, for a real run to it.
    pub backup_dir: Option<String>,
}

fn refused() -> AppError {
    AppError::other(tr!("rsync couldn't reach Kade", "rsync kon Kade niet bereiken"))
}

/// Quote one word for the server's shell. NUL and newlines are refused: they
/// can't be quoted for every shell, and no sane path has them.
fn quote(word: &str) -> Result<String, String> {
    if word.contains(['\n', '\r', '\0']) {
        return Err(format!("newline or NUL in {word:?}"));
    }
    shlex::try_quote(word).map(|q| q.into_owned()).map_err(|e| e.to_string())
}

/// Validate rsync's remote argv and build the command to exec instead.
/// The error text is only logged; the user sees one translated message.
///
/// The argv has the shape `kade rsync --server [--sender] <options…> . KADE_REMOTE[/]`.
pub fn remote_command(argv: &[String], spec: &RemoteSpec) -> Result<String, String> {
    let [host, rsync, server, rest @ ..] = argv else { return Err("too few arguments".into()) };
    if host != HOST || rsync != "rsync" || server != "--server" {
        return Err(format!("unexpected start: {host} {rsync} {server}"));
    }
    let Some((target, rest)) = rest.split_last() else { return Err("no path".into()) };
    let Some((dot, options)) = rest.split_last() else { return Err("no path".into()) };
    let slash = match target.strip_prefix(REMOTE) {
        Some("") => "",
        Some("/") => "/",
        _ => return Err(format!("unexpected path {target:?}")),
    };
    if dot != "." {
        return Err(format!("unexpected argument {dot:?}"));
    }
    let receiver = spec.direction == Direction::ToServer;
    let mut out = Vec::new();
    let mut sender = false;
    let mut words = options.iter();
    while let Some(word) = words.next() {
        let backup_dir = match word.as_str() {
            "--backup-dir" => Some(words.next().ok_or("--backup-dir without a value")?.as_str()),
            w => w.strip_prefix("--backup-dir="),
        };
        if backup_dir.is_some() {
            // rsync's value (possibly escaped differently per version) is ignored:
            // the receiver gets the job's own folder, a sender needs none.
            if receiver {
                let dir = spec.backup_dir.as_deref().ok_or("--backup-dir on a run without backups")?;
                out.push("--backup-dir".to_string());
                out.push(dir.to_string());
            }
            continue;
        }
        let digits = |prefix: &str| word.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        match word.as_str() {
            "--sender" if !receiver && !sender => sender = true,
            "--backup" | "--dirs" | "--delete-after" | "--log-format=%i" => out.push(word.clone()),
            _ if digits("--timeout=") => out.push(word.clone()),
            w if w.starts_with('-') && !w.starts_with("--") && w.len() > 1 => {
                let mut cluster = String::from("-");
                for (i, c) in w[1..].char_indices() {
                    match c {
                        // GNU's capability string runs to the end of the cluster.
                        'e' => {
                            let caps = &w[1 + i + 1..];
                            if caps.is_empty() || !caps.chars().all(|c| c == '.' || c.is_ascii_alphabetic()) {
                                return Err(format!("unexpected flag {w:?}"));
                            }
                            cluster.push('e');
                            cluster.push_str(caps);
                            break;
                        }
                        c if SHORT_FLAGS.contains(c) => cluster.push(c),
                        _ => return Err(format!("unexpected flag {w:?}")),
                    }
                }
                if cluster.len() > 1 {
                    out.push(cluster);
                }
            }
            _ => return Err(format!("unexpected argument {word:?}")),
        }
    }
    if !receiver && !sender {
        return Err("a download needs --sender".into());
    }
    if !spec.path.starts_with('/') {
        return Err("the server path must be absolute".into());
    }
    let rsync = if spec.rsync.starts_with('/') { quote(&spec.rsync)? } else { "rsync".into() };
    let mut cmd = vec![rsync, "--server".into()];
    if sender {
        cmd.push("--sender".into());
    }
    cmd.extend(out.iter().map(|w| quote(w)).collect::<Result<Vec<_>, _>>()?);
    cmd.push(".".into());
    cmd.push(quote(&format!("{}{slash}", spec.path.trim_end_matches('/')))?);
    Ok(cmd.join(" "))
}

/// The relay's connection after the handshake.
pub struct Accepted {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    command: String,
}

/// Wait for the relay, check it, and validate what it asks for. The listener is
/// closed and its socket removed after the first connection, good or bad.
pub async fn accept(listener: UnixListener, sock: &Path, token: &str, spec: &RemoteSpec) -> AppResult<Accepted> {
    let accepted = tokio::time::timeout(ACCEPT_TIMEOUT, listener.accept()).await;
    drop(listener);
    let _ = std::fs::remove_file(sock);
    let (stream, _) = accepted.map_err(|_| {
        eprintln!("kade: rsync relay never connected");
        refused()
    })??;
    // SAFETY: getuid has no preconditions and can't fail.
    let me = unsafe { libc::getuid() };
    if stream.peer_cred().map(|c| c.uid()).ok() != Some(me) {
        eprintln!("kade: rsync relay connection from another user refused");
        return Err(refused());
    }
    let (read, writer) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = Vec::new();
    let hello = tokio::time::timeout(HELLO_TIMEOUT, (&mut reader).take(HELLO_LIMIT).read_until(b'\n', &mut line)).await;
    let hello: Hello = match hello {
        Ok(Ok(_)) => serde_json::from_slice(&line).map_err(|_| refused())?,
        _ => return Err(refused()),
    };
    if !crate::mcp::constant_time_eq(hello.token.as_bytes(), token.as_bytes()) {
        eprintln!("kade: rsync relay connection with a wrong token refused");
        return Err(refused());
    }
    let command = remote_command(&hello.argv, spec).map_err(|e| {
        eprintln!("kade: rsync asked for an unexpected server command: {e}");
        AppError::other(tr!("rsync asked for an unexpected server command", "rsync vroeg om een onverwachte serveropdracht"))
    })?;
    Ok(Accepted { reader, writer, command })
}

/// Run the command on the server and pipe it to the relay until the server
/// side ends. `on_wire` gets every payload byte count, both ways.
pub async fn pump(conn: Accepted, session: &Session, on_wire: &(dyn Fn(u64) + Sync)) -> AppResult<()> {
    let Accepted { mut reader, mut writer, command } = conn;
    #[cfg(debug_assertions)]
    eprintln!("kade: rsync server command: {command}");
    let channel = session.ssh()?.channel_open_session().await?;
    channel.exec(true, command).await?;
    let (mut rx, tx) = channel.split();
    let result = {
        let up = async {
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                let n = reader.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                tx.data_bytes(buf[..n].to_vec()).await?;
                on_wire(n as u64);
            }
            tx.eof().await?;
            AppResult::Ok(())
        };
        let down = async {
            let mut code = None;
            while let Some(msg) = rx.wait().await {
                match msg {
                    ChannelMsg::Data { data } => {
                        on_wire(data.len() as u64);
                        writer.write_all(&frame(FRAME_STDOUT, &data)).await?;
                    }
                    ChannelMsg::ExtendedData { data, ext: 1 } => writer.write_all(&frame(FRAME_STDERR, &data)).await?,
                    ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
                    ChannelMsg::Failure => {
                        writer.write_all(&frame(FRAME_STDERR, b"kade: the server refused to run rsync\n")).await?;
                        break;
                    }
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            // No exit status: the session dropped, which rsync reports like a dead ssh.
            writer.write_all(&frame(FRAME_EXIT, &code.unwrap_or(255).to_be_bytes())).await?;
            let _ = writer.shutdown().await;
            AppResult::Ok(())
        };
        tokio::pin!(up, down);
        tokio::select! {
            r = &mut down => r,
            r = &mut up => match r {
                Ok(()) => (&mut down).await,
                // The relay is gone (rsync was killed): stop the server side too.
                Err(e) => Err(e),
            },
        }
    };
    let _ = tx.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(direction: Direction) -> RemoteSpec {
        RemoteSpec {
            rsync: "/usr/bin/rsync".into(),
            path: "/var/www/my site".into(),
            direction,
            backup_dir: Some("/home/me/.cache/kade/backups/t1/var/www/my site".into()),
        }
    }

    fn argv(line: &str) -> Vec<String> {
        line.split('|').map(String::from).collect()
    }

    /// Recorded from `/usr/bin/rsync` (openrsync, macOS 26) with `-e` pointing
    /// at a script that dumps "$@", for each pass Kade runs: preview, with
    /// `-c`, with deletes, and the real run.
    const OPENRSYNC: &[(Direction, &str)] = &[
        (Direction::ToServer, "kade|rsync|--server|-l|-n|-r|-t|--dirs|--log-format=%i|-8|-K|.|KADE_REMOTE"),
        (Direction::ToServer, "kade|rsync|--server|-c|-l|-n|-r|-t|--dirs|--log-format=%i|-8|-K|.|KADE_REMOTE"),
        (Direction::ToServer, "kade|rsync|--server|--delete-after|-l|-n|-r|-t|--dirs|--log-format=%i|-8|-K|.|KADE_REMOTE"),
        (Direction::ToServer, "kade|rsync|--server|-l|-r|-t|--backup|--backup-dir|/home/me/.cache/kade/backups/t1/var/www/my site|--dirs|--log-format=%i|-8|-K|.|KADE_REMOTE"),
        (Direction::FromServer, "kade|rsync|--server|--sender|-l|-n|-r|-t|--dirs|-8|.|KADE_REMOTE/"),
        (Direction::FromServer, "kade|rsync|--server|--sender|-c|-l|-n|-r|-t|--dirs|-8|.|KADE_REMOTE/"),
        (Direction::FromServer, "kade|rsync|--server|--sender|--delete-after|-l|-n|-r|-t|--dirs|-8|.|KADE_REMOTE/"),
        (Direction::FromServer, "kade|rsync|--server|--sender|-l|-r|-t|--backup|--backup-dir|/Users/me/Library/kade/backups/t1/Users/me/site|--dirs|-8|.|KADE_REMOTE/"),
    ];

    /// Recorded from GNU rsync 3.5.1 (Homebrew, protocol 33) the same way. One
    /// short cluster ending in the capability string (`i` is missing from it
    /// when `--delete-after` turns incremental recursion off), `--log-format=%i`
    /// on push, and the backup dir escaped for the old-args shell split.
    const GNU: &[(Direction, &str)] = &[
        (Direction::ToServer, "kade|rsync|--server|-nlKtre.iLsfxCIvu|--log-format=%i|--timeout=120|.|KADE_REMOTE"),
        (Direction::ToServer, "kade|rsync|--server|-nlKtrce.iLsfxCIvu|--log-format=%i|--timeout=120|.|KADE_REMOTE"),
        (Direction::ToServer, "kade|rsync|--server|-nlKtre.iLsfxCIvu|--log-format=%i|--timeout=120|--delete-after|.|KADE_REMOTE"),
        (Direction::ToServer, "kade|rsync|--server|-blKtre.iLsfxCIvu|--log-format=%i|--timeout=120|--backup-dir|/home/me/.cache/kade/backups/t1/var/www/my\\ site|.|KADE_REMOTE"),
        (Direction::FromServer, "kade|rsync|--server|--sender|-nltre.iLsfxCIvu|--timeout=120|.|KADE_REMOTE/"),
        (Direction::FromServer, "kade|rsync|--server|--sender|-nltrce.iLsfxCIvu|--timeout=120|.|KADE_REMOTE/"),
        (Direction::FromServer, "kade|rsync|--server|--sender|-nltre.LsfxCIvu|--timeout=120|--delete-after|.|KADE_REMOTE/"),
        (Direction::FromServer, "kade|rsync|--server|--sender|-bltre.iLsfxCIvu|--timeout=120|--backup-dir|/home/me/.cache/kade/backups/t1/var/www/my\\ site|.|KADE_REMOTE/"),
    ];

    #[test]
    fn accepts_recorded_argv() {
        for (direction, line) in OPENRSYNC.iter().chain(GNU) {
            let cmd = remote_command(&argv(line), &spec(*direction)).unwrap_or_else(|e| panic!("{line}: {e}"));
            assert!(cmd.starts_with("/usr/bin/rsync --server "), "{cmd}");
            let path = if *direction == Direction::ToServer { "'/var/www/my site'" } else { "'/var/www/my site/'" };
            assert!(cmd.ends_with(&format!(" . {path}")), "{cmd}");
        }
        let push = remote_command(&argv(OPENRSYNC[3].1), &spec(Direction::ToServer)).unwrap();
        assert!(push.contains("--backup-dir '/home/me/.cache/kade/backups/t1/var/www/my site'"), "{push}");
        let pull = remote_command(&argv(OPENRSYNC[7].1), &spec(Direction::FromServer)).unwrap();
        assert!(!pull.contains("backup-dir") && pull.contains("--sender"), "a sender gets no backup dir: {pull}");
        let gnu = remote_command(&argv(GNU[0].1), &spec(Direction::ToServer)).unwrap();
        assert_eq!(gnu, "/usr/bin/rsync --server -nlKtre.iLsfxCIvu '--log-format=%i' '--timeout=120' . '/var/www/my site'");
        let gnu = remote_command(&argv(GNU[3].1), &spec(Direction::ToServer)).unwrap();
        assert!(gnu.contains("--backup-dir '/home/me/.cache/kade/backups/t1/var/www/my site'"), "rsync's escaping is replaced: {gnu}");
    }

    #[test]
    fn rejects_anything_else() {
        let ok = "kade|rsync|--server|-l|-r|-t|.|KADE_REMOTE";
        assert!(remote_command(&argv(ok), &spec(Direction::ToServer)).is_ok());
        for bad in [
            "kade|rsync|--server|-l|-r|.|KADE_REMOTE|/etc/passwd",
            "kade|rsync|--server|-l|/etc|.|KADE_REMOTE",
            "kade|rsync|--server|-l|.|/etc",
            "kade|rsync|--server|--rsync-path=sh -c id|.|KADE_REMOTE",
            "kade|rsync|--server|-e|.|KADE_REMOTE",
            "kade|rsync|--server|-e;id|.|KADE_REMOTE",
            "kade|rsync|--server|-l;id|.|KADE_REMOTE",
            "kade|rsync|--server|;|.|KADE_REMOTE",
            "kade|rsync|--server|-p|.|KADE_REMOTE",
            "kade|rsync|--server|-logDtpre.iLsfxCIvu|.|KADE_REMOTE",
            "kade|rsync|--server|-nlEtrKie.iLsfxCIvu|.|KADE_REMOTE",
            "kade|rsync|--server|--executability|.|KADE_REMOTE",
            "kade|rsync|--server|--delete|.|KADE_REMOTE",
            "kade|rsync|--server|--log-format=X|.|KADE_REMOTE",
            "kade|rsync|--server|--max-delete=1;id|.|KADE_REMOTE",
            "kade|rsync|--server|--timeout=1;id|.|KADE_REMOTE",
            "kade|rsync|--server|--sender|-l|.|KADE_REMOTE",
            "kade|rsync|--server|-l|.|KADE_REMOTE;id",
            "kade|rsync|--server|-l|x|KADE_REMOTE",
            "kade|rsync|--server|.|KADE_REMOTE|KADE_REMOTE",
            "other|rsync|--server|-l|.|KADE_REMOTE",
            "kade|sh|--server|-l|.|KADE_REMOTE",
            "kade|rsync|-l|.|KADE_REMOTE",
            "kade|rsync|--server",
        ] {
            assert!(remote_command(&argv(bad), &spec(Direction::ToServer)).is_err(), "{bad}");
        }
        // A download must be a sender, and only once.
        assert!(remote_command(&argv("kade|rsync|--server|-l|.|KADE_REMOTE/"), &spec(Direction::FromServer)).is_err());
        assert!(remote_command(&argv("kade|rsync|--server|--sender|--sender|.|KADE_REMOTE/"), &spec(Direction::FromServer)).is_err());
        // A dry run has no backup dir to hand out.
        let dry = RemoteSpec { backup_dir: None, ..spec(Direction::ToServer) };
        assert!(remote_command(&argv("kade|rsync|--server|-b|--backup-dir=/x|.|KADE_REMOTE"), &dry).is_err());
    }

    fn relay_in_thread(
        sock: std::path::PathBuf,
        token: &'static str,
        input: &'static [u8],
    ) -> tokio::task::JoinHandle<(i32, Vec<u8>, Vec<u8>)> {
        tokio::task::spawn_blocking(move || {
            let (mut out, mut err) = (Vec::new(), Vec::new());
            let argv = argv(OPENRSYNC[0].1);
            let code = super::super::relay::relay(&sock, token, &argv, input, &mut out, &mut err);
            (code, out, err)
        })
    }

    #[tokio::test]
    async fn relay_round_trip() {
        let run = super::super::RunDir::create().unwrap();
        let sock = run.sock();
        let listener = UnixListener::bind(&sock).unwrap();
        let relay = relay_in_thread(sock.clone(), "secret", b"to the server");
        let mut conn = accept(listener, &sock, "secret", &spec(Direction::ToServer)).await.unwrap();
        assert!(conn.command.starts_with("/usr/bin/rsync --server -l -n -r -t --dirs"), "{}", conn.command);
        let mut up = Vec::new();
        conn.reader.read_to_end(&mut up).await.unwrap();
        assert_eq!(up, b"to the server", "stdin follows the hello unchanged");
        conn.writer.write_all(&frame(FRAME_STDOUT, b"from the server")).await.unwrap();
        conn.writer.write_all(&frame(FRAME_STDERR, b"a warning\n")).await.unwrap();
        conn.writer.write_all(&frame(FRAME_EXIT, &23i32.to_be_bytes())).await.unwrap();
        let (code, out, err) = relay.await.unwrap();
        assert_eq!((code, out.as_slice(), err.as_slice()), (23, b"from the server".as_slice(), b"a warning\n".as_slice()));
    }

    #[tokio::test]
    async fn refuses_a_wrong_token_and_a_second_connection() {
        let run = super::super::RunDir::create().unwrap();
        let sock = run.sock();
        let listener = UnixListener::bind(&sock).unwrap();
        let relay = relay_in_thread(sock.clone(), "guessed", b"");
        assert!(accept(listener, &sock, "secret", &spec(Direction::ToServer)).await.is_err());
        let (code, out, _) = relay.await.unwrap();
        assert_eq!((code, out.len()), (255, 0), "the relay fails without output");
        assert!(std::os::unix::net::UnixStream::connect(&sock).is_err(), "nothing listens after the first connection");
        let (code, _, err) = relay_in_thread(sock.clone(), "secret", b"").await.unwrap();
        assert_eq!(code, 255);
        assert!(String::from_utf8_lossy(&err).contains("can't reach Kade"));
    }

    #[test]
    fn quotes_remote_paths() {
        let run = |path: &str| {
            let spec = RemoteSpec { path: path.into(), ..spec(Direction::ToServer) };
            remote_command(&argv("kade|rsync|--server|-r|.|KADE_REMOTE"), &spec)
        };
        assert!(run("/srv/a b").unwrap().ends_with(" . '/srv/a b'"));
        assert!(run("/srv/it's").unwrap().ends_with(r#" . '/srv/it'"'"'s'"#) || run("/srv/it's").unwrap().ends_with(r#" . "/srv/it's""#));
        assert!(run("/srv/$(touch /tmp/pwned)").unwrap().ends_with(" . '/srv/$(touch /tmp/pwned)'"));
        assert!(run("/srv/`id`").unwrap().ends_with(" . '/srv/`id`'"));
        assert!(run("/srv/-rf").unwrap().ends_with(" . /srv/-rf"));
        assert!(run("-rf").is_err(), "relative paths are refused");
        assert!(run("/srv/new\nline").is_err());
        assert!(run("/srv/nul\0").is_err());
        let odd = RemoteSpec { rsync: "/opt/my rsync/rsync".into(), ..spec(Direction::ToServer) };
        assert!(remote_command(&argv("kade|rsync|--server|-r|.|KADE_REMOTE"), &odd).unwrap().starts_with("'/opt/my rsync/rsync' --server"));
        let alias = RemoteSpec { rsync: "rsync: aliased to rsync -v".into(), ..spec(Direction::ToServer) };
        assert!(remote_command(&argv("kade|rsync|--server|-r|.|KADE_REMOTE"), &alias).unwrap().starts_with("rsync --server"));
    }
}
