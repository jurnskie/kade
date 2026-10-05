#!/usr/bin/env bash
# Cut a new version: bump the version, tag and push. GitHub Actions then
# builds macOS and Linux bundles and publishes them as a GitHub release.
#
#   ./scripts/release.sh 0.4.0

set -euo pipefail

version="${1:-}"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Usage: $0 <version>, e.g. 0.4.0" >&2; exit 1; }
cd "$(dirname "${BASH_SOURCE[0]}")/.."

[[ -z $(git status --porcelain) ]] || { echo "There are uncommitted changes." >&2; exit 1; }
git rev-parse -q --verify "refs/tags/v$version" >/dev/null && { echo "Tag v$version already exists." >&2; exit 1; }

current=$(node -p "require('./src-tauri/tauri.conf.json').version")
if [[ $current != "$version" ]]; then
  node -e '
    const fs = require("fs"), v = process.argv[1];
    for (const f of ["package.json", "src-tauri/tauri.conf.json"]) {
      const j = JSON.parse(fs.readFileSync(f, "utf8"));
      j.version = v;
      fs.writeFileSync(f, JSON.stringify(j, null, 2) + "\n");
    }' "$version"
  # Only the [package] version, the first `version =` line.
  sed -i.bak "1,/^version = /s/^version = \".*\"/version = \"$version\"/" src-tauri/Cargo.toml && rm src-tauri/Cargo.toml.bak
  (cd src-tauri && cargo metadata --format-version 1 >/dev/null) # refresh Cargo.lock
  git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
  git commit -m "Kade $version"
fi

git tag -a "v$version" -m "Kade $version"
git push --follow-tags
echo
echo "Building release v$version. Follow along with:  gh run watch -R jurnskie/kade"
