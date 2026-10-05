# Contributing to Kade

Thanks for helping out! Bug reports, ideas and pull requests are all welcome.

## Reporting a bug

Open an issue with your OS and Kade version (Settings → About Kade), the kind of connection (SSH, SFTP, FTP or
FTPS, and how you sign in), what you did and what happened. Please leave out host names, user names and
anything else you'd rather not share.

For security issues, see [SECURITY.md](SECURITY.md) instead.

## Development

See [Build from source](README.md#build-from-source). Before opening a pull request:

```sh
pnpm check
cd src-tauri && cargo fmt --check && cargo test
```

CI runs the same checks on every pull request.

Some guidelines:

- Keep changes focused; one feature or fix per pull request.
- Match the style of the surrounding code. Comments explain *why*, not *what*.
- Kade must never lose data: anything that deletes or overwrites files goes through the backup module.
- Never store passwords. Secrets come from 1Password or the user, at the moment they're needed.

## Translations

Kade ships in English and Dutch.

- **Frontend**: wrap user-facing text in `t("English text")`, with `{name}` placeholders for values:
  `t("Connect to {name}", { name })`. For counts, use `tn(n, "{n} file", "{n} files")`. Add the Dutch
  translation to `src/lib/locales/nl.ts`; a missing entry falls back to English.
- **Backend**: user-facing errors use `tr!("English", "Dutch", args…)`, which works like `format!` with
  explicit arguments.

Adding a language means a new file in `src/lib/locales/`, an entry in the language list in Settings, and a third
text for `tr!` in the backend. Open an issue first, so we can agree on the approach.
