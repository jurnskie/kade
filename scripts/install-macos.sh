#!/usr/bin/env bash
# Install (or update) Kade on macOS.
#
#   ./scripts/install-macos.sh                   download and install the latest release
#   ./scripts/install-macos.sh --version 0.4.0   a specific version
#   ./scripts/install-macos.sh --source          build from source (inside a clone)
#   ./scripts/install-macos.sh --no-deps         don't check or install dependencies
#   ./scripts/install-macos.sh --no-open         don't start Kade afterwards
#
# Without a clone:
#   curl -fsSL https://github.com/jurnskie/kade/releases/latest/download/install-macos.sh | bash
#
# Running it again updates Kade. Nothing is removed except the old Kade.app.

set -euo pipefail

GH_REPO="jurnskie/kade"
MODE=release
VERSION=""
INSTALL_DEPS=1
OPEN_AFTER=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --source) MODE=source ;;
    --version)
      VERSION="${2:-}"
      [[ -n $VERSION ]] || { echo "--version needs a version, e.g. 0.4.0" >&2; exit 1; }
      shift
      ;;
    --no-deps) INSTALL_DEPS=0 ;;
    --no-open) OPEN_AFTER=0 ;;
    -h | --help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "Unknown option: $1 (see --help)" >&2
      exit 1
      ;;
  esac
  shift
done

bold=$'\033[1m'
green=$'\033[32m'
yellow=$'\033[33m'
red=$'\033[31m'
reset=$'\033[0m'
step() { printf '\n%s==> %s%s\n' "$bold" "$1" "$reset"; }
ok() { printf '  %s✓%s %s\n' "$green" "$reset" "$1"; }
warn() { printf '  %s!%s %s\n' "$yellow" "$reset" "$1"; }
die() {
  printf '\n%sError:%s %s\n' "$red" "$reset" "$1" >&2
  exit 1
}
has() { command -v "$1" >/dev/null 2>&1; }
ask() { # ask "question" -> 0 on yes
  local reply
  read -r -p "  $1 [y/N] " reply </dev/tty || return 1
  [[ $reply =~ ^[yY] ]]
}
load_brew() { # put Homebrew in PATH (Apple Silicon or Intel)
  local prefix
  for prefix in /opt/homebrew /usr/local; do
    if [[ -x "$prefix/bin/brew" ]]; then
      eval "$("$prefix/bin/brew" shellenv)"
      return 0
    fi
  done
  return 0
}
need_brew() {
  load_brew
  if has brew; then return 0; fi
  warn "Homebrew is not installed (needed to install missing tools)."
  ask "Install Homebrew now from brew.sh?" || die "install Homebrew and run this script again."
  # </dev/tty: when this script itself is piped into bash, stdin is the script.
  /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)" </dev/tty
  load_brew
  has brew || die "Homebrew is not in PATH after installing it."
  ok "Homebrew installed"
}

[[ $(uname -s) == Darwin ]] || die "this script is for macOS. On Linux, use install-linux.sh."

# When run from a clone, the repo root is one level above this script.
REPO=""
if [[ -n ${BASH_SOURCE[0]:-} && -f "${BASH_SOURCE[0]}" ]]; then
  candidate="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  if [[ -f "$candidate/src-tauri/tauri.conf.json" ]]; then REPO="$candidate"; fi
fi
if [[ $MODE == source && -z $REPO ]]; then
  die "--source only works inside a clone of $GH_REPO."
fi

# ---------------------------------------------------------------------------
if [[ $INSTALL_DEPS == 1 ]]; then
  step "Checking dependencies"

  if [[ $MODE == source ]]; then
    if xcode-select -p >/dev/null 2>&1; then
      ok "Xcode Command Line Tools"
    else
      warn "Xcode Command Line Tools are missing; opening the installer."
      xcode-select --install || true
      die "finish the installation in that window, then run this script again."
    fi
    need_brew
    ok "Homebrew"

    if [[ -f "$HOME/.cargo/env" ]]; then source "$HOME/.cargo/env"; fi
    if has mise; then eval "$(mise env -s bash 2>/dev/null || true)"; fi
    if has cargo; then
      ok "Rust ($(rustc --version | cut -d' ' -f2))"
    elif has mise; then
      mise use -g rust@stable
      eval "$(mise env -s bash)"
      ok "Rust installed with mise"
    else
      brew install rustup
      rustup-init -y --no-modify-path
      source "$HOME/.cargo/env"
      ok "Rust installed with rustup"
    fi

    node_major=0
    if has node; then node_major=$(node -p 'process.versions.node.split(".")[0]'); fi
    if ((node_major >= 22)); then
      ok "Node $(node -v)"
    else
      brew install node
      ok "Node $(node -v) installed"
    fi

    if has pnpm; then
      ok "pnpm $(pnpm -v)"
    else
      brew install pnpm
      ok "pnpm installed"
    fi
  fi

  # 1Password CLI is optional: only for "Password from 1Password".
  if has op; then
    ok "1Password CLI"
  elif ask "Install the 1Password CLI (for FTP passwords from 1Password)?"; then
    need_brew
    brew install --cask 1password-cli
    ok "1Password CLI installed"
  else
    warn "Skipped the 1Password CLI; signing in with 1Password SSH keys works without it."
  fi
