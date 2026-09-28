#!/usr/bin/env bash
# Build the Android AAR: arm64-v8a shared library + generated Kotlin.
# Usage: build-android.sh <version>   (needs ANDROID_NDK_HOME, cargo-ndk, gradle)
set -euo pipefail
version="$1"
rustup target add aarch64-linux-android
command -v cargo-ndk >/dev/null || cargo install cargo-ndk --version 4.1.2 --locked
rm -rf target/jniLibs
# 16 KB page alignment for Android 15 devices.
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo ndk --platform 21 -t arm64-v8a -o target/jniLibs build --release -p asw-mobile
crates/asw-mobile/scripts/gen-bindings.sh release kotlin target/uniffi/kotlin
(cd crates/asw-mobile/android && gradle --no-daemon assembleRelease)
mkdir -p target/mobile
cp crates/asw-mobile/android/build/outputs/aar/asw-mobile-release.aar "target/mobile/asw-mobile-$version.aar"
echo "wrote target/mobile/asw-mobile-$version.aar"
