#!/usr/bin/env bash
# Builds the extension and stages it where Ruby will look for it.
#
# Cargo produces `libstanchion.so`, and Ruby derives an extension's entry point from
# its file name — `libstanchion.so` would have it looking for `Init_libstanchion`.
# Staging it under its own name is what a packaged gem does too.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
profile="${PROFILE:-debug}"

built="$root/target/$profile/libstanchion.so"
[[ "$(uname)" == "Darwin" ]] && built="$root/target/$profile/libstanchion.dylib"

flags=()
[[ "$profile" == "release" ]] && flags+=(--release)
cargo build -p stanchion-ruby --manifest-path "$root/Cargo.toml" "${flags[@]}"

mkdir -p "$here/lib/stanchion"
cp "$built" "$here/lib/stanchion/stanchion.so"
