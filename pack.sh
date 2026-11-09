#!/bin/sh
# Builds build/rust_voice-<version>.hnmod from this repository.
# The .hnmod holds horizon-native.json, the bundle's .rs and .toml files and LICENSE, nothing else
# (the same files as horizon-mods' `native.py package`).
# Usage: sh pack.sh
set -e
cd "$(dirname "$0")"

ID=rust_voice
B="mixins/$ID"
VER=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$B/api/Cargo.toml" | head -1)
MVER=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$B/horizon-mixin.toml" | head -1)
[ -n "$VER" ] || { echo "no version in $B/api/Cargo.toml"; exit 1; }
[ "$VER" = "$MVER" ] || { echo "version mismatch: Cargo.toml $VER, horizon-mixin.toml $MVER"; exit 1; }
command -v zip >/dev/null || { echo "zip is required"; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required"; exit 1; }

REV=$(sed -n 's/^horizon_revision *= *"\(.*\)"/\1/p' "$B/horizon-mixin.toml")
python3 - "$REV" <<'PY'
import json, sys
c = json.load(open("compatibility.json"))
if c["horizon_revision"] != sys.argv[1]:
    sys.exit("horizon_revision differs between compatibility.json and horizon-mixin.toml")
PY

OUT="$(pwd)/build/$ID-$VER.hnmod"
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

mkdir -p "$T/$B/api/src"
cp "$B/horizon-mixin.toml" "$T/$B/"
cp "$B/api/Cargo.toml" "$T/$B/api/"
cp "$B"/api/src/*.rs "$T/$B/api/src/"
# the licence travels with the sources (the launcher accepts LICENSE* in the bundle root)
cp LICENSE "$T/$B/LICENSE"

python3 - "$ID" "$VER" > "$T/horizon-native.json" <<'PY'
import json, sys
c = json.load(open("compatibility.json"))
print(json.dumps({
    "game": c["game"],
    "hooks": c["hooks"],
    "horizon": c["horizon_revision"],
    "id": sys.argv[1],
    "mixins": c["horizon_mixins_revision"],
    "schema": 1,
    "version": sys.argv[2],
}, indent=2, sort_keys=True))
PY

# same input -> same file: fixed timestamps, sorted entries, no extra attributes
find "$T" -exec touch -h -d "2026-01-01 00:00:00" {} +
mkdir -p build
rm -f "$OUT"
(cd "$T" && find horizon-native.json mixins | LC_ALL=C sort | zip -q -X "$OUT" -@)
echo "built: $OUT"
command -v sha256sum >/dev/null && sha256sum "$OUT"
