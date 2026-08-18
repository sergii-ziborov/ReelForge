#!/usr/bin/env bash
# Local publish helper (requires CARGO_REGISTRY_TOKEN). Prefer GitHub Actions.
# Usage: scripts/publish-crates.sh [version]
# Default version is workspace.package.version from Cargo.toml.
set -euo pipefail
: "${CARGO_REGISTRY_TOKEN:?set CARGO_REGISTRY_TOKEN}"

workspace_version() {
  python3 - <<'PY'
import pathlib, re
text = pathlib.Path("Cargo.toml").read_text(encoding="utf-8")
m = re.search(r"\[workspace\.package\][^\[]*version\s*=\s*\"([^\"]+)\"", text, re.S)
if not m:
    raise SystemExit("workspace.package.version not found")
print(m.group(1))
PY
}

VER="${1:-$(workspace_version)}"

# Dependency order. Keep in sync with .github/workflows/publish.yml.
order=(
  reelforge-core
  reelforge-compose
  reelforge-fx
  reelforge-text
  reelforge-render-graph
  reelforge-sightloom-adapter
  reelforge-project
  reelforge-io
  reelforge
  reelforge-cli
)

for pkg in "${order[@]}"; do
  echo "==== $pkg ${VER} ===="
  cargo publish -p "$pkg" --locked || true
  sleep 20
done
echo "done ${VER}"
