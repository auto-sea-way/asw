# AvNav plugin for auto-sea-way: design

Date: 2026-10-30
Status: approved; revised 2026-10-30 (separate "Route to here" tool, warning instead of stop)

## Intent

[AvNav](https://github.com/wellenvogel/avnav) is an open source chart plotter that many DIY sailors run on a Raspberry Pi on board. In [avnav#435](https://github.com/wellenvogel/avnav/issues/435) a user asked for automatic route generation. The maintainer (wellenvogel) said he will not build it himself, but is open to contributions and pointed to Linux plugins.

This plugin lets an AvNav user put the crosshair on a destination, press one button, and get a route over water from the boat to that place that goes around land. asw runs on the Pi next to AvNav, so it works offline at sea. Before the route is saved, the plugin warns that asw does not know water depth and that the sailor must check every point.

Why now: AvNav is the smallest step to put asw into the hands of real sailors. The maintainer invited a plugin, he is also an editor of open-boat-projects.org, and we introduced asw on its forum on 2026-10-30 ([thread](https://www.segeln-forum.de/thread/99078-vorstellung-auto-sea-way-open-source-sea-routing/)).

## Goals

- A zip plugin that an AvNav user on Linux or a Raspberry Pi installs through AvNav's Plugins page.
- The plugin downloads the asw binary and the planet graph, starts `asw serve` locally and stops it with the plugin.
- A "Route to here" button on AvNav's chart page makes a route from the boat to the crosshair and saves it as a normal AvNav route after the sailor has read the warning.

## Non-goals

- Weather routing. #435 asks for routing with polars, wind, currents and waves. This plugin gives the shortest route over water that avoids land. The plugin and its README say this plainly. It can be a base for weather routing later.
- Android. AvNav runs Python plugins only on Linux, Raspberry Pi and Windows.
- 32-bit systems (armhf). asw ships arm64 and amd64 Linux binaries only. The plugin reports "32-bit system not supported" and does nothing else.
- Depth, tides, traffic separation schemes and any other chart data.
- Changes to asw itself, except the `.zst` graph download described under "Install and first start".

## Anti-goals

These would make the plugin a failure even if it works technically:

- A download of the 1.4 GB graph that starts without the sailor asking for it. Sailors at sea can have an expensive data plan.
- An asw route saved without the sailor seeing the warning that it does not know depth.
- An open port on the boat network. asw listens on `127.0.0.1` only.
- Text that makes the route look safe for navigation.
- The name of any consuming app other than AvNav and asw in the code, README or releases.

## Constraints found in AvNav

From the AvNav source (master, 2026-10-30) and its docs in `newDoc/docs/en/special/plugins.md`, `userjs.md` and `base/routes.md`:

- Minimum AvNav version: release 20261008 (the "202610xx" release in the docs, listed at wellenvogel.net/software/avnav/downloads/release/). It is the first release with zip plugins installed at runtime and `plugin.mjs`, and it includes `startAlarm` from 202607xx. Older AvNav is not supported. (Builder's call, from the release list on 2026-10-30.)

- A plugin is a directory with `plugin.json`, `plugin.mjs` (browser, all platforms), optional `plugin.css` and optional `plugin.py` (server, not Android). Zip plugins are uploaded on the Plugins page and appear as `user-<name>`.
- `plugin.py` gets an API object (`server/avnav_api.py`). The functions used here: `getDataDir`, `setStatus`, `registerRequestHandler` (HTTP calls from the browser part to `<getBaseUrl()>/api`), `registerEditableParameters` and `saveConfigValues` (settings on the plugin page), `registerRestart`, `shouldStopMainThread`, `startAlarm` and `clearAlarm` (AvNav 202607xx and newer), `log` and `error`.
- `plugin.mjs` can register a button on a page with `registerUserButton` (the chart page id is `navpage`), show a dialog with `showDialog`, and read any store key with `getStoreData`. Useful keys: `nav.routeHandler.editingRoute` (the route open in the editor), `nav.routeHandler.activeName`, `map.centerPosition`, `nav.gps.lat` and `nav.gps.lon`.
- The browser API cannot change the route in the editor and cannot start or stop navigation.
- Routes are `.gpx` files in the router's `routes` directory under the AvNav data directory. The Routes page lists that directory on every request (`AVNRouter.handleList`), so a new file written there shows up without a restart.
- There is no plugin hook to block or approve route activation. The router has `getleg` and `unsetleg` commands in `AVNRouter.handleSpecialApiRequest`, reachable through AvNav's own HTTP API. Using them from a plugin relies on an internal interface that can change between AvNav versions.

## Design decisions

### Install and first start

- Distribution: a zip `avnav-asw-<version>.zip` attached to each release of a new repo `auto-sea-way/avnav-plugin`. The zip holds only the plugin files, no binary and no graph. The zip's top folder name stays the same for every version, as AvNav requires.
- `plugin.json` sets the asw version the plugin works with. The binary and the graph come from that asw GitHub release.
- After install, the plugin page status says that the graph is missing, with its download size (about 540 MB) and a hint to use Wi-Fi. **The download starts only when the sailor presses a button.** The binary is small and is downloaded in the same step.
- The plugin downloads `asw.graph.zst` (about 540 MB) to save mobile data. The plain `asw.graph` is 1.4 GB. This needs one small asw change, decided by the owner: `asw serve --graph-url` accepts a `.zst` URL, unpacks it while downloading, checks it and only then renames it into place. The check is the zstd content checksum (the release builds the file with `zstd --check`) plus opening the result as a v5 graph, as asw-mobile's `installGraph` does. A `SHA256SUMS` lookup would come from the same server, so it adds no trust and is left out. (Builder's call, made while planning.) This change ships in an asw release before the plugin. The free-space check counts the unpacked size plus the temporary file (about 2 GB).
- Files go under AvNav's data directory (for example `<dataDir>/asw/`), so they survive plugin updates.
- The plugin checks the machine type. arm64 and x86_64 Linux are supported. Anything else shows "not supported" in the status.
- Before the download the plugin checks free disk space and refuses with a clear status if there is not enough.
- Below 2 GB of RAM the plugin only warns in the status. It does not refuse. The graph is memory-mapped, so Linux can drop its pages under memory pressure: routing gets slower, but AvNav is not pushed out of memory. The README says Pi 4 or 5 with 2 GB or more. (Builder's call.)

### Running asw

- The plugin starts `asw serve --host 127.0.0.1 --port <port> --graph <path> --api-key <random>` as a child process. The key is generated at each start and kept in memory.
- It waits for `/ready`, then sets the status to ready. It stops the child when the plugin stops (`registerRestart`) or AvNav exits, and restarts it if it dies.
- Status values the sailor sees: graph missing, downloading (with progress), starting, ready, error with a short reason.

### Making a route

Changed by the owner on 2026-10-30 after seeing a preview of the first design (a button in the route editor that rebuilt the drawn route). The owner found that UI confusing and chose a separate tool, and dropped the stop mechanism in favour of a warning before saving.

- A user button "Route to here" on AvNav's chart page (`navpage`), next to AvNav's own Goto. The start is the boat's GPS position (`nav.gps.lat`, `nav.gps.lon`), the destination is the map centre under AvNav's crosshair (`map.centerPosition`).
- Without a GPS fix, or while asw is not ready, the dialog only explains why it cannot route. (Builder's call.)
- Step 1, the dialog: distance from shore in nautical miles (asw `shore_buffer`, 0 to 5, default 0), avoid canals (`canals=false`), allow Arctic (`arctic=true`), remembered from last time, and a "Make route" button.
- The browser part sends start, destination and options to the Python part, which calls asw `/route` once. The two ends keep the names "Start" and "Destination"; the points in between are named "asw 1", "asw 2", and so on.
- Step 2, the same dialog shows the result: the distance in nautical miles, whether the start or the destination is on land (asw `land_legs`), the warning "This route only goes around land. It does not know water depth, rocks or traffic rules. Check every point on the chart before you use it.", a name field filled with "asw <yyyy-mm-dd hhmm>" (builder's call; no ":" because AvNav refuses it in route names), and the buttons "Save route" and "Cancel".
- Nothing is saved before "Save route". The route is saved as a normal AvNav route file `<name>.gpx`; if that name exists, the dialog asks before overwriting. After saving, the sailor opens, edits and starts it like any other AvNav route.

### What was removed

The first design kept a list of unchecked routes, stopped them if started and raised an alarm until the sailor pressed "Mark as checked". The owner removed this on 2026-10-30: the warning before saving is the only guard. Once saved, AvNav follows the route like any other, including output to an autopilot. The README states this.

## Testing

- One small self-check in the plugin repo, runnable with plain Python: turning an asw response into named route points, and the on-land check.
- Manual test: run AvNav from source on Linux or in a container, install the zip, download the graph, make routes from a set boat position to the crosshair in the Marmaris area and the Aegean, save one, open it on the Routes page and start it.
- A real Raspberry Pi has not been tested. The owner or a forum user needs to test a Pi 4 or 5 before the plugin is announced as working on a Pi.

## Success

- A sailor on a Pi 4 or 5 installs the zip, presses download at the marina, puts the crosshair on the destination, presses Route to here, reads the warning, saves the route, checks it on the chart and follows it.
- wellenvogel adds the plugin to AvNav's plugin list (`newDoc/docs/*/special/plugin-list.md`).
- We post it in the open-boat-projects.org forum thread and reply in #435.

## Left to the builder

- Plugin internals: file layout, how the browser part and Python part talk (beyond `registerRequestHandler`), how progress is shown, and the port choice.
- GPX details of the saved route, matching what AvNav itself writes.
- The repo's README, license (MIT / Apache 2.0 like asw) and release workflow.
