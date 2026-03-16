# API Key Authentication Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add static API key authentication to asw-serve so `/route` and `/info` require a valid `X-Api-Key` header, while `/health` and `/ready` remain public.

**Architecture:** Axum middleware layer applied to a nested router containing only the protected endpoints. The API key is passed via CLI arg (`--api-key`) with `env = "ASW_API_KEY"` fallback. Constant-time comparison via the `subtle` crate prevents timing attacks.

**Tech Stack:** Rust, axum 0.8 (middleware), subtle (constant-time eq), clap (CLI arg), tower (test utilities)

**Spec:** `docs/superpowers/specs/2026-03-16-api-key-auth-design.md`

---

## Chunk 1: Core Implementation

### Task 1: Add `subtle` dependency to asw-serve

**Files:**
- Modify: `crates/asw-serve/Cargo.toml`

- [ ] **Step 1: Add subtle and tower to Cargo.toml**

Add `subtle` as a regular dependency and `tower` as a dev-dependency (for `ServiceExt::oneshot` in tests):

```toml
subtle = "2"

[dev-dependencies]
tower = { version = "0.5", features = ["util"] }
tokio = { version = "1", features = ["macros", "rt"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p asw-serve`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add crates/asw-serve/Cargo.toml Cargo.lock
git commit -m "chore: add subtle and tower dev-dep to asw-serve"
```

---

### Task 2: Add `api_key` field to `ServerState`

**Files:**
- Modify: `crates/asw-serve/src/state.rs:6-17`

- [ ] **Step 1: Write failing test**

Add to bottom of `crates/asw-serve/src/state.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_state_stores_api_key() {
        let state = ServerState::new("test.graph".into(), "test-key-1234".into());
        assert_eq!(state.api_key, "test-key-1234");
        assert_eq!(state.graph_path, "test.graph");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p asw-serve --lib state::tests::server_state_stores_api_key`
Expected: FAIL — `ServerState::new` takes 1 argument, not 2

- [ ] **Step 3: Update ServerState**

In `crates/asw-serve/src/state.rs`, add `api_key` field to `ServerState` and update `new()`:

```rust
pub struct ServerState {
    pub inner: tokio::sync::RwLock<Option<AppState>>,
    pub graph_path: String,
    pub api_key: String,
}

impl ServerState {
    pub fn new(graph_path: String, api_key: String) -> Self {
        Self {
            inner: tokio::sync::RwLock::new(None),
            graph_path,
            api_key,
        }
    }
    // ... set_ready unchanged
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p asw-serve --lib state::tests::server_state_stores_api_key`
Expected: PASS

> **Note:** After this task, `asw-cli` will not compile because `ServerState::new()` now takes 2 args. This is fixed in Task 4. Until then, only run tests scoped to `asw-serve` (`cargo test -p asw-serve`).

- [ ] **Step 5: Commit**

```bash
git add crates/asw-serve/src/state.rs
git commit -m "feat: add api_key field to ServerState"
```

---

### Task 3: Write the auth middleware and split the router

**Files:**
- Modify: `crates/asw-serve/src/api.rs:1-152`

- [ ] **Step 1: Write failing tests for auth middleware**

Add these tests to the existing `#[cfg(test)] mod tests` block in `crates/asw-serve/src/api.rs`. These test the full router (middleware + handlers) using `tower::ServiceExt::oneshot`:

```rust
use axum::body::Body;
use axum::http::{Request, StatusCode as HyperStatus};
use tower::ServiceExt; // for oneshot

fn test_state() -> Arc<ServerState> {
    Arc::new(ServerState::new(
        "test.graph".into(),
        "secret-key-1234567890".into(),
    ))
}

#[tokio::test]
async fn health_no_auth_required() {
    let app = create_router(test_state());
    let req = Request::get("/health").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), HyperStatus::OK);
}

#[tokio::test]
async fn ready_no_auth_required() {
    let app = create_router(test_state());
    let req = Request::get("/ready").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // 503 because graph not loaded, but NOT 401
    assert_eq!(resp.status(), HyperStatus::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn route_returns_401_without_key() {
    let app = create_router(test_state());
    let req = Request::get("/route?from=36.85,28.27&to=36.90,28.30")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
}

#[tokio::test]
async fn info_returns_401_without_key() {
    let app = create_router(test_state());
    let req = Request::get("/info").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
}

#[tokio::test]
async fn route_returns_401_with_wrong_key() {
    let app = create_router(test_state());
    let req = Request::get("/route?from=36.85,28.27&to=36.90,28.30")
        .header("X-Api-Key", "wrong-key")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json, serde_json::json!({"error": "unauthorized"}));
}

#[tokio::test]
async fn info_returns_401_with_wrong_key() {
    let app = create_router(test_state());
    let req = Request::get("/info")
        .header("X-Api-Key", "wrong-key")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
}

#[tokio::test]
async fn route_passes_with_correct_key() {
    let app = create_router(test_state());
    let req = Request::get("/route?from=36.85,28.27&to=36.90,28.30")
        .header("X-Api-Key", "secret-key-1234567890")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // 503 because graph not loaded, but NOT 401 — auth passed
    assert_eq!(resp.status(), HyperStatus::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn info_passes_with_correct_key() {
    let app = create_router(test_state());
    let req = Request::get("/info")
        .header("X-Api-Key", "secret-key-1234567890")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // 503 because graph not loaded, but NOT 401
    assert_eq!(resp.status(), HyperStatus::SERVICE_UNAVAILABLE);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p asw-serve --lib api::tests`
Expected: compilation errors — `ServerState::new` signature changed, no middleware yet

- [ ] **Step 3: Replace imports and write middleware function**

Replace the entire import block at the top of `crates/asw-serve/src/api.rs` (lines 1-11) with:

```rust
use asw_core::routing::compute_route;
use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::state::ServerState;
```

Note: `axum::extract::Request` is a type alias for `http::Request<Body>` in axum 0.8 — the same type used in tests via `Request::get(...)`. No separate `axum::http::Request` import needed.

Add the middleware function before `create_router`:

```rust
async fn api_key_middleware(
    State(state): State<Arc<ServerState>>,
    req: Request,
    next: Next,
) -> Result<axum::response::Response, (StatusCode, Json<ErrorResponse>)> {
    let provided = req
        .headers()
        .get("X-Api-Key")
        .and_then(|v| v.to_str().ok());

    match provided {
        Some(key) if key.as_bytes().ct_eq(state.api_key.as_bytes()).into() => {
            Ok(next.run(req).await)
        }
        _ => {
            tracing::warn!("Rejected request: invalid or missing API key");
            Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "unauthorized".into(),
                }),
            ))
        }
    }
}
```

- [ ] **Step 4: Split the router into public and protected groups**

Replace `create_router` in `crates/asw-serve/src/api.rs`:

```rust
pub fn create_router(state: Arc<ServerState>) -> Router {
    let protected = Router::new()
        .route("/route", get(route_handler))
        .route("/info", get(info_handler))
        .layer(middleware::from_fn_with_state(state.clone(), api_key_middleware));

    Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .merge(protected)
        .with_state(state)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p asw-serve --lib api::tests`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/asw-serve/src/api.rs
git commit -m "feat: add API key auth middleware with public/protected route split"
```

---

## Chunk 2: CLI Integration and Final Verification

### Task 4: Update CLI to pass API key to ServerState

**Files:**
- Modify: `crates/asw-cli/src/main.rs:39-55` (Serve struct)
- Modify: `crates/asw-cli/src/main.rs:232-247` (Serve handler)

- [ ] **Step 1: Add `api_key` field to Serve command**

In `crates/asw-cli/src/main.rs`, add to the `Serve` variant inside `Commands`:

```rust
/// API key for authenticating requests (required)
#[arg(long, env = "ASW_API_KEY")]
api_key: String,
```

- [ ] **Step 2: Add startup validation and pass key to ServerState**

In the `Commands::Serve` match arm, destructure the new field:

```rust
Commands::Serve {
    graph,
    host,
    port,
    graph_url,
    api_key,
} => {
```

Add validation before the `listen` binding:

```rust
let api_key = api_key.trim().to_string();
if api_key.is_empty() {
    anyhow::bail!("ASW_API_KEY must not be empty or whitespace-only");
}
```

Update the `ServerState::new` call:

```rust
let state = std::sync::Arc::new(asw_serve::state::ServerState::new(graph_path, api_key));
```

- [ ] **Step 3: Verify full workspace compiles**

Run: `cargo build -p asw-cli`
Expected: compiles with no errors

- [ ] **Step 4: Verify --help shows the new flag**

Run: `cargo run -p asw-cli -- serve --help`
Expected: output includes `--api-key <API_KEY>` with env `ASW_API_KEY`

- [ ] **Step 5: Verify server refuses to start without key**

Run: `cargo run -p asw-cli -- serve 2>&1 || true`
Expected: error message about missing `--api-key` or `ASW_API_KEY`

- [ ] **Step 6: Commit**

```bash
git add crates/asw-cli/src/main.rs
git commit -m "feat: add --api-key CLI arg with ASW_API_KEY env fallback"
```

---

### Task 5: Run full test suite

- [ ] **Step 1: Run all tests**

Run: `cargo test --workspace`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Final commit if any fixups needed**

Only if clippy/test required changes.
