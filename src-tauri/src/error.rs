use serde::Serialize;

/// Errors that cross the IPC boundary. The `kind` lets the frontend react to
/// specific situations (e.g. ask the user to trust an unknown host key).
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AppError {
    HostKeyUnknown {
        fingerprint: String,
        algorithm: String,
    },
    HostKeyChanged {
        line: usize,
    },
    AuthFailed {
        message: String,
    },
    PasswordRequired,
    AgentUnavailable {
        message: String,
    },
    /// A failed `op` call (prompt dismissed, not signed in, timeout…); `message` is already
    /// translated and cleaned up, so show it as is. Retrying is meaningful.
    OnePassword {
        message: String,
    },
    SessionNotFound,
    Unsupported {
        message: String,
    },
    Other {
        message: String,
    },
}

/// Shown in the transfer queue, edit bar and logs, so in the user's language.
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            AppError::HostKeyUnknown { fingerprint, .. } => {
                tr!("Unknown host key ({fp})", "Onbekende host-sleutel ({fp})", fp = fingerprint)
            }
            AppError::HostKeyChanged { line } => tr!(
                "The host key changed since the last connection (known_hosts line {line})",
                "Host-sleutel is gewijzigd sinds de vorige verbinding (known_hosts regel {line})",
                line = line
            ),
            AppError::AuthFailed { message } => tr!("Login failed: {m}", "Inloggen mislukt: {m}", m = message),
            AppError::PasswordRequired => tr!("Password required", "Wachtwoord vereist"),
            AppError::AgentUnavailable { message } => {
                tr!("Agent not reachable: {m}", "Agent niet bereikbaar: {m}", m = message)
            }
            AppError::OnePassword { message } => message.clone(),
            AppError::SessionNotFound => tr!("Connection not found", "Verbinding niet gevonden"),
            AppError::Unsupported { message } => tr!("Not supported: {m}", "Niet ondersteund: {m}", m = message),
            AppError::Other { message } => message.clone(),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for AppError {}

/// Turn low-level connection errors ("No route to host (os error 65)") into an
/// explanation with a likely fix. Anything unrecognised is passed through.
pub fn network_error(host: &str, port: u16, e: impl std::fmt::Display) -> AppError {
    let raw = e.to_string();
    let msg = raw.to_lowercase();
    let text = if msg.contains("no route to host") || msg.contains("host is unreachable") {
        tr!(
            "{host} can't be reached from this network. Are you on a different network than the server? \
             Then use e.g. its Tailscale address. On macOS, also check System Settings → Privacy & \
             Security → Local Network.",
            "{host} is niet bereikbaar vanaf dit netwerk. Zit je ergens anders dan het netwerk van de server? \
             Gebruik dan bijvoorbeeld het Tailscale-adres. Op macOS: controleer ook Systeeminstellingen → \
             Privacy en beveiliging → Lokaal netwerk.",
            host = host
        )
    } else if msg.contains("network is unreachable") {
        tr!("No network connection.", "Geen netwerkverbinding.")
    } else if msg.contains("connection refused") {
        tr!(
            "{host} refuses the connection on port {port}. Is the service running, and is the port right?",
            "{host} weigert de verbinding op poort {port}. Draait de dienst, en klopt de poort?",
            host = host,
            port = port
        )
    } else if msg.contains("timed out") || msg.contains("time out") {
        tr!(
            "{host} doesn't respond (timeout). The server is off, or a firewall blocks port {port}.",
            "{host} reageert niet (time-out). De server staat uit, of een firewall houdt poort {port} tegen.",
            host = host,
            port = port
        )
    } else if msg.contains("lookup address")
        || msg.contains("nodename nor servname")
        || msg.contains("name or service not known")
        || msg.contains("no address associated")
    {
        tr!(
            "The name {host} wasn't found. Is the hostname right, and is Tailscale on if it's a Tailscale name?",
            "De naam {host} is niet gevonden. Klopt de hostnaam, en staat Tailscale aan als het een Tailscale-naam is?",
            host = host
        )
    } else {
        return AppError::other(raw);
    };
    AppError::Other { message: text }
}

impl AppError {
    pub fn other(e: impl std::fmt::Display) -> Self {
        AppError::Other { message: e.to_string() }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::other(e)
    }
}

impl From<russh::Error> for AppError {
    fn from(e: russh::Error) -> Self {
        AppError::other(e)
    }
}

impl From<russh_sftp::client::error::Error> for AppError {
    fn from(e: russh_sftp::client::error::Error) -> Self {
        AppError::other(e)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::other(e)
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_network_errors() {
        let e = network_error("10.0.0.20", 22, "No route to host (os error 65)");
        assert!(e.to_string().contains("Tailscale"));
        let e = network_error("myhost", 22, "failed to lookup address information: Name or service not known");
        assert!(e.to_string().contains("myhost"));
        let e = network_error("h", 2222, "Connection refused (os error 111)");
        assert!(e.to_string().contains("2222"));
        assert!(network_error("h", 22, "No route to host").to_string().contains("Tailscale"));
        assert_eq!(network_error("h", 22, "iets anders").to_string(), "iets anders");
    }
}
