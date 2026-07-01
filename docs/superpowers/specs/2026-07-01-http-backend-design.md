# HTTP Backend + Backend Trait Lifecycle Design

**Date:** 2026-07-01
**Scope:** Generalize `Backend` trait to cover lifecycle (startup/teardown), device
discovery/validation, and event sink; add an `http` backend that serves the last
10 dispatched events as plain text over HTTP.
**Status:** Design approved, ready for implementation

---

## Overview

Today `Backend` has a single method, `send_event`. This spec extends it to a full
lifecycle trait, required (no default methods) for every implementor, and adds a
second backend — `http` — alongside the existing `stdout` backend. The `http`
backend runs a small axum server exposing `GET /`, which returns the last 10
events it received as plain text, one per line.

---

## `Backend` Trait

```rust
pub enum DeviceList {
    Anything,
    List(Vec<String>),
}

pub trait Backend: Send + Sync {
    fn startup(&self) -> anyhow::Result<()>;
    fn teardown(&self) -> anyhow::Result<()>;

    /// On failure, the returned error is printed to the console (stderr) by
    /// the caller (see main.rs's device validation pass) — implementors just
    /// return a descriptive `Err`, they don't print anything themselves.
    fn validate_device(&self, device_id: &str) -> anyhow::Result<()>;

    fn list_devices(&self) -> anyhow::Result<DeviceList>;
    fn send_event(&self, device_id: String, event: &Event);
}
```

No default method bodies — every backend implements all five explicitly.

`DeviceList::Anything` signals a backend accepts any device ID (no fixed set to
enumerate); `DeviceList::List(ids)` gives the concrete set of valid IDs.

### `StdoutBackend`

- `startup` → `Ok(())`, no-op.
- `teardown` → `Ok(())`, no-op.
- `list_devices` → `Ok(DeviceList::Anything)`.
- `validate_device` → delegates to `list_devices()`: `Anything` always passes.
- `send_event` → unchanged (`println!("{device_id} {event:?}")`).

### `HttpBackend`

Fields:
- `events: Arc<Mutex<VecDeque<String>>>` — last 10 formatted event lines, oldest
  at the front; push new lines at the back, pop the front once len > 10.
- `bind: String` — address to listen on.
- `shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>` — set by
  `startup`, consumed by `teardown`.

Behavior:
- `startup()`: builds an axum router (`GET /` reads the buffer, joins lines with
  `\n`, returns as `text/plain`), `tokio::spawn`s the server with
  `axum::serve(...).with_graceful_shutdown(...)` wired to a fresh oneshot
  channel; stores the sender in `shutdown_tx`.
- `teardown()`: takes the sender out of `shutdown_tx` and fires it (ignores the
  result if already taken/dropped). Fire-and-forget — does not await the
  server task finishing, since the trait method stays sync.
- `list_devices()` → `Ok(DeviceList::List(vec!["0".to_string()]))` — the backend
  represents one conceptual sink device.
- `validate_device(id)` → delegates to `list_devices()`: `List(ids)` → bail
  unless `ids.contains(&id.to_string())`.
- `send_event(device_id, event)`: lock the buffer, push
  `format!("{device_id} {event:?}")`, truncate the front until len ≤ 10.

---

## Config

Add an optional `http` section to `Config`:

```yaml
http:
  bind: "127.0.0.1:8080"   # optional, default shown
```

```rust
#[derive(Debug, Deserialize, Default)]
pub struct HttpConfig {
    #[serde(default = "default_http_bind")]
    pub bind: String,
}

fn default_http_bind() -> String {
    "127.0.0.1:8080".to_string()
}
```

`Config` gains `pub http: Option<HttpConfig>`.

`backend::create` signature changes from `create(name: &str)` to
`create(name: &str, config: &Config) -> anyhow::Result<Box<dyn Backend>>` so the
`http` backend can read `config.http.clone().unwrap_or_default()` for its bind
address. `stdout`'s construction ignores `config`.

