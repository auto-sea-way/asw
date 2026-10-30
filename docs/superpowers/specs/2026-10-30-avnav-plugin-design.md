# AvNav plugin for auto-sea-way: design

Date: 2026-10-30
Status: builder check done, waiting for owner approval

## Intent

[AvNav](https://github.com/wellenvogel/avnav) is an open source chart plotter that many DIY sailors run on a Raspberry Pi on board. In [avnav#435](https://github.com/wellenvogel/avnav/issues/435) a user asked for automatic route generation. The maintainer (wellenvogel) said he will not build it himself, but is open to contributions and pointed to Linux plugins.

This plugin lets an AvNav user draw a rough route (at minimum a start and a destination), press one button, and get a route over water that goes around land. asw runs on the Pi next to AvNav, so it works offline at sea. The route is saved as a normal AvNav route that the sailor can edit, but the plugin keeps the sailor from following it until they have checked it, because asw does not know water depth.

Why now: AvNav is the smallest step to put asw into the hands of real sailors. The maintainer invited a plugin, he is also an editor of open-boat-projects.org, and we introduced asw on its forum on 2026-10-30 ([thread](https://www.segeln-forum.de/thread/99078-vorstellung-auto-sea-way-open-source-sea-routing/)).

## Goals

- A zip plugin that an AvNav user on Linux or a Raspberry Pi installs through AvNav's Plugins page.
- The plugin downloads the asw binary and the planet graph, starts `asw serve` locally and stops it with the plugin.
- A button in AvNav's route editor turns the route being edited into an asw route and saves it as a new route.
- A generated route cannot be followed until the sailor marks it as checked.

## Non-goals

- Weather routing. #435 asks for routing with polars, wind, currents and waves. This plugin gives the shortest route over water that avoids land. The plugin and its README say this plainly. It can be a base for weather routing later.
- Android. AvNav runs Python plugins only on Linux, Raspberry Pi and Windows.
- 32-bit systems (armhf). asw ships arm64 and amd64 Linux binaries only. The plugin reports "32-bit system not supported" and does nothing else.
- Depth, tides, traffic separation schemes and any other chart data.
- Changes to asw itself, except the `.zst` graph download described under "Install and first start".

## Anti-goals

These would make the plugin a failure even if it works technically:

- A download of the 1.4 GB graph that starts without the sailor asking for it. Sailors at sea can have an expensive data plan.
- An asw route that an autopilot follows without the sailor having looked at it.
- An open port on the boat network. asw listens on `127.0.0.1` only.
- Text that makes the route look safe for navigation.
- The name of any consuming app other than AvNav and asw in the code, README or releases.

## Constraints found in AvNav

From the AvNav source (master, 2026-10-30) and its docs in `newDoc/docs/en/special/plugins.md`, `userjs.md` and `base/routes.md`:

- Minimum AvNav version: release 20261008 (the "202610xx" release in the docs, listed at wellenvogel.net/software/avnav/downloads/release/). It is the first release with zip plugins installed at runtime and `plugin.mjs`, and it includes `startAlarm` from 202607xx. Older AvNav is not supported. (Builder's call, from the release list on 2026-10-30.)

- A plugin is a directory with `plugin.json`, `plugin.mjs` (browser, all platforms), optional `plugin.css` and optional `plugin.py` (server, not Android). Zip plugins are uploaded on the Plugins page and appear as `user-<name>`.
- `plugin.py` gets an API object (`server/avnav_api.py`). The functions used here: `getDataDir`, `setStatus`, `registerRequestHandler` (HTTP calls from the browser part to `<getBaseUrl()>/api`), `registerEditableParameters` and `saveConfigValues` (settings on the plugin page), `registerRestart`, `shouldStopMainThread`, `startAlarm` and `clearAlarm` (AvNav 202607xx and newer), `log` and `error`.
- `plugin.mjs` can register a button on a page with `registerUserButton` (the route editor page id is `editroutepage`), show a dialog with `showDialog`, and read any store key with `getStoreData`. Useful keys: `nav.routeHandler.editingRoute` (the route open in the editor), `nav.routeHandler.activeName`, `map.centerPosition`, `nav.gps.lat` and `nav.gps.lon`.
- The browser API cannot change the route in the editor and cannot start or stop navigation.
- Routes are `.gpx` files in the router's `routes` directory under the AvNav data directory. The Routes page lists that directory on every request (`AVNRouter.handleList`), so a new file written there shows up without a restart.
- There is no plugin hook to block or approve route activation. The router has `getleg` and `unsetleg` commands in `AVNRouter.handleSpecialApiRequest`, reachable through AvNav's own HTTP API. Using them from a plugin relies on an internal interface that can change between AvNav versions.

## Design decisions

### Install and first start

- Distribution: a zip `avnav-asw-<version>.zip` attached to each release of a new repo `auto-sea-way/avnav-plugin`. The zip holds only the plugin files, no binary and no graph. The zip's top folder name stays the same for every version, as AvNav requires.
- `plugin.json` sets the asw version the plugin works with. The binary and the graph come from that asw GitHub release.
- After install, the plugin page status says that the graph is missing, with its download size (about 540 MB) and a hint to use Wi-Fi. **The download starts only when the sailor presses a button.** The binary is small and is downloaded in the same step.
- The plugin downloads `asw.graph.zst` (about 540 MB), not the plain `asw.graph` (1.4 GB), to save mobile data. This needs one small asw change, decided by the owner: `asw serve --graph-url` accepts a `.zst` URL, unpacks it while downloading, checks it and only then renames it into place. The check is the zstd content checksum (the release builds the file with `zstd --check`) plus opening the result as a v5 graph, as asw-mobile's `installGraph` does. A `SHA256SUMS` lookup would come from the same server, so it adds no trust and is left out. (Builder's call, made while planning.) This change ships in an asw release before the plugin. The free-space check counts the unpacked size plus the temporary file (about 2 GB).
- Files go under AvNav's data directory (for example `<dataDir>/asw/`), so they survive plugin updates.
- The plugin checks the machine type. arm64 and x86_64 Linux are supported. Anything else shows "not supported" in the status.
- Before the download the plugin checks free disk space and refuses with a clear status if there is not enough.
- Below 2 GB of RAM the plugin only warns in the status. It does not refuse. The graph is memory-mapped, so Linux can drop its pages under memory pressure: routing gets slower, but AvNav is not pushed out of memory. The README says Pi 4 or 5 with 2 GB or more. (Builder's call.)

### Running asw

- The plugin starts `asw serve --host 127.0.0.1 --port <port> --graph <path> --api-key <random>` as a child process. The key is generated at each start and kept in memory.
- It waits for `/ready`, then sets the status to ready. It stops the child when the plugin stops (`registerRestart`) or AvNav exits, and restarts it if it dies.
- Status values the sailor sees: graph missing, downloading (with progress), starting, ready, error with a short reason.

### Making a route

- A user button on the route editor page, "Auto route", opens a dialog. It needs a route open in the editor with at least two points.
- Dialog options: distance from shore in nautical miles (asw `shore_buffer`, 0 to 5, default 0), avoid canals (`canals=false`), allow Arctic (`arctic=true`). The plugin remembers the last values.
- The browser part sends the waypoints and options to the Python part. Python calls asw `/route` for each leg between consecutive waypoints and joins the results into one route.
- Every waypoint of the sailor's route is kept, in order and with its name. Points added by asw are named "asw 1", "asw 2" and so on. Shared points between legs are not duplicated.
- Each asw response has `distance_nm` and `land_legs`. The dialog shows the total distance in nautical miles. If any leg has land legs (for example a waypoint on a quay), the route still contains them, and the dialog lists which waypoints are on land.
- The result is saved as a new route file named `<original name> (asw, unchecked)`. The original route is never changed. If that name exists, it is overwritten after the dialog asks.
- The dialog states that the route does not consider depth and is not for navigation.

### Checking before use

Decided by the owner: the sailor must not be able to follow a generated route before checking it, and must be able to move its points in the route editor. AvNav has no hook for this, so there are two steps:

1. **Now: watch and stop.** The plugin keeps its own list of the routes it generated and has not seen checked, in its data directory, matched by the route's points. The name `(asw, unchecked)` is only a hint for the sailor, so renaming the route on AvNav's Routes page does not unlock it. The Python part watches the active route (through `getleg` or the router's state). If the active route matches an unchecked entry, it stops navigation right away (`unsetleg`) and raises an AvNav alarm saying that the route must be checked first. The sailor marks a route as checked with a "Mark as checked" action in the plugin dialog. This removes it from the list and removes `, unchecked` from the file name. If the sailor edits an unchecked route, its points change, so the plugin must still recognise it: the builder decides how (for example, by the file name together with the points, or by keeping the original file name). Decided by the owner.
   - Known limit: the route is active for about a second before it is stopped, so a few NMEA sentences may reach an autopilot. The README states this.
   - Known limit: in AvNav's disconnected mode, a local copy of a route can be activated without the server knowing, so the plugin cannot stop it. AvNav sends no RMB or APB for local routes, so an autopilot gets nothing. The browser part watches `nav.routeHandler.activeName` and the active route's points, and shows a warning when an unchecked asw route is followed locally. Decided by the owner.
2. **Later: a hook in AvNav.** Propose in #435 a route property that AvNav respects, such as "activation blocked until confirmed", and offer to send the AvNav pull request. When AvNav has it, the plugin uses it and drops the watcher.

## Testing

- One small self-check in the plugin repo, runnable with plain Python: joining leg responses, naming the added points, skipping duplicate shared points, and the unchecked and checked naming rule.
- Manual test: run AvNav from source on Linux or in a container, install the zip, download the graph, make routes in the Marmaris area and the Aegean, edit one, try to activate it unchecked, then mark it checked and activate it.
- A real Raspberry Pi has not been tested. The owner or a forum user needs to test a Pi 4 or 5 before the plugin is announced as working on a Pi.

## Success

- A sailor on a Pi 4 or 5 installs the zip, presses download at the marina, draws start and destination, presses Auto route, edits the result, marks it checked and follows it.
- wellenvogel adds the plugin to AvNav's plugin list (`newDoc/docs/*/special/plugin-list.md`).
- We post it in the open-boat-projects.org forum thread and reply in #435.

## Left to the builder

- Plugin internals: file layout, how the browser part and Python part talk (beyond `registerRequestHandler`), how progress is shown, and the port choice.
- How the watcher reads and stops the active route (the HTTP API or the router object in the same process), as long as the risk is documented.
- GPX details of the saved route, matching what AvNav itself writes.
- The repo's README, license (MIT / Apache 2.0 like asw) and release workflow.
