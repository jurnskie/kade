#!/usr/bin/env bash
# Install (or update) Kade on Linux, as an AppImage.
#
#   ./scripts/install-linux.sh                  the latest release
#   ./scripts/install-linux.sh --version 0.4.0  a specific version
#
# Without a clone:
#   curl -fsSL https://github.com/jurnskie/kade/releases/latest/download/install-linux.sh | bash
#
# Installs to ~/.local/bin/kade with a shortcut in your app launcher.
# No root needed. Debian/Ubuntu users can also use the .deb from the release.

set -euo pipefail

GH_REPO="jurnskie/kade"
VERSION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      [[ -n $VERSION ]] || { echo "--version needs a version, e.g. 0.4.0" >&2; exit 1; }
      shift
      ;;
    -h | --help)
      sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
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

[[ $(uname -s) == Linux ]] || die "this script is for Linux; on macOS, use install-macos.sh."
[[ $(uname -m) == x86_64 ]] || die "there is only an x86_64 build; on $(uname -m), build it yourself: pnpm install && pnpm tauri build."

step "Checking dependencies"
has curl || die "curl is missing; install it and run this script again."

# AppImages mount themselves with FUSE 2; without it they can still run by
# unpacking first (slower start), so this is a warning, not an error.
FUSE_OK=1
if ! { ldconfig -p 2>/dev/null | grep -q 'libfuse\.so\.2'; } && ! ls /usr/lib*/libfuse.so.2* >/dev/null 2>&1; then
  FUSE_OK=0
  if has pacman; then hint="sudo pacman -S fuse2"
  elif has apt; then hint="sudo apt install libfuse2t64  (libfuse2 on older releases)"
  elif has dnf; then hint="sudo dnf install fuse-libs"
  else hint="install FUSE 2 (libfuse.so.2)"; fi
  warn "FUSE 2 is missing, so Kade starts a little slower. To fix: $hint"
else
  ok "FUSE 2"
fi
if has op; then ok "1Password CLI"; else warn "1Password CLI not found; only needed for FTP passwords from 1Password."; fi

step "Downloading the release${VERSION:+ (v$VERSION)}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
if [[ -n $VERSION ]]; then
  tag="v${VERSION#v}"
else
  # /releases/latest redirects to /releases/tag/<tag>.
  latest=$(curl -fsSLI --proto '=https' -o /dev/null -w '%{url_effective}' "https://github.com/$GH_REPO/releases/latest") ||
    die "can't reach GitHub."
  tag="${latest##*/}"
  [[ $tag == v* ]] || die "no release found."
fi
new="${tag#v}"
base="https://github.com/$GH_REPO/releases/download/$tag"
image="$WORK/Kade-linux-x86_64.AppImage"
for f in Kade-linux-x86_64.AppImage.sha256 Kade-linux-x86_64.AppImage kade.png; do
  curl -fsSL --proto '=https' -o "$WORK/$f" "$base/$f" 2>"$WORK/err" ||
    [[ $f == kade.png ]] || die "downloading $f failed: $(tail -n 1 "$WORK/err")"
done
(cd "$WORK" && sha256sum -c "$(basename "$image").sha256" >/dev/null) || die "checksum mismatch; not installing this download."
ok "Kade $new (checksum OK)"

step "Installing"
BIN="$HOME/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/256x256/apps"
mkdir -p "$BIN" "$APPS" "$ICONS"

old=""
if [[ -f "$BIN/.kade-version" ]]; then old=$(cat "$BIN/.kade-version"); fi
if pgrep -x kade >/dev/null 2>&1 || pgrep -f "$BIN/kade" >/dev/null 2>&1; then
  warn "Kade is running; the new version starts after a restart."
fi
# Replace atomically, so a running Kade keeps its (old) file.
install -m 755 "$image" "$BIN/kade.new" && mv -f "$BIN/kade.new" "$BIN/kade"
echo "$new" >"$BIN/.kade-version"
if [[ -f "$WORK/kade.png" ]]; then install -m 644 "$WORK/kade.png" "$ICONS/kade.png"; fi

exec_line="$BIN/kade"
if [[ $FUSE_OK == 0 ]]; then exec_line="env APPIMAGE_EXTRACT_AND_RUN=1 $BIN/kade"; fi
cat >"$APPS/kade.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Kade
Comment=SSH, SFTP and FTP manager
Exec=$exec_line
Icon=kade
Terminal=false
Categories=Network;FileTransfer;Development;
StartupWMClass=kade
EOF
if has update-desktop-database; then update-desktop-database "$APPS" >/dev/null 2>&1 || true; fi

if [[ -n $old && $old == "$new" ]]; then
  ok "Reinstalled Kade $new in $BIN/kade"
elif [[ -n $old ]]; then
  ok "Updated from $old to $new"
else
  ok "Installed Kade $new in $BIN/kade, with a shortcut in your app launcher"
  case ":$PATH:" in
    *":$BIN:"*) ;;
    *) warn "$BIN is not in your PATH; starting from the app launcher still works." ;;
  esac
  cat <<EOF

${bold}Done.${reset} For 1Password: turn on the SSH agent and the CLI integration in
1Password → Settings → Developer. To share connections between computers: Kade → Settings → Sync.
EOF
fi