fi

# ---------------------------------------------------------------------------
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [[ $MODE == release ]]; then
  step "Downloading the release${VERSION:+ (v$VERSION)}"
  if [[ -n $VERSION ]]; then
    base="https://github.com/$GH_REPO/releases/download/v${VERSION#v}"
  else
    base="https://github.com/$GH_REPO/releases/latest/download"
  fi
  zip="$WORK/Kade-macos-universal.zip"
  if ! curl -fsSL --proto '=https' -o "$zip.sha256" "$base/Kade-macos-universal.zip.sha256" 2>"$WORK/err" ||
    ! curl -fsSL --proto '=https' -o "$zip" "$base/Kade-macos-universal.zip" 2>>"$WORK/err"; then
    if [[ -n $REPO ]]; then
      warn "No release found; building from source instead."
      rm -rf "$WORK"
      exec "$0" --source $([[ $INSTALL_DEPS == 0 ]] && echo --no-deps) $([[ $OPEN_AFTER == 0 ]] && echo --no-open)
    fi
    die "download failed: $(tail -n 1 "$WORK/err")"
  fi
  (cd "$WORK" && shasum -a 256 -c "$(basename "$zip").sha256" >/dev/null) || die "checksum mismatch; not installing this download."
  ok "$(basename "$zip") (checksum OK)"
  ditto -x -k "$zip" "$WORK/out"
  APP_SRC="$WORK/out/Kade.app"
else
  step "Building Kade (the first time takes a few minutes)"
  cd "$REPO"
  pnpm install --frozen-lockfile
  pnpm tauri build --bundles app
  APP_SRC="$REPO/src-tauri/target/release/bundle/macos/Kade.app"
fi
[[ -d "$APP_SRC" ]] || die "Kade.app is missing after $([[ $MODE == release ]] && echo downloading || echo building)."

# ---------------------------------------------------------------------------
step "Installing"
if [[ -w /Applications ]]; then
  DEST_DIR=/Applications
else
  DEST_DIR="$HOME/Applications"
  mkdir -p "$DEST_DIR"
fi
DEST="$DEST_DIR/Kade.app"
version_of() { defaults read "$1/Contents/Info" CFBundleShortVersionString 2>/dev/null || echo "?"; }
old=""
if [[ -d $DEST ]]; then old=$(version_of "$DEST"); fi

# A running Kade would keep the old binary in use.
if pgrep -ixq kade; then
  osascript -e 'quit app "Kade"' >/dev/null 2>&1 || true
  sleep 1
fi
rm -rf "$DEST"
ditto "$APP_SRC" "$DEST"
# Not signed with an Apple Developer ID: make sure Gatekeeper doesn't block it.
xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true
new=$(version_of "$DEST")
if [[ -n $old && $old == "$new" ]]; then
  ok "Reinstalled Kade $new in $DEST"
elif [[ -n $old ]]; then
  ok "Updated from $old to $new in $DEST"
else
  ok "Installed Kade $new in $DEST"
fi

if [[ -z $old ]]; then
  cat <<EOF

${bold}Done.${reset} Optional, in 1Password → Settings → Developer:
  • "Use the SSH agent"               → sign in with your SSH keys
  • "Integrate with 1Password CLI"    → FTP passwords from 1Password

To share connections between computers: Kade → Settings → Sync, and pick
a folder your sync client (iCloud Drive, Syncthing, …) keeps in step.
EOF
fi

if [[ $OPEN_AFTER == 1 ]]; then
  open "$DEST"
fi
