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
# The shipped library must still export the binding entry points (JNA looks
# them up in the dynamic symbol table at runtime).
nm_tool="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-nm"
"$nm_tool" -D --defined-only target/jniLibs/arm64-v8a/libasw_mobile.so | grep -q uniffi_asw_mobile_fn_func_open_graph \
  || { echo "error: libasw_mobile.so does not export the binding entry points" >&2; exit 1; }
# Bindings come from the host debug build: the release profile strips the
# UniFFI metadata the generator reads, and the API is the same either way.
crates/asw-mobile/scripts/gen-bindings.sh debug kotlin target/uniffi/kotlin
(cd crates/asw-mobile/android && gradle --no-daemon assembleRelease)
mkdir -p target/mobile
cp crates/asw-mobile/android/build/outputs/aar/asw-mobile-release.aar "target/mobile/asw-mobile-$version.aar"
echo "wrote target/mobile/asw-mobile-$version.aar"
