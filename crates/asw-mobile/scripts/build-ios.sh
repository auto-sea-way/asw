#!/usr/bin/env bash
# Build the iOS xcframework (device + simulator, arm64) and zip it with the
# generated Swift source. Usage: build-ios.sh <version>   (macOS with Xcode)
set -euo pipefail
version="$1"
export IPHONEOS_DEPLOYMENT_TARGET=17.0
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build --release -p asw-mobile --target aarch64-apple-ios
cargo build --release -p asw-mobile --target aarch64-apple-ios-sim
crates/asw-mobile/scripts/gen-bindings.sh release swift target/uniffi/swift
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
