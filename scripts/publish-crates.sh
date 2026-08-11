#!/usr/bin/env bash
# Local publish helper (requires CARGO_REGISTRY_TOKEN). Prefer GitHub Actions.
set -euo pipefail
: "${CARGO_REGISTRY_TOKEN:?set CARGO_REGISTRY_TOKEN}"
VER="${1:-0.1.0}"
order=(reelforge-core reelforge-compose reelforge-fx reelforge-io reelforge-text reelforge reelforge-cli)
for pkg in "${order[@]}"; do
  echo "==== $pkg ===="
  cargo publish -p "$pkg" --locked || true
  sleep 20
done
echo "done $VER"
