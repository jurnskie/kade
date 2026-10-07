//! SSH tunnels (local port forwarding, like `ssh -L`): Kade listens on
//! 127.0.0.1:<local_port> and carries every connection over the open SSH
//! session to <remote_host>:<remote_port> as seen from the server.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

use crate::error::{AppError, AppResult};
use crate::ssh::Session;

/// A saved tunnel, part of a connection profile (and synced with it).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
pub struct Tunnel {
    pub id: String,
    /// What it's for, e.g. "MySQL" or "Grafana".
    #[serde(default)]
    pub name: String,
    pub local_port: u16,
    /// Host as seen from the server; usually 127.0.0.1.
    pub remote_host: String,
    pub remote_port: u16,
    /// Start it whenever the connection opens.
    #[serde(default)]
    pub auto_start: bool,
}

/// A running tunnel, as the UI sees it.
#[derive(Debug, Clone, Serialize)]
pub struct TunnelState {
    pub tunnel_id: String,
    pub local_port: u16,
    /// Connections currently carried.
    pub open: u64,
    /// Connections carried since it started.
    pub total: u64,
}

struct Running {
    session_id: String,
    local_port: u16,
    open: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    task: JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Called whenever the set of running tunnels or their counters change.
pub type Notify = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
pub struct Tunnels(Mutex<HashMap<(String, String), Running>>);

impl Tunnels {
    pub async fn start(&self, session: Arc<Session>, session_id: &str, tunnel: &Tunnel, notify: Notify) -> AppResult<TunnelState> {
        session.ssh()?; // tunnels need SSH, not FTP
        let key = (session_id.to_string(), tunnel.id.clone());
        if let Some(r) = self.0.lock().unwrap().get(&key) {
            return Ok(state(&key.1, r));
        }
        let listener = TcpListener::bind(("127.0.0.1", tunnel.local_port)).await.map_err(|e| {
            AppError::other(if e.kind() == std::io::ErrorKind::AddrInUse {
                tr!(
                    "Port {port} on this computer is already in use; pick another local port",
                    "Poort {port} op deze computer is al in gebruik; kies een andere lokale poort",
                    port = tunnel.local_port
                )
            } else {
                tr!("Can't listen on port {port}: {e}", "Kan niet luisteren op poort {port}: {e}", port = tunnel.local_port, e = e)
            })
        })?;
        let open = Arc::new(AtomicU64::new(0));
        let total = Arc::new(AtomicU64::new(0));
        let task = tokio::spawn(serve(
            listener,
            session,
            tunnel.remote_host.clone(),
            tunnel.remote_port,
            open.clone(),
            total.clone(),
            notify,
            session_id.to_string(),
        ));
        let running = Running { session_id: session_id.to_string(), local_port: tunnel.local_port, open, total, task };
        let s = state(&key.1, &running);
        self.0.lock().unwrap().insert(key, running);
        Ok(s)
    }

    pub fn stop(&self, session_id: &str, tunnel_id: &str) {
        self.0.lock().unwrap().remove(&(session_id.to_string(), tunnel_id.to_string()));
    }

    pub fn stop_session(&self, session_id: &str) {
        self.0.lock().unwrap().retain(|_, r| r.session_id != session_id);
    }

    pub fn list(&self, session_id: &str) -> Vec<TunnelState> {
        self.0.lock().unwrap().iter().filter(|((s, _), _)| s == session_id).map(|((_, id), r)| state(id, r)).collect()
    }
}

fn state(id: &str, r: &Running) -> TunnelState {
    TunnelState {
        tunnel_id: id.to_string(),
        local_port: r.local_port,
        open: r.open.load(Ordering::Relaxed),
        total: r.total.load(Ordering::Relaxed),
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    listener: TcpListener,
    session: Arc<Session>,
    remote_host: String,
    remote_port: u16,
    open: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    notify: Notify,
    session_id: String,
) {
    // Owned here, so stopping the tunnel (aborting this task) also drops every
    // connection it carries.
    let mut conns = JoinSet::new();
    loop {
        let (mut stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(c) => c,
                // E.g. out of file descriptors: back off instead of spinning.
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => continue,
        };
        let (session, host, open, total, notify, sid) =
            (session.clone(), remote_host.clone(), open.clone(), total.clone(), notify.clone(), session_id.clone());
        conns.spawn(async move {
            let Some(ssh) = session.ssh.as_ref() else { return };
            let channel =
                match ssh.channel_open_direct_tcpip(host.as_str(), remote_port as u32, peer.ip().to_string(), peer.port() as u32).await {
                    Ok(c) => c,
                    // The server refused (nothing listening there, or forwarding disabled): drop this connection.
                    Err(_) => return,
                };
            open.fetch_add(1, Ordering::Relaxed);
            total.fetch_add(1, Ordering::Relaxed);
            notify(&sid);
            let mut remote = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut remote).await;
            open.fetch_sub(1, Ordering::Relaxed);
            notify(&sid);
        });
    }
}

/// Check a tunnel before saving it.
pub fn validate(t: &Tunnel) -> AppResult<()> {
    if t.local_port == 0 || t.remote_port == 0 {
        return Err(AppError::other(tr!("Ports must be between 1 and 65535", "Poorten moeten tussen 1 en 65535 liggen")));
    }
    if t.remote_host.trim().is_empty() {
        return Err(AppError::other(tr!("Enter the host to forward to", "Vul de host in om naar door te sturen")));
    }
    Ok(())
}
