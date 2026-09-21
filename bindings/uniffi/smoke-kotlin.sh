#!/usr/bin/env bash
# Compiles and runs the generated Kotlin bindings on the JVM.
#
# Formatting proves the generated file parses; only `kotlinc` proves it is valid
# Kotlin, and only running it proves JNA finds the library and the callback vtables
# work from the JVM.
#
# Needs a Kotlin compiler and the JNA and coroutines jars. Point KOTLINC and JARS at
# them, or let the defaults find a `mise`-managed toolchain.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
lib="$root/target/debug/libstanchion_uniffi.so"
[[ "$(uname)" == "Darwin" ]] && lib="$root/target/debug/libstanchion_uniffi.dylib"

# A `mise` shim with no version set is on PATH and passes `command -v` while being
# unusable, so ask the compiler to identify itself rather than trusting the lookup.
KOTLINC="${KOTLINC:-kotlinc}"
if ! $KOTLINC -version >/dev/null 2>&1; then
    KOTLINC="mise exec kotlin@2.2.20 -- kotlinc"
fi
if ! $KOTLINC -version >/dev/null 2>&1; then
    echo "no usable Kotlin compiler; set KOTLINC or run: mise install kotlin@2.2.20" >&2
    exit 1
fi
JARS="${JARS:-$here/.jars}"
mkdir -p "$JARS"
fetch() {
    local path="$1" file="$JARS/$(basename "$1")"
    [[ -f "$file" ]] || curl -fsSL "https://repo1.maven.org/maven2/$path" -o "$file"
    echo "$file"
}
jna="$(fetch net/java/dev/jna/jna/5.14.0/jna-5.14.0.jar)"
coroutines="$(fetch org/jetbrains/kotlinx/kotlinx-coroutines-core-jvm/1.8.1/kotlinx-coroutines-core-jvm-1.8.1.jar)"

cargo build -p stanchion-uniffi --manifest-path "$root/Cargo.toml"
cargo run -q -p stanchion-uniffi --manifest-path "$root/Cargo.toml" --bin uniffi-bindgen -- \
    generate --library "$lib" --language kotlin --out-dir "$here/generated"

# A jar, not a directory: `-include-runtime` bundles the Kotlin stdlib only when the
# output is an archive, and without it the JVM cannot even load `Function1`.
jar="$here/generated/smoke.jar"
rm -f "$jar"
# shellcheck disable=SC2086
$KOTLINC -nowarn -classpath "$jna:$coroutines" \
    "$here/generated/uniffi/stanchion_uniffi/stanchion_uniffi.kt" "$here/tests/Smoke.kt" \
    -include-runtime -d "$jar"

exec java -cp "$jar:$jna:$coroutines" \
    "-Djna.library.path=$(dirname "$lib")" SmokeKt
