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

- **Environment variable:** `ASW_API_KEY`
- Picked up from `.env` via dotenvy (consistent with `HETZNER_TOKEN`)
- If `ASW_API_KEY` is not set at startup, the server exits with an error — no accidental open mode

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

The middleware function:
1. Extracts `X-Api-Key` header from request
2. Compares against stored key (constant-time)
3. Returns 401 JSON response on mismatch
4. Passes request through on match

### State Changes

- `ServerState` gains an `api_key: String` field, set at construction from env var
- CLI `serve` command reads `ASW_API_KEY` and passes it to `ServerState::new()`

### Dependencies

- `subtle` crate for constant-time comparison (or manual constant-time eq)

## Out of Scope

- Key rotation, expiry, or multiple keys
- Rate limiting
- CORS
- OAuth/JWT
