#!/usr/bin/env bash
# Builds the library, regenerates the bindings, and runs the generated boundary.
#
# The generator is built from this crate so it always matches the `uniffi` the
# library was compiled against; a mismatch produces bindings that compile and then
# misbehave, which is the worst way to find out.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
lib="$root/target/debug/libstanchion_uniffi.so"
[[ "$(uname)" == "Darwin" ]] && lib="$root/target/debug/libstanchion_uniffi.dylib"

cargo build -p stanchion-uniffi --manifest-path "$root/Cargo.toml"
cargo run -q -p stanchion-uniffi --manifest-path "$root/Cargo.toml" --bin uniffi-bindgen -- \
    generate --library "$lib" \
    --language kotlin --language swift --language python \
    --out-dir "$here/generated"

# The generated Python loads the cdylib from its own directory.
cp "$lib" "$here/generated/"
PYTHONPATH="$here/generated" exec python3 "$here/tests/smoke.py"
