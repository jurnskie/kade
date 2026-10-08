# Plan: "Sync folder" over rsync

Status: backend built (phases 1–4, e2e of phase 6); UI (phase 5) and the step 0 benchmark not done. See
"Answers from the spikes" under Open questions for where the build differs from this design.

## Goal and non-goals

**Goal.** A folder can be synced one way, "Sync to server" or "Sync from server". The sync uses the local
`rsync` binary, and its traffic goes over Kade's existing russh session. This keeps every sign-in route
working without extra steps: the 1Password agent, a key fetched with `op`, a typed password, pinned keys,
workspace defaults and host-key checks. Kade shows a dry-run preview first. Every overwrite and delete goes
through Kade's backup transactions, so Restore works the same way it does today.

**Non-goals.**
- Replacing normal uploads and downloads. Drag-and-drop and the transfer queue stay on SFTP/FTP.
- Two-way sync, watching for changes, or scheduled syncs.
- FTP/FTPS. They have no exec channel, so sync is never offered there.
- Running the system `ssh`. It would lose Kade's sign-in, pinned keys and known-hosts handling.
- Preserving owner, group or permissions from the source (see Semantics).
- Pause in v1.

## Step 0 — benchmark gate

Only build this if rsync is clearly faster for the case it targets: re-uploading a large tree with few changes.

**Data set.** A real project checkout, such as a Statamic site with `vendor/`. That is about 3,000–8,000
files, 500+ folders and 100–300 MB. Note the exact file, folder and byte counts.

**Hosts.**
- LAN: an sshd under 1 ms RTT.
- Remote: a VPS at 20–40 ms RTT, such as a Forge box. Record the RTT from `ping`.

**Scenarios.** Each is run against a destination that already holds the previous state.

| # | Scenario | Kade (SFTP) | rsync |
|---|----------|-------------|-------|
| A | Cold upload to an empty dir | upload, policy any | `rsync -rlt` |
| B | 10 files edited, 2 added, 1 deleted | upload, policy `Newer` | `rsync -rlt --delete-after` |
| C | No changes | upload, policy `Newer` | `rsync -rlt` |
| D | B as a dry run only | n/a | `rsync -rlt -n -i` |

**How to run it.**
- Kade: add an `#[ignore]` bench test next to `e2e.rs` that calls `transfer::start` and times until `Done`.
  Use a release build.
- rsync: `rsync … -e "ssh -o ControlPath=none"`, once with `/usr/bin/rsync` (openrsync) and once with GNU
  3.x from Homebrew. Measure the SSH handshake separately and subtract it. Kade's session is already open.
- Add a control: Kade with `make_dirs` listing folders in parallel. Today it lists one folder at a time,
  which is the likely bottleneck in B and C. A one-hour prototype is enough.
- Run each cell 5 times and report the median and min. Record whether the caches were warm or cold. Run
  scenario A once.

**What to measure.**
- Wall time.
- Bytes on the wire, from `nettop -P -L1` or interface counters before and after.
- Local CPU time from `/usr/bin/time -l`.
- Number of SFTP requests, as a debug counter in `RemoteFs`.

**Decision.** Build it only if both conditions hold on the remote host:
- In scenario B, rsync's median is ≤ ⅓ of Kade's `Newer` time, and it saves at least 5 s in absolute terms.
- The parallel-listing control does not close the gap to within 2×.

Otherwise, ship parallel listing in `transfer.rs` and drop this plan. Re-run B after phase 2 through
Kade's relay. The relay must reach ≥ 80 % of the system-ssh throughput, or the design must be revisited
(russh window sizes, cipher choice).

## Architecture

```
 Kade app (tauri)                                        server
 ┌───────────────────────────────────────────┐
 │ sync job ── spawns ──► rsync (client)       │
 │   │            stdout: itemize lines ─┐     │
 │   │            -e <rundir>/rsh        │     │
 │   │                   │ spawns        │     │
 │   │                   ▼               │     │
 │   │        kade --rsync-relay         │     │
 │   │      (same binary, no GUI)        │     │
 │   │        stdin/stdout ⇅             │     │
 │   ▼                   │ unix socket   │     │   russh session (already signed in)
 │ bridge ◄──────────────┘ token+argv    │     │   exec channel:
 │   └── validates argv, opens exec ═════╪═════╪══► rsync --server [--sender] … . '<path>'
 │       channel, pipes bytes both ways  │     │
 │ parser ◄──────────────────────────────┘     │
 │   → Progress events, backup journal         │
 └───────────────────────────────────────────┘
```

