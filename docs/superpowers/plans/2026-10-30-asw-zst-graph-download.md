# asw `.zst` graph download Implementation Plan

**Goal:** `asw serve --graph-url <...>/asw.graph.zst` downloads the compressed graph, unpacks it while downloading, checks it and only then puts it in place.

**Architecture:** One change in `crates/asw-cli/src/download.rs`. When the URL path ends in `.zst`, the HTTP body goes through a zstd stream decoder into the existing temporary file; then the file is opened as a v5 graph before the rename. Plain URLs keep working as today.

**Tech Stack:** Rust, `reqwest` blocking (already in asw-cli), `zstd` (workspace dependency, already used by asw-mobile).

**Spec:** `docs/superpowers/specs/2026-10-30-avnav-plugin-design.md`, section "Install and first start".

## Global Constraints

- Plain `asw.graph` URLs behave exactly as before.
- On any failure the destination path does not exist afterwards and no temporary file is left.
- Check = zstd content checksum (verified by the decoder at end of stream) + `RoutingGraph::open(&tmp, false)` succeeds. No `SHA256SUMS` lookup.
- Run `cargo fmt --all` before pushing. Distances in docs in nautical miles. No consuming-app names anywhere.
- Ships in asw 0.11.0 (released with `graph_tag=v0.10.0`, graph unchanged).

## Review Focus

- A truncated `.zst` (connection drops in the marina): must fail, leave nothing behind, and a restart must start clean. Test: `zst_truncated_leaves_nothing`.
- A `.zst` that is not a graph: must fail at the open check. Test: `zst_not_a_graph_leaves_nothing`.
- URL with a query string or upper case (`.../asw.graph.zst?x=1`): decide on the URL path, case-insensitive. Test: `zst_detected_from_url_path`.
- Server returns 404: error mentions the HTTP status, nothing left behind. Covered by the existing `error_for_status`; add `http_error_leaves_nothing`.
- The musl release builds (`x86_64/aarch64-unknown-linux-musl` via `cross`) must still compile with `zstd-sys` in asw-cli. Checked in Task 1 Step 6.

---

### Task 1: `.zst` support in `ensure_graph`

**Files:**
- Modify: `crates/asw-cli/Cargo.toml` (add `zstd.workspace = true`; dev-dependency none new)
- Modify: `crates/asw-cli/src/download.rs`
- Modify: `README.md` (env table row for `ASW_GRAPH_URL`, slim-image example), `docs/deployment.md` (graph URL examples), `CHANGELOG.md` (Unreleased)

**Interfaces:**
- Produces: `pub fn ensure_graph(path: &Path, url: Option<&str>) -> Result<()>` (unchanged signature); private `fn is_zst(url: &str) -> bool`.
- The temporary file stays `path.with_extension("graph.tmp")`: the AvNav plugin watches its size for download progress.

- [ ] **Step 1: Write the failing tests** in a `#[cfg(test)] mod tests` in `download.rs`. Serve bytes from a one-shot `std::net::TcpListener` thread on `127.0.0.1:0` that writes a minimal `HTTP/1.1 200 OK` (or `404 Not Found`) response with `Content-Length`. Build the fixture graph with `asw_core::graph::GraphBuilder::with_version("test")`, one node, `.build().save(path)` (same pattern as `fixture_graph_path` in `crates/asw-mobile/src/lib.rs`), and compress it with `zstd::stream::Encoder::new(Vec::new(), 3)` plus `include_checksum(true)`, like the release's `zstd --check`.

```rust
#[test] fn zst_download_unpacks_and_opens()      // dest exists, RoutingGraph::open(dest) ok, no .graph.tmp
#[test] fn plain_download_still_works()          // uncompressed bytes saved as-is
#[test] fn zst_truncated_leaves_nothing()        // first half of the .zst → Err, !dest.exists(), !tmp.exists()
#[test] fn zst_not_a_graph_leaves_nothing()      // zstd of b"hello" → Err, !dest.exists(), !tmp.exists()
#[test] fn http_error_leaves_nothing()           // 404 → Err containing "404", !dest.exists()
#[test] fn zst_detected_from_url_path()          // is_zst("http://h/a/asw.graph.ZST?x=1") && !is_zst("http://h/a.zst.graph")
```

- [ ] **Step 2: Run them, expect failures**

Run: `export PATH="$HOME/.cargo/bin:$PATH"; cargo test -p asw-cli download`
Expected: compile error (`is_zst` missing) or the zst tests FAIL.

- [ ] **Step 3: Implement.** In `ensure_graph`, after the response is checked: if `is_zst(url)`, copy through `zstd::stream::read::Decoder::new(resp)`, else copy `resp` directly. After writing, `sync_all`, then `asw_core::graph::RoutingGraph::open(&tmp_path, false)`; on any error from copy or open, remove `tmp_path` and return the error with context ("graph download is corrupt or incomplete"). `is_zst` takes the URL path (strip `?` and `#` parts) and checks `ends_with(".zst")` case-insensitively.

- [ ] **Step 4: Run the tests, expect PASS**

Run: `cargo test -p asw-cli download`
Expected: 6 passed.

- [ ] **Step 5: Manual check against the real release asset** (needs ~2 GB free on `/Volumes/2TB`):

Run: `cargo build --release -p asw-cli && ./target/release/asw serve --graph /Volumes/2TB/Projects/auto-sea-way/export/zst-test/asw.graph --graph-url https://github.com/auto-sea-way/asw/releases/download/v0.10.0/asw.graph.zst --api-key t --port 3999` then `curl -s localhost:3999/ready` → 200, then stop it and `shasum -a 256 export/zst-test/asw.graph` starts with `3dcb8545`. Delete `export/zst-test` afterwards.

- [ ] **Step 6: Check the musl targets still build.** If Docker is available: `cross build --release -p asw-cli --target aarch64-unknown-linux-musl`. If not, note it in the PR and watch the `build` job of the 0.11.0 release run.

- [ ] **Step 7: Docs.** README env row: "URL to download the graph if the file is missing; a `.zst` URL is unpacked and checked while downloading". Switch the slim-image example and `docs/deployment.md` examples to `asw.graph.zst`. CHANGELOG Unreleased: "`--graph-url` / `ASW_GRAPH_URL` accept the compressed `asw.graph.zst` (540 MB instead of 1.4 GB)".

- [ ] **Step 8: Commit and open a PR**

```bash
cargo fmt --all && cargo clippy -p asw-cli -- -D warnings
git add crates/asw-cli README.md docs/deployment.md CHANGELOG.md
git commit -m "Accept a .zst graph URL in asw serve"
```
Then push the branch and open a PR to `main`.
