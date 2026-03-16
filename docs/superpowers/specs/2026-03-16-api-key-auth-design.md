# API Key Authentication for asw-serve

## Problem

The asw-serve HTTP server exposes all endpoints without authentication. When deployed standalone on the internet, anyone who discovers the port can use the routing API.

## Design

### Mechanism

Static API key authentication via HTTP header. Requests to protected endpoints must include:

```
X-Api-Key: <key>
```

The server validates the header value against a stored key using constant-time comparison (`subtle` crate or equivalent) to prevent timing side-channel attacks.

### Configuration

- **CLI argument:** `--api-key` on the `Serve` clap subcommand, with `env = "ASW_API_KEY"` fallback (same pattern as `--graph`, `--host`, `--port`)
- Picked up from `.env` via dotenvy (consistent with `HETZNER_TOKEN`)
- Clap's required-field validation handles the "refuse to start" behavior — no custom validation needed
- Startup validation: reject empty or whitespace-only keys with a clear error message

### Protected vs Public Endpoints

| Endpoint | Auth Required |
|----------|--------------|
| `GET /route` | Yes |
| `GET /info` | Yes |
| `GET /health` | No |
| `GET /ready` | No |

Health and readiness probes remain public for container orchestrators.

### Error Responses

- **401 Unauthorized** — missing or invalid `X-Api-Key` header
- Response body: `{"error": "unauthorized"}` (JSON, consistent with existing error responses)

### Implementation Approach

Use axum's middleware layering to split the router:

```
Router::new()
    .route("/health", get(health_handler))
    .route("/ready", get(ready_handler))
    .merge(
        Router::new()
            .route("/route", get(route_handler))
            .route("/info", get(info_handler))
            .layer(middleware::from_fn_with_state(state, api_key_middleware))
    )
    .with_state(state)
```

The middleware function (lives in `api.rs` alongside router construction):
1. Extracts `X-Api-Key` header from request
2. Compares value (case-sensitive, as bytes) against stored key using constant-time eq
3. Returns `(StatusCode::UNAUTHORIZED, Json(ErrorResponse))` on mismatch — reuses existing `ErrorResponse` struct, axum sets `Content-Type: application/json` automatically
4. Logs auth failures at `warn` level via `tracing` (without leaking the submitted key value)
5. Passes request through on match

### State Changes

- `ServerState` gains an `api_key: String` field
- `ServerState::new()` signature changes to `new(graph_path: String, api_key: String)`
- CLI `Serve` struct gains `#[arg(long, env = "ASW_API_KEY")] api_key: String` field
- Call site in `main.rs` updated to pass `api_key` to `ServerState::new()`

### Dependencies

- `subtle` crate for constant-time comparison (or manual constant-time eq)

## Testing

- **Unit: 401 on missing header** — send request without `X-Api-Key`, expect 401 + JSON error body
- **Unit: 401 on wrong key** — send request with incorrect key, expect 401
- **Unit: 200 on correct key** — send request with valid key, expect pass-through
- **Integration: public endpoints unaffected** — `/health` and `/ready` return 200 without any auth header
- **Integration: protected endpoints gated** — `/route` and `/info` return 401 without auth

Tests use `axum::test` (tower::ServiceExt `oneshot`) to test the router directly without spinning up a TCP server.

## Out of Scope

- Key rotation, expiry, or multiple keys
- Rate limiting
- CORS
- OAuth/JWT