`backend::is_known(name: &str) -> bool` is unchanged in shape, extended to also
match `"http"`.

---

## Validation Flow

`Config::validate()` keeps using the free-standing `backend::is_known()` check
as it does today (backends aren't instantiated at that point, so trait-level
`validate_device` can't run there).

A **new second validation pass** runs in `main.rs`, immediately after the
`backends` map is built (before subscribing to MQTT topics): for every rule's
device address, look up the backend by name and call
`backends[backend_name].validate_device(device_id)`. On `Err`, `main.rs` prints
the error to stderr (e.g. `eprintln!("{e}")`) and then returns/bails, stopping
startup. This catches invalid IDs (e.g. `http/1`, since only `http/0` is valid)
before the bridge starts consuming events.

---

## Startup / Shutdown Wiring in `main.rs`

- Right after each backend is inserted into the `backends` map (during the loop
  that currently just calls `backend::create`), call `.startup()?` on it.
- Wrap the existing MQTT poll loop in `tokio::select!` against
  `tokio::signal::ctrl_c()`:
  - the loop branch runs as it does today;
  - the ctrl-c branch calls `teardown()` on every backend in the map (logging
    but not propagating individual errors) and then returns, ending the
    program.

---

## Data Flow (new backend)

```
Rule matches → Backend::send_event(device_id, event)
  → HttpBackend locks buffer, pushes "{device_id} {event:?}", truncates to 10

GET / (any time)
  → HttpBackend's axum handler locks buffer, joins lines with "\n", returns text/plain
```

---

## Dependencies

- `axum` — new dependency, HTTP server.
- No new tokio features needed; `full` (already enabled) includes `signal` and
  `sync`.

---

## Testing

- `HttpBackend::validate_device`: accepts `"0"`, rejects `"1"` / arbitrary
  strings.
- `HttpBackend::list_devices`: returns `List(["0"])`.
- Buffer truncation: push 11 events, assert len stays 10 and the oldest was
  dropped (first remaining line is the 2nd pushed, not the 1st).
- Route handler: construct `HttpBackend`, push a couple of events directly via
  `send_event`, call the `GET /` handler (via axum's test utilities or a direct
  function call bypassing the network), assert body text matches expected
  joined lines.
- `backend::tests`: extend factory tests — `is_known("http")` true,
  `create("http", &config)` succeeds, unknown backend names still error.
- `StdoutBackend`: `list_devices` returns `Anything`, `validate_device` always
  `Ok(())`, `startup`/`teardown` return `Ok(())`.
- `Config` parsing: YAML with and without an `http:` section both deserialize,
  missing section leaves `http: None`, present section without `bind` defaults
  to `127.0.0.1:8080`.

---

## Non-Goals

- Configurable buffer size (fixed at 10 per the request).
- Any route besides `GET /` (no health check, no JSON, no per-device
  filtering).
- Graceful draining of in-flight HTTP requests on shutdown beyond axum's
  built-in graceful shutdown behavior.
- Authentication/TLS on the HTTP server.
- Persisting the event buffer across restarts.

---

## Success Criteria

1. ✓ `Backend` trait has `startup`, `teardown`, `validate_device`,
   `list_devices`, `send_event`, all required (no default bodies).
2. ✓ `StdoutBackend` implements all five explicitly; existing behavior for
   `send_event` unchanged.
3. ✓ New `http` backend: `GET /` on configured bind address returns the last 10
   events as plain text, one per line.
4. ✓ `http/0` is a valid device address; any other `http/<id>` fails the
   instance-level validation pass in `main.rs` with a clear error before the
   bridge starts.
5. ✓ Ctrl-C triggers `teardown()` on all backends before exit.
6. ✓ All existing tests pass; new unit tests cover buffer truncation, device
   validation/listing, and the HTTP route's output format.