# Security

Kade handles SSH keys, passwords and access to your servers, so security reports are very welcome.

## Reporting a vulnerability

Please **don't open a public issue**. Report it privately through
[GitHub's security advisories](https://github.com/jurnskie/kade/security/advisories/new) instead, with steps to
reproduce and the Kade version. You'll get a reply within a week.

## How Kade handles secrets

- Passwords are never written to disk. They come from 1Password (`op` CLI) or are typed in when connecting, and
  are only kept in memory for that connection.
- Private keys stay in 1Password or ssh-agent where possible. A key fetched from a 1Password vault is only held
  in memory.
- Host keys are verified against `~/.ssh/known_hosts`; a changed key stops the connection.
- The MCP server is off by default. When on, it only listens on 127.0.0.1 and requires a random bearer token.
- Updates are downloaded over HTTPS from GitHub releases and checked against a SHA-256 checksum before they're
  installed.
