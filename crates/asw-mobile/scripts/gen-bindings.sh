#!/usr/bin/env bash
# Generate UniFFI bindings from the host build of asw-mobile.
# Usage: gen-bindings.sh <debug|release> <swift|kotlin> <out-dir>
set -euo pipefail
profile="$1"; lang="$2"; out="$3"
case "$(uname -s)" in
  Darwin) lib="target/$profile/libasw_mobile.dylib" ;;
  *)      lib="target/$profile/libasw_mobile.so" ;;
esac
flag=""; [ "$profile" = "release" ] && flag="--release"
cargo build $flag -p asw-mobile
# The crate's uniffi.toml (module and package names) is picked up from the
# crate root automatically; --no-format skips the optional ktlint pass.
cargo run $flag -p asw-mobile --bin uniffi-bindgen -- generate \
  --library "$lib" --language "$lang" --out-dir "$out" --no-format
