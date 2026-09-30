# asw-mobile

Swift and Kotlin bindings for the auto-sea-way routing graph. Four calls over a
memory-mapped v4 graph file:

    installGraph(source, destination)
    openGraph(path) -> Graph
    Graph.version() -> String
    Graph.isWater(lat, lon) -> Water (.water | .land | .unknown)
    Graph.route(fromLat, fromLon, toLat, toLon, shoreBufferNm, arctic = false) -> Route

`arctic` opens the Northern Sea Route and the Northwest Passage; the sea north
of 80°N is always closed. `Route` has `coordinates` (latitude, longitude), `distanceNm` (water only) and
`landLegs` (indices of segments that cross land). Errors are `AswError`:
`NotFound`, `BadFormat`, `InvalidArgument`, `NoRoute`, `Internal`, the last four
carrying a `detail` string. A panic inside the graph code surfaces as `Internal`
(or `.unknown` from `isWater`).

## Installing and updating the graph

Every release attaches `asw.graph.zst` (about 540 MB for the planet), the
zstd-compressed graph, and lists its checksum in `SHA256SUMS`. Download it,
check the checksum, then call:

    installGraph(source: downloadedZstPath, destination: graphPath)

`installGraph` decompresses into a temporary file next to `destination`
(about 8 MB of memory, not the file size), verifies the zstd checksum and the
graph header, and only then renames it over `destination` atomically. On
failure the old file is untouched and nothing is left behind. A `Graph`
already open on the old file keeps working until released; the next
`openGraph` sees the new one. Delete the download afterwards.

The file is memory-mapped, so never truncate, overwrite or delete it in place
while a `Graph` holds it: the operating system raises a bus error on the next
read, which no library can catch, and the app is terminated. `installGraph`
is the safe way to replace it.

## iOS

Every release attaches `AswMobile-<version>.zip` containing `AswMobile.xcframework`
(arm64 device and simulator, iOS 17+) and `AswMobile.swift`. Declare a binary
target with the asset URL and its checksum from `SHA256SUMS`, and add
`AswMobile.swift` to your app target.

## Android

Every release attaches `asw-mobile-<version>.aar` (arm64-v8a, minSdk 21, Android
15 page alignment). Fetch it for your pinned version into `libs/` and add
`net.java.dev.jna:jna:5.14.0@aar` alongside it; the generated Kotlin lives in
package `org.autoseaway.mobile`.

## Memory and timing

Measured through the binding on the 1.44 GB planet file (macOS, Apple Silicon;
first line cold, second with the file in the page cache):

| Step | Time | Resident memory added |
| --- | --- | --- |
| openGraph | 0.5 ms cold, 0.05 ms warm | 0.3 MB |
| isWater | 0.3 ms cold, 0.1 ms warm | 1.6 MB |
| short route (25 nm) | 2.4 ms cold, 0.9 ms warm | 4 MB |
| transatlantic route (3,040 nm) | 0.6 s | 98 MB |

Mapped file pages are clean and evictable. The A* buffers are zero-filled and
allocated on the first route, so resident memory follows the search, not the
graph. All calls block and are safe from any thread; routes serialise on one
buffer set.

## Building locally

`scripts/build-ios.sh <version>` needs Xcode and a rustup toolchain with the
`aarch64-apple-ios` and `aarch64-apple-ios-sim` targets; `scripts/build-android.sh
<version>` needs the Android NDK, `cargo-ndk` and Gradle 8.14 (the Android Gradle
plugin does not support Gradle 9 yet). `scripts/gen-bindings.sh` regenerates the
Swift or Kotlin sources from the host build.
