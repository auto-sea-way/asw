#!/usr/bin/env bash
# Build the iOS xcframework (device + simulator, arm64) and zip it with the
# generated Swift source. Usage: build-ios.sh <version>   (macOS with Xcode)
set -euo pipefail
version="$1"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
# The workspace release profile's thin LTO drops the exported UniFFI entry
# points from the library; the mobile builds turn it off.
export CARGO_PROFILE_RELEASE_LTO=false
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --release -p asw-mobile --target aarch64-apple-ios
cargo build --release -p asw-mobile --target aarch64-apple-ios-sim
# The shipped archives must still export the binding entry points. The
# symbol list is captured first: piping nm into `grep -q` under pipefail
# fails on SIGPIPE, and nm exits non-zero on archive members without symbols.
for a in target/aarch64-apple-ios/release/libasw_mobile.a target/aarch64-apple-ios-sim/release/libasw_mobile.a; do
  syms=$(nm -g "$a" 2>/dev/null || true)
  grep -q uniffi_asw_mobile_fn_func_open_graph <<<"$syms" \
    || { echo "error: $a does not export the binding entry points" >&2; exit 1; }
done
# Bindings come from the host debug build: the release profile strips the
# UniFFI metadata the generator reads, and the API is the same either way.
crates/asw-mobile/scripts/gen-bindings.sh debug swift target/uniffi/swift
rm -rf target/xc && mkdir -p target/xc/Headers target/mobile
cp target/uniffi/swift/AswMobileFFI.h target/xc/Headers/
cp target/uniffi/swift/AswMobileFFI.modulemap target/xc/Headers/module.modulemap
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libasw_mobile.a -headers target/xc/Headers \
  -library target/aarch64-apple-ios-sim/release/libasw_mobile.a -headers target/xc/Headers \
  -output target/xc/AswMobile.xcframework
cp target/uniffi/swift/AswMobile.swift target/xc/
(cd target/xc && rm -f "../mobile/AswMobile-$version.zip" && zip -qr "../mobile/AswMobile-$version.zip" AswMobile.xcframework AswMobile.swift)
echo "wrote target/mobile/AswMobile-$version.zip"