**Relay mechanism (recommended: re-exec Kade's own binary).**
- `main.rs` checks `argv[1] == "--rsync-relay"` before `kade_lib::run()`, and runs
  `kade_lib::rsync::relay::main()`. That path never touches Tauri or GTK/AppKit. On macOS this starts no
  dock icon, because NSApplication is never initialised.
- `std::env::current_exe()` gives the binary path:
  - macOS: `Kade.app/Contents/MacOS/kade`
  - deb/rpm: `/usr/bin/kade`
  - AppImage: `/tmp/.mount_*/usr/bin/kade`, which is valid while the app runs.
- `-e` gets a generated wrapper script, `<rundir>/rsh`, mode 0700:
  `exec env LD_LIBRARY_PATH='…' '<exe>' --rsync-relay "$@"`. The script solves three problems:
  - rsync splits the `-e` string on whitespace, and openrsync handles quoting differently.
  - `hostenv::command` strips the AppImage `LD_LIBRARY_PATH` from children, but Kade's own binary needs
    it back to load its bundled libraries.
  - It puts the arguments into one known shape.
- Run dir: `$TMPDIR/kade-rsync-<random>/`, mode 0700. It holds `rsh`, `sock` and `excludes`, and is removed
  when the job ends. The path must fit the `sun_path` limit of 104 bytes on macOS. Check this, and fall
  back to `/tmp/kade-<uid>/` when it is too long.
- The relay connects to `sock` and sends one JSON line: `{token, argv}`. The token comes from the env var
  `KADE_RSYNC_TOKEN`, which is set on the rsync process and inherited by the relay. The relay then splices
  stdin→socket and socket→stdout. A one-byte trailer frame carries the remote exit status, so the relay
  exits with it.
  - Framing: after the hello, data is passed through raw. The exit status arrives as an out-of-band last
    message on a second tiny socket. Alternatively, use length-prefixed frames, which is simpler to get
    right; decide in phase 2.
  - Remote stderr (`ExtendedData`) is written to the relay's stderr. That stderr is inherited from rsync,
    so it lands in the stderr pipe Kade already captures.
- Bridge, in-app: one `UnixListener` per run.
  - It accepts exactly one connection.
  - It checks `peer_cred().uid == getuid()` and compares the token in constant time (reuse the helper in
    `mcp.rs`).
  - It validates the argv and builds the remote command (see Security).
  - It opens `session.ssh()?.channel_open_session()` and runs `exec(true, cmd)`, the same pattern as
    `status.rs`.
  - It pipes data, counting wire bytes for the speed display.

**Alternatives considered.**
- **Tauri sidecar** (`bundle.externalBin`, a tiny `kade-relay` bin with only std/tokio). Cleaner at
  runtime: no WebKit dylib load (about 30–80 ms per start) and no AppImage env issues. It costs
  target-triple build plumbing in `beforeBuildCommand`, and macOS signing of a second binary. This is the
  upgrade path if the re-exec start time or AppImage issues bite.
- **`sh` + FIFOs, no helper.** `-e "<rundir>/rsh"` would be
  `printf '%s\0' "$@" >ctl; cat <in & exec cat >out`. It needs only `/bin/sh` and `cat`, and directory
  permissions are the authentication. It loses the remote exit status and costs two extra copies. Kept
  as the fallback if re-exec is blocked on some platform.
- **Pre-opened fds (fd 3/4) inherited through rsync into the rsh.** Rejected: it relies on neither
  rsync implementation closing extra fds, and openrsync's behaviour is undocumented.
- **Running both sides as `rsync --server` and wiring them together.** Rejected: both server roles wait
  for the client's filter list (`recv_filter_list`) and would deadlock. Making it work means writing the
  client half of the protocol ourselves.
- **System `ssh` with ControlMaster.** Rejected: see the non-goals.

## Detection

Detection runs once per session, after connect, and is lazy: it starts the first time a folder context
menu opens. The result is cached in `AppState` by `session_id`.

**Local side.**
- Candidates, in order:
  - `$KADE_RSYNC` (tests)
  - `/opt/homebrew/bin/rsync`
  - `/usr/local/bin/rsync`
  - `/usr/bin/rsync`
  - `which rsync` through `hostenv::command`
- A GUI launch has a minimal `PATH`, so these fixed paths matter. Prefer GNU 3.x over openrsync when both
  exist.
- Parse the first line of `rsync --version`:
  - `openrsync: protocol version 29` → openrsync, the macOS 15.4+ default.
  - `rsync version 2.6.9 … protocol version 29` → old samba rsync, older macOS.
  - `rsync  version 3.x.y  protocol version 31/32` → GNU.
- Minimums: GNU ≥ 3.0, 2.6.9, or any openrsync. For **Sync from server** with GNU < 3.4.0, warn: those
  versions have the CVE-2024-12087/12088 client issues, where a malicious server can write outside the
  destination.

**Remote side.**
- One exec through the existing session, with a 5 s timeout:
  `command -v rsync && rsync --version 2>/dev/null | head -n 1`.
- If exec is refused (ForceCommand internal-sftp, restricted shell), report "the server only allows
  SFTP".
- The remote must have `remote_home` (SFTP), because the backup dir and the paths come from it.

**Result.**
- `RsyncSupport { available: bool, reason: Option<String>, local: Version, remote: Version }`
- The menu items are disabled with `reason` as a tooltip.
- FTP/FTPS, and SSH-only profiles without SFTP, get a fixed reason and are never probed.
- Phase 1 spike: confirm that the openrsync client produces itemize lines in **push** mode against a GNU
  server. They come from the remote generator. If it does not, preview is impossible for that combination
  and it is reported as unsupported.

## Flags

Only flags that openrsync, 2.6.9 and GNU 3.x all accept are used. The short flags `-E` and `-s` are never
used: `-E` means `--extended-attributes` in openrsync and `--executability` in GNU, and openrsync has no
`-s`.

| Flag | Why | openrsync | 2.6.9 | GNU 3.x | Used |
|------|-----|:-:|:-:|:-:|------|
| `-r` | recurse | ✓ | ✓ | ✓ | always |
| `-l` | copy symlinks as symlinks (never follow source links) | ✓ | ✓ | ✓ | always |
| `-t` | keep mtimes (incremental runs and Kade's "Newer" both rely on it) | ✓ | ✓ | ✓ | always |
| `-K` / `--keep-dirlinks` | a symlink-to-dir at the destination stays a symlink | ✓ | ✓ | ✓ | always |
| `--executability` | new files get the source's x bit; existing files keep their mode | ✓ | ✓ | ✓ | always (long form only) |
| `-n` | dry run | ✓ | ✓ | ✓ | preview |
| `-i` / `--itemize-changes` | per-item change codes | ✓ | ✓ | ✓ | both runs |
| `--out-format='%i %l %n'` | parseable line: codes, length, name | ✓ | ✓ | ✓ | both runs |
| `-8` | don't escape UTF-8 in names (control chars are still escaped `\#ooo`) | ✓ | ✓ | ✓ | always |
| `-b --backup-dir=<abs>` | move replaced or deleted files into Kade's tx folder | ✓ | ✓ | ✓ | real run |
| `--delete-after` | opt-in; deletes only after a successful transfer, and GNU skips deletes on I/O errors | ✓ | ✓ | ✓ | opt-in |
| `--max-delete=<n>` | cap at the preview's delete count, so a changed tree can't delete more than was shown | ✓ | ✓ | ✓ | with delete |
| `--exclude-from=<file>` | user excludes and Kade's backup roots (never on argv) | ✓ | ✓ | ✓ | always |
| `--timeout=120` | an I/O stall becomes an error instead of hanging | ✓ | ✓ | ✓ | always |
| `-c` | compare by checksum instead of size+mtime | ✓ | ✓ | ✓ | option ("Compare contents") |
| `-e <rundir>/rsh` | the relay | ✓ | ✓ | ✓ | always |
| `-p -o -g` | would impose the source's mode/owner | ✓ | ✓ | ✓ | **never** |
| `--partial`, `--inplace` | leave half files at the destination | ✓ | ✓ | ✓ | **never** |
| `-z` | compression; zlib negotiation across implementations at protocol 29 has a history of bugs | ✓ | ✓ | ✓ | not in v1 |
| `-s`/`--protect-args`, `--info=progress2`, `--mkpath`, `--old-args` | | ✗ | ✗ | ✓ | not used |

Kade's own rules for the arguments:
- The destination folder is created with SFTP `mkdir_p` or `create_dir_all` first, because there is no
  `--mkpath`.
- The source is passed with a trailing `/`, so the contents go into the destination.
- Local paths are always absolute. A leading `/` rules out option parsing and the `host:` interpretation.

## Semantics

Kade's transfers keep the destination's mode and owner, and write through symlinks. rsync's version of
this:

- **Mode.**
  - Without `-p`, GNU keeps the existing permissions of updated files. New files get the source mode minus
    the umask, and `--executability` carries the x bit.
  - openrsync's behaviour without `-p` is not documented. It gets an e2e test, and if it differs, sync is
    disabled when the local client is openrsync.
- **Owner and group.**
  - Without `-o`/`-g`, rsync's temp file and rename make a replaced file owned by the login user and the
    folder's default group.
  - Kade's SFTP path tries to keep both through `restore_kept`, but a non-root user can't chown anyway. In
    practice only the group can differ (for example `www-data` without setgid).
  - The preview shows a note: "Files replaced by sync get your user's default group". This divergence is
    accepted.
- **Symlinks at the destination.**
  - `-K` keeps folder symlinks, such as a `storage` → shared dir link on Forge.
  - A symlink to a *file* at the destination would be replaced by a regular file. Kade never does that.
  - So during the preview, Kade checks each `>f` item whose destination exists. Remote: from the SFTP
    listings of the touched folders. Local: `symlink_metadata`.
  - A link that would be replaced is added to the excludes and listed under "Skipped: symlinked files —
    use a normal upload for these".
- **Atomic replace.** rsync writes `.name.XXXXXX` and renames it into place, so a failure never leaves a
  half file. This matches Kade's "don't leave a half-written file" rule.

## Backups and the journal

**Mapping.**
- Each sync job is one `Recorder`:
  - "to server" → `Side::Remote`
  - "from server" → `Side::Local`
  - `Op::Overwrite` (see Open questions), with the summary "Sync of X to Y".
- `--backup-dir` is set to the transaction folder mirrored at the *destination root*, so rsync's layout
  matches Kade's `mirrored()` layout exactly:
  - Remote: `<remote_home>/.cache/kade/backups/<tx>/<dest_root without leading />`
  - Local: `<data>/kade/backups/<tx>/<dest_root without leading />`
  - rsync stores `rel/path` at `<backup-dir>/rel/path`, which is `mirrored(root, dest_root/rel/path)`.
- Nothing is lost with `--backup-dir`: rsync uses no suffix, and repeated syncs create new transactions.

**Recording.**
- *Streamed.* During the real run, each itemize line is recorded through a new public
  `Recorder::record(BackupEntry)`, which is the existing private `add_entry`. The journal stays
  crash-safe this way. Lines recorded:
  - `>f.`/`<f.` lines for files that existed (not `+++++++`). Push shows `<f`, pull shows `>f`.
  - Every `*deleting` line for a file.
  - Entry values: `original = dest_root/rel`, `stored = backup_dir/rel`, `stored_on = side`,
    `is_dir = false`.
- *Reconciled.* When the job ends (done, failed or cancelled), the backup dir is walked: SFTP `list`, or
  `read_dir`. Any file not yet recorded is added. That covers lines lost to a crash or kill between
  rsync's rename and our read. The walk is authoritative.
- Deleted folders are not recorded as folders. Their files are, and `move_back` recreates the parents.
  An empty deleted folder is not restored. This is accepted and noted in the UI help.
- Cross-filesystem backups:
  - GNU falls back to copy+unlink when `rename` fails with EXDEV.
  - openrsync is unverified. The e2e test reuses the existing two-base scenario (`~/.cache` and `/tmp`).
  - If openrsync fails there, the job fails before anything is replaced, because rsync backs up just
    before the rename.
- Protection:
  - Kade's own backup roots are always excluded: the remote `~/.cache/kade/` and the local data
    `kade/backups/`, as anchored patterns relative to the source root when they lie inside it.
  - The destination root goes through `backup::guard_remote` / `fs::guard`, so syncing *onto* the home
    folder or `/` is refused.
- Restore, Delete and Purge need no changes. The entries look exactly like those from a transfer.

**Changes to `backup.rs`.**
- `pub fn record(&self, entry)`
- `pub fn backup_dir(&self, session, dest_root) -> AppResult<String>`, which reuses
  `remote_tx_root`/`local_root` and `mirrored`
- `pub async fn reconcile(&self, session, backup_dir, dest_root)`

## UI flow

1. The folder context menu in either pane gets "Sync to server…" (local pane) or "Sync from server…"
   (remote pane). Both are disabled with a reason until detection passes.
   - The target defaults to the other pane's current folder joined with the folder name. This is the same
     as a drop.
2. `SyncDialog.svelte` (`Modal.svelte`, registered in `modals.ts`) shows:
   - source → destination, with the destination editable
   - [ ] "Delete files on the destination that aren't in the source (backed up)"
   - [ ] "Compare contents" (`-c`)
   - excludes (one per line, prefilled with `.DS_Store`)
   - a **Preview** button
3. Preview runs the dry run: a spinner, then a cancel button after 2 s.
   - The changes are grouped into New, Updated, Deleted and Skipped (symlinked files), with counts and
     total bytes, in a virtualised list capped at 5,000 rows.
   - "Nothing to do" closes the dialog.
4. **Sync** queues a job in the transfer queue:
   - `Progress.kind = "sync"`, a new serde-defaulted field, so the queue shows a sync icon and hides Pause.
   - Cancel works.
   - When the job finishes, the queue shows the `backup_id` link and `onFinished` refreshes the
     destination pane, as for transfers.
5. The real run is authoritative. The preview is not replayed with `--files-from`, and `--max-delete`
   guards the destructive part.

**Progress.**
- `files_total`/`bytes_total` come from the preview: the sum of `%l` over `>f`/`<f`.
- `files_done`/`bytes_done` advance per completed out-format line.
- `speed` comes from the bridge's wire-byte counter, smoothed by `Reporter::add_bytes`, which is reused.
- A big single file shows no byte progress until it completes. That is acceptable for v1. `--progress`
  parsing is a later best-effort improvement, because the two formats differ.

**Cancel.**
- SIGTERM to the local rsync, so it removes its temp file. Then close the channel; the remote rsync
  gets EOF and its cleanup removes its temp file.
- SIGKILL after 5 s.
- Then reconcile and commit, as on failure.
- `tokio::process::Child::start_kill` only sends SIGKILL, so SIGTERM needs `libc::kill`. Add `libc` as a
  direct dependency; it is already transitive.

**Jobs.** Sync jobs share `Transfers.slots` and the `Ctl` map. `Transfers::pause` is a no-op for them.

## Error cases

| Case | Behaviour |
|------|-----------|
| No local rsync / too old | Menu disabled: "Install rsync (e.g. `brew install rsync`)" |
| Remote has no rsync / exec refused | Menu disabled with the reason. Normal upload still works |
| Session drops mid-run | The channel closes, so rsync exits with 12 or 255 → Failed. Reconcile is skipped (no session); streamed entries are committed |
| Exit 23 (partial) | Failed, with the last stderr lines. What was replaced is restorable |
| Exit 24 (source files vanished) | Done, with a warning toast |
| Exit 25 (`--max-delete` hit) | Failed: "the source changed since the preview; preview again" |
| Exit 30/35 (timeout) | Failed: "the server stopped responding" |
| Relay never connects (5 s) or wrong token | Kill rsync, Failed: "rsync couldn't reach Kade". Log the details |
| Unknown itemize line | Ignored for the UI, still counted by reconcile |
| Name with a newline / non-UTF-8 | Decode `\#ooo` escapes. Display is lossy, the paths stay exact |
| Destination is under a backup root, or is home or `/` | Refused before the preview |
| Disk full in the backup dir | rsync fails that file (23). The original stays in place, because the backup comes before the rename |

## Security considerations

- **Relay socket.** Only the relay started for this run may use it:
  - It lives in a 0700 run dir.
  - Peer uid check.
  - A 256-bit per-run token from `uuid`/`rand`, sent via env (not argv, which `ps` shows).
  - Single connection; the listener closes after the first accept or after 30 s.
  - The token is bound to one run spec, so a stolen token can only run *that* sync again within the
    window, never an arbitrary command.
- **Remote command.** The bridge never runs rsync's argv as given:
  - The argv must be `<host> rsync --server [--sender] <options…> . KADE_REMOTE`.
  - Every option must match the allowlist derived from our own flags table: short-flag clusters made
    of `rltKbinc8` and nothing else, plus GNU's trailing capability string (`e.LsfxCIvu`-style, after
    an `e` in the cluster) and a fixed set of `--long` options.
  - The placeholder is replaced with `shlex::try_quote(real_remote_path)`. The remote path comes from the
    job spec, never from rsync. This sidesteps openrsync's lack of `--protect-args` and GNU ≥ 3.2.4's
    escaping differences.
  - The rsync binary path from detection is quoted the same way.
  - Paths containing NUL or `\n` are refused.
- **Local argv.** Kade spawns rsync with `Command::args`, with no shell. Paths are absolute, so they can't
  be read as options or as `host:path`. User excludes go into `--exclude-from`, never onto argv. Lines are
  validated: no `+ `/`- `/`!` rule prefixes and no `merge`, so an exclude can't become an include or a
  merge directive.
- **Downloads from a hostile server.** Pull mode trusts the server's file list. Mitigations:
  - `-l` without `-L`/`-k`, so links are copied, never followed on the local side.
  - The `-K` risk is only links the user already has.
  - The warning above for GNU < 3.4.0.
  - Kade's own backup roots excluded.
- **Environment.** rsync is spawned through `hostenv::command`, with `RSYNC_RSH`, `RSYNC_CONNECT_PROG`
  and `RSYNC_PROXY` removed, so a user env can't bypass the relay.
- **Logs.** The token never appears in logs or error strings. The remote command is logged with paths
  only at debug level.

## Phased implementation

0. **Benchmark** (above). No product code; the bench test lives under `#[ignore]` in a new
   `src-tauri/src/bench.rs`, or in a scratch crate.
1. **Detection.** `src-tauri/src/rsync/mod.rs`, `rsync/detect.rs` (version parsing, local discovery,
   remote probe). `lib.rs`: command `rsync_support(session_id)`, plus a cache in `AppState`.
   `src/lib/api.ts`: type and call. The push-mode itemize spike for openrsync goes here.
2. **Relay and bridge.**
   - `src-tauri/src/main.rs`: the `--rsync-relay` branch.
   - `rsync/relay.rs`: the client, using std only (no Tauri).
   - `rsync/bridge.rs`: listener, auth, argv validation, command build, exec channel, byte counter.
   - Unit tests for validation and quoting.
   - Exit check: the relay throughput re-benchmark.
3. **Preview.**
   - `rsync/args.rs`: flag building from `SyncSpec`.
   - `rsync/itemize.rs`: line parser and escape decoding.
   - `rsync/preview.rs`: dry run plus the symlink-at-destination check.
   - Command `sync_preview`.
4. **Run and backups.**
   - `rsync/run.rs`: spawn, parse, Progress, cancel.
   - `backup.rs`: `record`, `backup_dir`, `reconcile`.
   - `transfer.rs`: `Progress.kind`; make `Ctl`/`Reporter`/`slots` `pub(crate)` for reuse; pause no-op.
   - `lib.rs`: `sync_start`. `Cargo.toml`: `libc`.
5. **UI.**
   - `src/lib/components/SyncDialog.svelte` (new)
   - `src/lib/modals.ts`
   - `FilePane.svelte` (context menu)
   - `TransferQueue.svelte` (kind icon, no pause)
   - `src/lib/api.ts`
   - `src/lib/locales/nl.ts` (all strings via `tr!`/`t()`, en+nl)
6. **E2E and docs.** `src-tauri/src/e2e.rs` scenarios and the README section.

## Test plan

**Unit tests (`cargo test`).**
- Version parsing fixtures: openrsync 29, samba 2.6.9, GNU 3.2.7, 3.4.1, garbage → unsupported.
- Itemize parser:
  - `>f+++++++`, `>f.st....`, `>f..T......`, `<f.s.....`, `cd+++++++`, `.d..t....`
  - `*deleting dir/`
  - `cL+++++++ a -> b`
  - names with spaces, `\#012`, trailing `/`
  - both 9-column (protocol 29) and 11-column (GNU 3.x) codes
- argv validation: accepts our own server argv for push and pull from each implementation. These are
  recorded fixtures, captured by pointing `-e` at a script that dumps `$@`. It rejects extra paths,
  `--rsync-path`, `-e`, `;`, and anything after the placeholder.
- Remote command quoting: spaces, `'`, `$(…)`, backticks, a leading `-`. Newline and NUL are refused.
- Backup mapping: `backup_dir` + rel equals `mirrored()`/`local_copy_path` for the same original.
  Reconcile adds missing files and does not duplicate streamed ones.
- Exclude-file validation and the backup-root excludes, both when the source contains them and when it
  doesn't.

**E2E (`#[ignore]`, `KADE_E2E_PORT`/`KADE_E2E_KEY`, local sshd, as in `e2e.rs`).** Run them as a matrix
over the local client (`/usr/bin/rsync`, plus Homebrew GNU when present, via `KADE_RSYNC`) and over both
backup bases (`~/.cache` and `/tmp`).
1. Sync to server: edit 2, add 1, delete 1 with `--delete-after`. Check the destination tree, then
   Restore and check the original. A second Restore is refused.
2. Sync from server: the same, mirrored.
3. Preview equals the real run's change list. A no-change run records no transaction.
4. Mode kept: a 0600 and a 0755 destination file stay so after an update. New executables keep +x.
5. Symlinks: a dest dir symlink survives (`-K`). A dest file symlink is skipped and reported, and its
   target is untouched.
6. Hostile names: `a b`, `it's`, `$(touch /tmp/pwned)`, `-rf`, unicode. They sync correctly and
   `/tmp/pwned` never appears.
7. Cancel mid-run on a 200 MB file: no `.name.*` temp files left on either side, the job is Cancelled,
   and the partial transaction is restorable.
8. `--max-delete`: add files to the destination after the preview, so the real run fails with 25 and
   deletes nothing extra.
9. Relay auth: a connection with a wrong token or a second connection is refused, and rsync fails
   cleanly.

## Open questions

1. `Op::Sync` as a new backup op, for a clearer label in BackupsDialog, versus `Op::Overwrite` with a
   "Sync …" summary. A new variant makes an older Kade fail to parse `index.json` after a downgrade.
   Leaning towards `Overwrite` for v1.
2. Prefer Homebrew GNU rsync over openrsync when both exist? It gives better behaviour and diagnostics,
   but different results on different machines.
3. Should excludes and "delete" be remembered per (connection, local folder, remote folder) pair? That
   would make "Sync again" one click, and would need a store field.
4. ~~Unverified openrsync behaviour~~ — mostly settled, see below. Still open: `--backup-dir` across
   filesystems (this Mac has one volume). Push against a GNU server: settled, see below.
5. Sidecar binary versus re-exec, if WebKit dylib load time or AppImage edge cases show up in phase 2.
6. Pause via SIGSTOP on the local rsync plus not reading the channel. It is feasible if no `--timeout`
   is set. Is it worth it in v2?
7. Should "Compare contents" (`-c`) be the default for "from server", where mtimes on the server are
   less trustworthy (deploy tools touching files)?

### Answers from the spikes (macOS 26, `/usr/bin/rsync` = openrsync, protocol 29, both sides)

Tested by hand against a user-mode sshd and by the e2e suite. GNU rsync 3.5.1 (Homebrew, protocol 33) was
added later and the whole suite re-run as a client x server matrix; see "Verified against GNU rsync 3.5.1".

- **Modes without `-p`: kept.** An updated file keeps its destination mode (0600 stays 0600). A new file
  gets the source's mode. **Deviation:** `--executability` is *not* used. With it, an existing 0755 file
  whose source is 0644 loses its x bit (GNU documents the same), which breaks "the destination keeps its
  mode". Without it, new executables still arrive executable. e2e scenario 4 checks this.
- **Temp file on SIGTERM: removed** on both sides (pull: by the local rsync; push: by the server's rsync
  once the channel closes). Exit code 20. e2e scenario 7 checks it through the relay.
- **Itemize in push mode: yes**, `<f…` lines and `*deleting` lines, from an openrsync server.
- **openrsync drops deletes when `--backup-dir` is set.** `--delete*` plus `-b --backup-dir` deletes
  nothing and says nothing (exit 0), in both directions; `-b` without a dir fails on `fchownat`. Dry runs
  still list `*deleting`. **Deviation:** the real run never passes `--delete-after`/`--max-delete`. After
  a successful transfer Kade runs one more dry run with `--delete-after`, checks that every listed delete
  was in the preview (otherwise: Failed, "the source changed since the preview", nothing deleted), and
  moves the top-most deleted items into the same backup transaction with `Recorder::stash`. This works
  the same for GNU, and a deleted folder now comes back whole, empty subfolders included.
- **openrsync's stdout is block-buffered on a pipe**: all lines arrived at exit. rsync's stdout is a
  pseudo-terminal (raw mode) instead, so lines stream. In push mode the sender prints a file's line when
  it *starts* sending it, so `files_done` runs slightly ahead; the streamed journal entry may point at a
  backup that isn't there yet, which `reconcile` drops.
- **openrsync makes its socketpair end non-blocking**, which the relay inherits as stdin/stdout; the relay
  clears `O_NONBLOCK` on fds 0 and 1 before piping.
- **Deletion lines:** openrsync prints `*deleting name` (no length, `%l` ignored); GNU pads the code and
  prints a length. The parser accepts both; a GNU name that starts with "digits space" is ambiguous.
- **Names:** `\#ooo` escapes for control characters (newline → `\#012`), a backslash itself is not
  escaped, `%n` prints no `-> target` for links.
- **Destination missing in a dry run:** openrsync lists files as `<f.......` (no `+`), so the preview
  counts everything as new when the destination folder doesn't exist yet.
- **Symlinked file at the destination** is listed as a *new* file (`<f+++++++`) and would be replaced by
  a regular file; only new files whose folder already existed are checked, one listing per folder.
- **openrsync's server argv** (recorded): separate short flags, `--dirs`, `--log-format=%i` on push,
  `--backup`, and `--backup-dir <value>` as two words, also to a *sender*. The bridge drops it for a
  sender and replaces the value with the job's own folder for a receiver.

### Verified against GNU rsync 3.5.1

Matrix, e2e suite (`e2e_rsync_sync`, both backup bases, plus `e2e_rsync_relay_auth` and
`e2e_rsync_detection_minimal_path`), macOS 26, sshd on 127.0.0.1 with the server's PATH controlled:

| Client | Server | Result |
|--------|--------|--------|
| openrsync | openrsync | all pass |
| GNU 3.5.1 | openrsync | all pass |
| openrsync | GNU 3.5.1 | all pass; scenario 5 hits the `-K` limitation below |
| GNU 3.5.1 | GNU 3.5.1 | all pass; scenario 5 hits the `-K` limitation below |

- **GNU server argv** (captured with `-e` pointing at a script that dumps `"$@"`; fixtures in `bridge.rs`):
  one cluster such as `-nlKtre.iLsfxCIvu` (`b` instead of `n` for the real run, `c` with `-c`), `--log-format=%i`
  on push only, `--timeout=120`, `--delete-after`, then `--backup-dir <value>` as two words with spaces
  backslash-escaped (also sent to a *sender*), then `. KADE_REMOTE`. With `--delete-after` the capability
  string loses its `i` (`-nltre.LsfxCIvu`). The earlier fixtures written from source were wrong in three
  ways: option order, `--log-format=%i` on push, and `-K` in the cluster. The allow-list lost
  `--log-format=X`, which nothing sends; it still accepts any letters in the capability string, since GNU
  adds new ones over time.
- **GNU itemize**: `>f.st......`, `cL+++++++++`, 11 columns; deletions as `*deleting   0 name` (length
  always present, so a name like `7 digits name` parses whole). Fixtures in `itemize.rs`.
- **Real run with delete, modes, hostile names, cancel (no temp files left on either side), `--max-delete`
  replacement (source changed after the preview), relay auth**: pass with GNU as client and as server. The
  GNU server keeps modes without `-p` like openrsync does.
- **GNU 3.4+ as receiver can't write into a symlinked folder, even with `-K`.** `rsync -rK l/ r/` with
  `r/shared -> real/` fails with `recv_generator: failed to stat ".../shared/inner.txt": Too many levels of
  symbolic links (62)` (exit 23), also for a plain local GNU rsync, with relative or absolute link targets and
  with `--no-inc-recursive`. Likely the symlink-race fix (`secure_relative_open`, `O_NOFOLLOW` on every
  component). Without `-K` the link would be replaced by a real folder, so it can't be worked around with flags.
  Effect in Kade: the dry run already fails with that message, nothing is changed. A pull whose local client
  is GNU 3.4+ should behave the same; not tested. e2e scenario 5 expects this failure for a GNU 3.4+
  server. Possible follow-up: detect folder symlinks at the destination during the preview and offer
  "use a normal upload" for them.
- **Detection**: with a GUI-like `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, `/opt/homebrew/bin/rsync` is found and
  preferred over openrsync. `/usr/local/bin/rsync` shares the code path; not testable here (Apple Silicon).
- The server's rsync is whatever `command -v rsync` prints in the SSH exec environment, so a Homebrew rsync
  is only used when the remote PATH has it.

Open question 1 is settled as `Op::Overwrite` with the summary "Sync of X to Y". Question 2: GNU 3.x
is preferred when both exist (`detect::local`). The relay socket token is three v4 UUIDs (366 random bits).

