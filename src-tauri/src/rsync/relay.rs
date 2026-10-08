//! `kade --rsync-relay`: the "remote shell" rsync starts.
//!
//! rsync thinks it runs `ssh host rsync --server …`. Instead this process
//! hands its arguments to the running app over a private Unix socket, and
//! then pipes rsync's stdin/stdout through it; the app runs the real command
//! on its already signed-in SSH session. Only std and libc are used here: no
//! Tauri, no window, no dock icon.
//!
//! Wire format. Relay → app: one JSON line `{token, argv}`, then stdin as is,
//! with a half-close at EOF. App → relay: frames of `[kind: u8][len: u32 BE]
//! [payload]`, kind 0 stdout, 1 stderr, 2 exit status (4 bytes, i32 BE).

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;

pub(crate) const TOKEN_VAR: &str = "KADE_RSYNC_TOKEN";
pub(crate) const SOCK_VAR: &str = "KADE_RSYNC_SOCK";

pub(crate) const FRAME_STDOUT: u8 = 0;
pub(crate) const FRAME_STDERR: u8 = 1;
pub(crate) const FRAME_EXIT: u8 = 2;

/// rsync's own "error in rsync protocol data stream"-style code for a broken pipe.
const BROKEN: i32 = 255;

/// Entry point from `main.rs`; returns the process exit code.
pub fn main() -> i32 {
    let argv: Vec<String> = std::env::args().skip(2).collect();
    let (Ok(token), Some(sock)) = (std::env::var(TOKEN_VAR), std::env::var_os(SOCK_VAR)) else {
        eprintln!("kade: --rsync-relay is started by Kade itself");
        return BROKEN;
    };
    // openrsync hands us its socketpair end non-blocking; plain blocking reads
    // and writes would then fail with EAGAIN and cut the stream short.
    for fd in [0, 1] {
        // SAFETY: fcntl on our own stdio descriptors; no memory is touched.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags >= 0 && flags & libc::O_NONBLOCK != 0 {
                libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
    }
    relay(Path::new(&sock), &token, &argv, std::io::stdin(), &mut std::io::stdout(), &mut std::io::stderr())
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Hello {
    pub token: String,
    pub argv: Vec<String>,
}

pub(crate) fn relay(
    sock: &Path,
    token: &str,
    argv: &[String],
    input: impl Read + Send + 'static,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    let mut stream = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(err, "kade: can't reach Kade: {e}");
            return BROKEN;
        }
    };
    let hello = Hello { token: token.to_string(), argv: argv.to_vec() };
    let line = serde_json::to_string(&hello).expect("strings serialise") + "\n";
    if stream.write_all(line.as_bytes()).is_err() {
        let _ = writeln!(err, "kade: Kade closed the connection");
        return BROKEN;
    }
    let Ok(mut upstream) = stream.try_clone() else { return BROKEN };
    // Not joined: when the remote side exits, rsync may still hold stdin open.
    std::thread::spawn(move || {
        let mut input = input;
        let _ = std::io::copy(&mut input, &mut upstream);
        let _ = upstream.shutdown(Shutdown::Write);
    });
    loop {
        let mut head = [0u8; 5];
        if stream.read_exact(&mut head).is_err() {
            let _ = writeln!(err, "kade: the connection to the server was lost");
            return BROKEN;
        }
        let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
        let mut payload = vec![0u8; len];
        if stream.read_exact(&mut payload).is_err() {
            return BROKEN;
        }
        let written = match head[0] {
            FRAME_STDOUT => out.write_all(&payload).and_then(|()| out.flush()),
            FRAME_STDERR => err.write_all(&payload).and_then(|()| err.flush()),
            FRAME_EXIT => {
                return payload.get(..4).map_or(BROKEN, |b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            }
            _ => Ok(()),
        };
        // rsync went away; nothing left to relay to.
        if written.is_err() {
            return BROKEN;
        }
    }
}

pub(crate) fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}
