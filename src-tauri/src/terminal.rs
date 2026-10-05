use russh::client::Msg;
use russh::{ChannelMsg, ChannelWriteHalf};
use serde::Serialize;
use tauri::ipc::Channel;

use crate::error::AppResult;
use crate::ssh::Session;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TermEvent {
    Data { bytes: Vec<u8> },
    Exit,
}

pub struct Terminal {
    pub session_id: String,
    pub writer: ChannelWriteHalf<Msg>,
}

/// Open an interactive shell with a PTY on an existing SSH connection. Output
/// is streamed to `events` until the remote side closes the channel.
pub async fn open(session: &Session, session_id: String, cols: u32, rows: u32, events: Channel<TermEvent>) -> AppResult<Terminal> {
    let channel = session.ssh()?.channel_open_session().await?;
    channel.request_pty(true, "xterm-256color", cols, rows, 0, 0, &[]).await?;
    channel.request_shell(true).await?;
    let (mut reader, writer) = channel.split();

    tauri::async_runtime::spawn(async move {
        while let Some(msg) = reader.wait().await {
            match msg {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    if events.send(TermEvent::Data { bytes: data.to_vec() }).is_err() {
                        break;
                    }
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        let _ = events.send(TermEvent::Exit);
    });

    Ok(Terminal { session_id, writer })
}
