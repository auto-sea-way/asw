# Sea Route Demo

Throwaway example app: a fullscreen Leaflet map (CARTO Voyager + OpenSeaMap
seamarks) with one tool — click to lay down a multi-point sea route, computed
segment-by-segment via the real `asw-serve` `/route` API, with live
nautical-mile distance labels.

Not integrated into the main build, README, or CI — self-contained here.

## Run it

1. From the repo root, make sure `export/planet.graph` exists and `.env`
   has `ASW_API_KEY` set (both already required by the rest of this repo).
2. Start the backend:
   ```bash
   cd examples/map-routing
   docker compose up -d --build
   ```
   Wait for it to become ready (planet graph load takes ~10-30s):
   ```bash
   until curl -s -o /dev/null -w "%{http_code}" http://localhost:3000/ready | grep -q 200; do sleep 2; done
   ```
3. Open `index.html` directly in a browser (double-click it, or
   `open index.html` on macOS) — no local web server needed.
4. Click "Route Tool: On", then click the map to lay down waypoints.
   Drag a point to move it, click a point to delete it, Ctrl+Z to undo,
   "Clear" to reset.

`index.html` hardcodes `API_BASE = 'http://localhost:3000'` and
`API_KEY = 'test-api-key'` — if your `.env`'s `ASW_API_KEY` differs, edit
`API_KEY` in `index.html` to match, or set `ASW_API_KEY=test-api-key` in
your `.env` for this demo.

## Stop it

```bash
docker compose down
```
