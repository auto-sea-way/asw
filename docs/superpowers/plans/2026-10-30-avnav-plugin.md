# AvNav asw plugin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A zip plugin for AvNav (20261008 and newer, Linux and Raspberry Pi) that runs asw locally and turns the route in AvNav's route editor into a land-avoiding route, saved as a new route that cannot be followed until the sailor marks it as checked.

**Architecture:** `plugin.py` is thin AvNav glue (status, HTTP handler, child process, watcher thread). All logic that can be tested without AvNav lives in a small package `aswplugin/` next to it: route joining and GPX, the unchecked registry, install checks. `plugin.mjs` adds the "Auto route" button and dialog in the route editor and the local-route warning.

**Tech Stack:** Python 3.11+ standard library only (AvNav runs Debian's Python). Browser JavaScript module using AvNav's plugin API. asw 0.11.0 binary and `asw.graph.zst` from the asw GitHub release.

**Spec:** `docs/superpowers/specs/2026-10-30-avnav-plugin-design.md` (in the asw repo). Depends on plan `2026-10-30-asw-zst-graph-download.md` being released as asw 0.11.0.

## Global Constraints

- Repo `auto-sea-way/avnav-plugin`, local checkout `/Volumes/2TB/Projects/avnav-plugin`. **Ask the owner before creating the GitHub repo or pushing.**
- Zip top folder `asw` (plugin shows as `user-asw`); asset name `avnav-asw-<version>.zip`. The folder name never changes.
- Minimum AvNav 20261008. Linux arm64 and x86_64 only.
- asw listens on `127.0.0.1` only, random API key per start, kept in memory.
- The graph download starts only on the sailor's button press. Before it, the status shows "about 540 MB, use Wi-Fi".
- Free space needed before download: 2.1 GB in the data directory. RAM below 2 GB: warning only.
- Route names: `<name> (asw, unchecked)` when generated, `<name> (asw)` after "Mark as checked". Added points: `asw 1`, `asw 2`, ... numbered across the whole route.
- Copy in the dialog and README: the route does not consider depth and is not for navigation; this is not weather routing.
- Distances in nautical miles. No consuming-app names. No AI or tool attribution anywhere in the repo, commits or releases.
- Standard library only in Python; no npm build for `plugin.mjs`.
- License MIT OR Apache-2.0, same files as asw.

## Review Focus

- A Pi with a 64-bit kernel and 32-bit userland (`platform.machine()` says `aarch64`): the static musl arm64 binary still runs, so decide by running `asw --version` after download, not by `platform.machine()` alone. Test: `test_unsupported_machine` plus manual step in Task 4.
- The sailor renames an unchecked route on AvNav's Routes page, or edits its points, then activates it: it must still be stopped. Tests: `test_registry_follows_rename`, `test_registry_follows_edit`.
- A waypoint on land (pin on a quay): the route keeps the land segment and the dialog names that waypoint. Test: `test_land_waypoints_named`.
- One leg has no route (asw 404, for example a lake not connected to the sea): the dialog names the leg, nothing is saved. Test: `test_leg_without_route_reported`.
- AvNav restarts or the plugin is disabled during the graph download: on next start the leftover `asw.graph.tmp` is deleted and the status says "graph missing" again. Test: `test_stale_tmp_removed`.

---

### Task 1: Repo skeleton and route joining

**Files:**
- Create: `asw/plugin.json`, `asw/aswplugin/__init__.py`, `asw/aswplugin/route.py`, `tests/selfcheck.py`, `build.sh`, `LICENSE-MIT`, `LICENSE-APACHE` (copy from asw), `.gitignore`

**Interfaces:**
- Produces in `aswplugin/route.py`:
  - `Point = dict` with keys `lat: float`, `lon: float`, `name: str`
  - `join_legs(waypoints: list[Point], legs: list[dict]) -> tuple[list[Point], float, list[str]]` — `legs[i]` is the asw `/route` JSON for `waypoints[i] → waypoints[i+1]`; returns (points, total distance_nm rounded to 0.1, names of waypoints on land, in order, no duplicates).
  - `route_gpx(name: str, points: list[Point]) -> str`
  - `unchecked_name(name: str) -> str`, `checked_name(name: str) -> str`
  - `points_key(points: list[Point]) -> str` — sha1 of lat/lon rounded to 6 decimals, names ignored.
- `plugin.json`: `{"version": "<plugin version>", "description": "Auto route around land with auto-sea-way", "aswVersion": "0.11.0"}`

- [ ] **Step 1: Write the failing checks** in `tests/selfcheck.py` (plain `assert`, runs with `python3 tests/selfcheck.py`, adds `asw/` to `sys.path`):

```python
def test_join_two_legs():
    # wp A,B,C; leg AB coords A,x,B ; leg BC coords B,y,z,C ; asw coords are [lon, lat]
    pts, nm, land = join_legs(wps, [leg_ab, leg_bc])
    assert [p["name"] for p in pts] == ["A", "asw 1", "B", "asw 2", "asw 3", "C"]
    assert nm == round(leg_ab["distance_nm"] + leg_bc["distance_nm"], 1) and land == []
def test_land_waypoints_named():
    # leg AB land_legs [0] (A on land), leg BC land_legs [last segment] (C on land)
    assert join_legs(wps, [ab, bc])[2] == ["A", "C"]
def test_names():
    assert unchecked_name("Bodrum") == "Bodrum (asw, unchecked)"
    assert checked_name("Bodrum (asw, unchecked)") == "Bodrum (asw)"
def test_points_key_ignores_names():
    assert points_key(p) == points_key([dict(x, name="other") for x in p])
def test_gpx_roundtrip():
    # parse route_gpx output with xml.etree: one <rte>, <name>, rtept lat/lon/name in order
```

- [ ] **Step 2: Run, expect failure** — `python3 tests/selfcheck.py` → `ModuleNotFoundError: aswplugin`.

- [ ] **Step 3: Implement `route.py`.** GPX: GPX 1.1 with one `<rte>` matching what AvNav writes for routes (read `viewer/nav/routeobjects` / `avnrouter.py` in the AvNav source for the exact element names); escape names with `xml.sax.saxutils.escape`.

- [ ] **Step 4: Run, expect PASS** — `python3 tests/selfcheck.py` → prints `ok` and exits 0.

- [ ] **Step 5: `build.sh`** zips `asw/` (excluding `__pycache__`) to `dist/avnav-asw-$(version from plugin.json).zip`. Run it; `unzip -l` shows only `asw/...` entries.

- [ ] **Step 6: Commit** — `git init`, `git add -A`, `git commit -m "Route joining and GPX for the AvNav plugin"`.

### Task 2: Unchecked registry

**Files:**
- Create: `asw/aswplugin/unchecked.py`
- Modify: `tests/selfcheck.py`

**Interfaces:**
- Consumes: `points_key`, `checked_name` from Task 1.
- Produces: `class Registry` with
  - `Registry(path: str)` — JSON file `{"routes": [{"name": str, "key": str}]}`; missing file = empty.
  - `add(name: str, key: str) -> None`, `remove(name: str) -> None` (both save)
  - `is_unchecked(name: str, key: str) -> bool` — true if an entry matches by name **or** by key
  - `sync(routes: dict[str, str]) -> None` — `routes` maps every route file name in the routes dir to its points key. For each entry: if its name is gone but its key is found under another name, take that name (rename); if its name is found with another key, take that key (edit). Saves if changed.

- [ ] **Step 1: Write the failing checks**

```python
def test_registry_follows_rename():
    r.add("A (asw, unchecked)", "k1"); r.sync({"Renamed": "k1"})
    assert r.is_unchecked("Renamed", "k1")
def test_registry_follows_edit():
    r.add("A (asw, unchecked)", "k1"); r.sync({"A (asw, unchecked)": "k2"})
    assert r.is_unchecked("A (asw, unchecked)", "k2")
def test_registry_remove_and_persist():
    r.remove("A (asw, unchecked)"); assert not Registry(path).is_unchecked("A (asw, unchecked)", "k2")
```

- [ ] **Step 2: Run, expect failure.** `python3 tests/selfcheck.py` → `NameError: Registry`.
- [ ] **Step 3: Implement** with `json` and an atomic write (`os.replace` from a temp file in the same directory).
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** — `git commit -am "Unchecked route registry"`.

### Task 3: Install checks

**Files:**
- Create: `asw/aswplugin/install.py`
- Modify: `tests/selfcheck.py`

**Interfaces:**
- Produces:
  - `binary_asset(machine: str) -> str | None` — `aarch64`/`arm64` → `asw-linux-arm64`, `x86_64`/`amd64` → `asw-linux-amd64`, else `None`
  - `release_url(asw_version: str, asset: str) -> str` — `https://github.com/auto-sea-way/asw/releases/download/v{asw_version}/{asset}`
  - `NEED_FREE_BYTES = 2_100_000_000`, `WARN_RAM_BYTES = 2_000_000_000`
  - `preflight(data_dir: str, machine: str, mem_total: int, free: int) -> tuple[list[str], list[str]]` — (errors, warnings); errors stop the download, warnings only show in status
  - `mem_total() -> int` (from `/proc/meminfo`, 0 if unreadable), `cleanup_stale(graph_path: str) -> None` (deletes `<graph>.tmp`, the asw temp name `asw.graph.tmp`)
  - `download_binary(url: str, dest: str) -> None` — `urllib.request` to `dest + ".part"`, `chmod 0o755`, run `[dest_part, "--version"]` with a 10 s timeout; on failure delete and raise `RuntimeError("this system cannot run asw")`; on success `os.replace`.

- [ ] **Step 1: Write the failing checks**

```python
def test_unsupported_machine():
    assert binary_asset("armv7l") is None and binary_asset("aarch64") == "asw-linux-arm64"
    assert preflight("/x", "armv7l", 4e9, 5e9)[0]          # error
def test_preflight_space_and_ram():
    assert preflight("/x", "aarch64", 4e9, 1e9)[0]          # not enough space → error
    errs, warns = preflight("/x", "aarch64", 1e9, 5e9); assert not errs and warns
def test_stale_tmp_removed():
    # create asw.graph.tmp in a temp dir, cleanup_stale(dir/asw.graph) removes it, keeps asw.graph if present
```

- [ ] **Step 2: Run, expect failure.** `python3 tests/selfcheck.py` → `NameError: binary_asset`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Commit** — `git commit -am "Install checks for the AvNav plugin"`.

### Task 4: `plugin.py` — asw process, status and HTTP API

**Files:**
- Create: `asw/plugin.py`

**Interfaces:**
- Consumes: everything from Tasks 1–3.
- AvNav glue: `class Plugin` with `pluginInfo()` (description only, no data keys), `__init__(self, api)`, `run(self)`, `stop(self)`. `plugin.py` puts its own directory first on `sys.path` before `import aswplugin` (AvNav loads it with `importlib.util.spec_from_file_location`).
- Files under `<api.getDataDir()>/asw/`: `asw` (binary), `asw.graph`, `unchecked.json`, `options.json`.
- Produces the HTTP API at `<api.getBaseUrl()>/api/<cmd>` (via `registerRequestHandler`), all JSON:
  - `status` → `{"state": "missing"|"downloading"|"starting"|"ready"|"error", "detail": str, "warnings": [str], "downloadedMB": int}`
  - `download` → starts the download thread, returns `status`
  - `route?data=<json>` with `{"name": str, "points": [Point], "shore_buffer": float, "canals": bool, "arctic": bool}` → `{"ok": true, "name": str, "distance_nm": float, "land": [str], "points": int}` or `{"ok": false, "error": str}` (leg that failed named as `"<from> → <to>: <asw error>"`)
  - `check?name=<route name>` → renames the file to `checked_name`, removes it from the registry
  - `unchecked` → `{"names": [str]}` and `options` / `options?data=<json>` to read and save the dialog values

- [ ] **Step 1: Write the failing check** for the leg error message, which is pure: move the HTTP call behind `aswplugin.route.call_legs(get: Callable[[Point, Point], tuple[int, dict]], waypoints) -> list[dict]` raising `LegError(str)`, and test it with a fake `get`:

```python
def test_leg_without_route_reported():
    get = lambda a, b: (404, {"error": "no route"}) if a["name"] == "B" else (200, leg)
    try: call_legs(get, [A, B, C]); assert False
    except LegError as e: assert str(e) == "B → C: no route"
```

- [ ] **Step 2: Run, expect failure; Step 3: implement `call_legs`; Step 4: run, expect PASS.**

- [ ] **Step 5: Implement `plugin.py`.** Startup: `cleanup_stale`; if binary and graph exist, start asw; else status `missing` with the download hint. Download thread: `preflight` → `download_binary` → start `asw serve --host 127.0.0.1 --port <free port from socket bind 0> --graph <path> --graph-url <release_url(aswVersion, "asw.graph.zst")> --api-key <secrets.token_hex(16)>`; progress = size of `asw.graph.tmp` in MB; poll `/ready` every second; `ready` when 200. Child dies → restart once per 30 s, status `error` with the last stderr line. `registerRestart(self.stop)` kills the child. `route` writes the GPX to `<api.getDataDir()>/routes/<unchecked_name>.gpx` (overwrite is confirmed in the dialog before the call) and adds it to the registry.

- [ ] **Step 6: Manual test on Linux** (container on `/Volumes/2TB`, AvNav from source at the 20261008 tag, see AvNav `newDoc/docs/en/installation/linux.md`): upload the zip, status shows "missing, about 540 MB"; press download; status goes downloading → ready; `curl <base>/api/route?data=...` for Marmaris → Rhodes returns ok and a file appears on the Routes page. Delete the container and data afterwards.

- [ ] **Step 7: Commit** — `git commit -am "Run asw from the AvNav plugin"`.

### Task 5: Watch and stop unchecked routes

**Files:**
- Modify: `asw/plugin.py`

**Interfaces:**
- Consumes: `Registry`, `points_key`.
- Router access in the same process: `AVNWorker.findHandlerByName("AVNRouter")` (from AvNav's `avnav_worker`), then `router.getCurrentLeg()` and `router.setCurrentLeg(None)`. Wrap every call in `try`: if AvNav changes these names, set status warning "cannot watch active route on this AvNav version" and keep routing working. This is the internal-interface risk the spec accepts.

- [ ] **Step 1: Implement the watcher loop in `run()`** (every 2 s): build `{name: points_key}` for all `.gpx` files in the routes dir (parse with `xml.etree`), `registry.sync(...)`; read the current leg's route name and points; if `registry.is_unchecked(...)`: `setCurrentLeg(None)` and `api.startAlarm("asw-unchecked", ...)` with text "This asw route is not checked yet. Check it and press Mark as checked in the Auto route dialog." (read `startAlarm`'s parameters in AvNav's `avnav_api.py`). Clear the alarm with `clearAlarm` when the route is checked.

- [ ] **Step 2: Manual test** in the same container: generate a route, activate it from the Routes page → navigation stops within 2 s and the alarm shows; rename it on the Routes page and activate → stopped; move one point in the editor and activate → stopped; press Mark as checked → activation works and the name is `<name> (asw)`.

- [ ] **Step 3: Commit** — `git commit -am "Stop unchecked asw routes from being followed"`.

### Task 6: `plugin.mjs` — button, dialog and local warning

**Files:**
- Create: `asw/plugin.mjs`, `asw/plugin.css` (button icon only), `asw/icon.svg`

**Interfaces:**
- Consumes: the HTTP API from Task 4 at `api.getBaseUrl() + "/api/<cmd>"`.
- AvNav API: `registerUserButton({name: "aswAutoRoute", ...}, "editroutepage")`, `showDialog`, `getStoreData("nav.routeHandler.editingRoute")`, `getStoreData("nav.routeHandler.activeName")`. Before writing code, open AvNav in the browser and log `getStoreData("nav.routeHandler.editingRoute")` to see the object's real shape (name and points with lat, lon, name); use what is there.

- [ ] **Step 1: Implement the dialog.** Disabled with a short reason unless status is `ready` and the route has 2 or more points. Fields: distance from shore (nm, 0–5, step 0.1), avoid canals, allow Arctic, from `options`. Text under the fields: "The route goes around land. It does not consider depth and is not for navigation. Check every point before you use it." Buttons: Make route, Cancel. After success: distance in nm, number of points, waypoints on land (if any), and "Saved as <name>. Open it on the Routes page, check it, then press Mark as checked." A second section lists `unchecked` routes, each with a Mark as checked button (confirmation inside the dialog, no browser `confirm()`). If the name exists, ask in the dialog before overwriting.

- [ ] **Step 2: Local-route warning.** Every 5 s read `activeName`; if it is in `unchecked` names, show one AvNav dialog per activation: "You are following an asw route that is not checked." (The server cannot see local routes in disconnected mode.)

- [ ] **Step 3: Manual test** in the container with the browser: the full flow from the spec's "Success" section, plus the disconnected-mode warning. Take screenshots for the README.

- [ ] **Step 4: Commit** — `git commit -am "Auto route button and dialog"`.

### Task 7: README, release workflow, first release

**Files:**
- Create: `README.md`, `.github/workflows/release.yml`, `CHANGELOG.md`

- [ ] **Step 1: README** in the owner's plain style: what it does (one paragraph), the not-for-navigation and not-weather-routing notes, requirements (AvNav 20261008+, 64-bit Linux or Pi 4/5 with 2 GB or more, about 2.1 GB free), install (upload zip on the Plugins page, press download on Wi-Fi), use (draw start and destination, Auto route, check, Mark as checked), the two known limits from the spec (about a second active before stop; local routes only warned), screenshots, link to asw.
- [ ] **Step 2: Release workflow** on tag `v*`: run `python3 tests/selfcheck.py`, run `build.sh`, attach `dist/avnav-asw-<version>.zip` to the GitHub release.
- [ ] **Step 3: Run `python3 tests/selfcheck.py` and `./build.sh` locally** → `ok`, zip present.
- [ ] **Step 4: Draft (do not post) the #435 reply** in the owner's voice: the plugin, its limits, and the proposal for an AvNav route flag "activation blocked until confirmed", with an offer to send that AvNav pull request. Save it in the plugin repo's `docs/` for the owner.
- [ ] **Step 5: Commit.** Then ask the owner to create `auto-sea-way/avnav-plugin`, push, and tag `v0.1.0` only after the owner says yes. A Pi test by the owner or a forum user comes before announcing it in #435 and the forum thread.
