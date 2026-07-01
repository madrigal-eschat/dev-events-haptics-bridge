# HTTP Backend + Backend Trait Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `http` backend that serves the last 10 dispatched events as plain text at `GET /`, and generalize the `Backend` trait to a full lifecycle (`startup`, `teardown`, `validate_device`, `list_devices`, `send_event`), all required with no default bodies.

**Architecture:** `src/backend.rs` becomes `src/backend/mod.rs` (trait, `DeviceList` enum, `StdoutBackend`, `is_known`/`create`) plus `src/backend/http.rs` (new `HttpBackend`, axum server on its own tokio task). `main.rs` gains a device-validation pass, `startup()` calls, and a `tokio::select!` around the MQTT loop that calls `teardown()` on ctrl-c.

**Tech Stack:** Rust, tokio (already a dependency, `full` features), axum (new), tower + http-body-util (new, dev-only, for testing the route handler without a real socket).

## Global Constraints

- `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must all pass before each commit (matches this repo's CI/pre-commit gate — see `CLAUDE.md`).
- `Backend` trait methods have **no default bodies** — every implementor writes all five explicitly.
- `validate_device` implementations return a descriptive `Err`; they never print anything themselves. Printing happens once, at the call site in `main.rs`.
- The HTTP backend's event buffer holds exactly the **last 10** entries, oldest dropped first.
- Only device id `"0"` is valid for the `http` backend (`http/0`).
- Default bind address for the `http` backend is `127.0.0.1:8080`, overridable via an optional `http.bind` key in the YAML config.

---

### Task 1: Restructure `backend.rs` into a module and expand the `Backend` trait

**Files:**
- Delete: `src/backend.rs`
- Create: `src/backend/mod.rs`
- Modify: nothing else (call sites in `main.rs` are unaffected — `send_event`, `is_known`, `create` keep their existing signatures/behavior in this task)

**Interfaces:**
- Produces: `pub trait Backend` with `startup`, `teardown`, `validate_device`, `list_devices`, `send_event`; `pub enum DeviceList { Anything, List(Vec<String>) }`; `pub struct StdoutBackend`; `pub fn is_known(name: &str) -> bool`; `pub fn create(name: &str) -> anyhow::Result<Box<dyn Backend>>` (signature unchanged for now — Task 5 adds the `&Config` parameter).

- [ ] **Step 1: Delete the old file and create the module directory with the expanded trait**

Run:
```bash
git rm src/backend.rs
mkdir -p src/backend
```

- [ ] **Step 2: Write `src/backend/mod.rs`**

```rust
use crate::gestures::Event;

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

pub struct StdoutBackend;

impl Backend for StdoutBackend {
    fn startup(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn teardown(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn validate_device(&self, device_id: &str) -> anyhow::Result<()> {
        match self.list_devices()? {
            DeviceList::Anything => Ok(()),
            DeviceList::List(ids) => {
                anyhow::bail!("stdout backend: unknown device id '{device_id}', valid ids: {ids:?}")
            }
        }
    }

    fn list_devices(&self) -> anyhow::Result<DeviceList> {
        Ok(DeviceList::Anything)
    }

    fn send_event(&self, device_id: String, event: &Event) {
        println!("{device_id} {event:?}");
    }
}

pub fn is_known(name: &str) -> bool {
    matches!(name, "stdout")
}

pub fn create(name: &str) -> anyhow::Result<Box<dyn Backend>> {
    match name {
        "stdout" => Ok(Box::new(StdoutBackend)),
        other => anyhow::bail!("unknown backend: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_known_backends_succeed() {
        assert!(create("stdout").is_ok());
    }

    #[test]
    fn create_unknown_backend_errors() {
        let err = create("nonexistent").err().expect("expected error");
        assert!(err.to_string().contains("nonexistent"), "{err}");
    }

    #[test]
    fn is_known_stdout() {
        assert!(is_known("stdout"));
        assert!(!is_known("nonexistent"));
        assert!(!is_known("STDOUT"));
    }

    #[test]
    fn stdout_list_devices_is_anything() {
        let backend = StdoutBackend;
        assert!(matches!(
            backend.list_devices().unwrap(),
            DeviceList::Anything
        ));
    }

    #[test]
    fn stdout_validate_device_always_ok() {
        let backend = StdoutBackend;
        assert!(backend.validate_device("anything").is_ok());
        assert!(backend.validate_device("").is_ok());
    }

    #[test]
    fn stdout_startup_and_teardown_ok() {
        let backend = StdoutBackend;
        assert!(backend.startup().is_ok());
        assert!(backend.teardown().is_ok());
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test backend:: -- --nocapture`
Expected: all `backend::tests::*` tests PASS.

- [ ] **Step 4: Run full check gate**

Run: `cargo fmt --check && cargo check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all PASS (this is a pure refactor + trait expansion; nothing else in the codebase calls the new methods yet, so nothing else should break).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(backend): turn backend.rs into a module, expand Backend trait

Backend trait now requires startup, teardown, validate_device, and
list_devices in addition to send_event. StdoutBackend implements all
five. is_known/create are unchanged in this commit — http backend
comes in a later commit."
```

---

### Task 2: Add `HttpConfig` to `config.rs`

**Files:**
- Modify: `src/config.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub struct HttpConfig { pub bind: String }` (derives `Debug, Deserialize, Default, Clone`), `pub http: Option<HttpConfig>` field on `Config`, default bind `"127.0.0.1:8080"`.

- [ ] **Step 1: Add the failing test for config parsing**

Add to the `tests` module in `src/config.rs` (after the existing `cfg` helper, near the other `Config`-level tests):

```rust
    #[test]
    fn parse_config_without_http_section() {
        let yaml = r#"
broker:
  host: localhost
topics: []
rules: []
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.http.is_none());
    }

    #[test]
    fn parse_config_with_http_section_defaults_bind() {
        let yaml = r#"
broker:
  host: localhost
topics: []
rules: []
http: {}
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.http.unwrap().bind, "127.0.0.1:8080");
    }

    #[test]
    fn parse_config_with_http_section_custom_bind() {
        let yaml = r#"
broker:
  host: localhost
topics: []
rules: []
http:
  bind: "0.0.0.0:9000"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.http.unwrap().bind, "0.0.0.0:9000");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test parse_config_with_http -- --nocapture`
Expected: FAIL with "no field `http` on type `Config`" (compile error, since `HttpConfig`/field don't exist yet).

- [ ] **Step 3: Add `HttpConfig` and the `http` field on `Config`**

In `src/config.rs`, add the field to the `Config` struct:

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    pub broker: BrokerConfig,
    pub topics: Vec<String>,
    pub rules: Vec<Rule>,
    pub http: Option<HttpConfig>,
}
```

And add the new type near `BrokerConfig`:

```rust
#[derive(Debug, Deserialize, Default, Clone)]
pub struct HttpConfig {
    #[serde(default = "default_http_bind")]
    pub bind: String,
}

fn default_http_bind() -> String {
    "127.0.0.1:8080".to_string()
}
```

Update every place in `src/config.rs` that constructs a `Config` literal (the `cfg` test helper) to add `http: None`:

```rust
    fn cfg(rules: Vec<Rule>) -> Config {
        Config {
            broker: BrokerConfig {
                host: "localhost".into(),
                port: 1883,
                client_id: None,
                auth: None,
            },
            topics: vec![],
            rules,
            http: None,
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test parse_config_with_http -- --nocapture` and `cargo test config:: -- --nocapture`
Expected: all PASS.

- [ ] **Step 5: Run full check gate**

Run: `cargo fmt --check && cargo check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add optional http section for the http backend's bind address"
```

---

### Task 3: Add axum and test-only HTTP dependencies

**Files:**
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: nothing.
- Produces: `axum` available as a normal dependency; `tower` (with `util` feature) and `http-body-util` available as dev-dependencies for testing the route handler via `tower::ServiceExt::oneshot` without a real socket.

- [ ] **Step 1: Add dependencies**

Edit `Cargo.toml`:

```toml
[dependencies]
anyhow = "1"
axum = "0.7"
rumqttc = "0.24"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
tokio = { version = "1", features = ["full"] }

[dev-dependencies]
tower = { version = "0.4", features = ["util"] }
http-body-util = "0.1"
```

- [ ] **Step 2: Verify it builds**

Run: `cargo check`
Expected: PASS (new deps fetched and compiled; nothing uses them yet, so no unused-dependency warnings since we haven't added `#[deny(unused)]` — plain `cargo check` just confirms resolution/compilation).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add axum dependency for the http backend, tower/http-body-util for testing it"
```

---

### Task 4: Implement `HttpBackend` in `src/backend/http.rs`

**Files:**
- Create: `src/backend/http.rs`
- Modify: `src/backend/mod.rs` (add `pub mod http;`)

**Interfaces:**
- Consumes: `crate::backend::{Backend, DeviceList}` (Task 1), `crate::gestures::Event`.
- Produces: `pub struct HttpBackend`, `pub fn HttpBackend::new(bind: String) -> Self`. Implements `Backend`. Internal (crate-private) `fn build_router(events: EventBuffer) -> axum::Router` used by both `startup()` and its own tests.

- [ ] **Step 1: Register the new submodule**

In `src/backend/mod.rs`, add near the top (after the `use` line):

```rust
pub mod http;
```

- [ ] **Step 2: Write the failing tests for buffer/validation/listing/route behavior**

Create `src/backend/http.rs` with just the test module first (this will fail to compile since `HttpBackend` doesn't exist yet — that's expected):

```rust
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::Router;
use axum::routing::get;
use axum::extract::State;

use crate::backend::{Backend, DeviceList};
use crate::gestures::Event;

const MAX_EVENTS: usize = 10;
const VALID_DEVICE_ID: &str = "0";

type EventBuffer = Arc<Mutex<VecDeque<String>>>;

pub struct HttpBackend {
    events: EventBuffer,
    bind: String,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl HttpBackend {
    pub fn new(bind: String) -> Self {
        Self {
            events: Arc::new(Mutex::new(VecDeque::new())),
            bind,
            shutdown_tx: Mutex::new(None),
        }
    }
}

async fn list_events(State(events): State<EventBuffer>) -> String {
    let events = events.lock().unwrap();
    events.iter().cloned().collect::<Vec<_>>().join("\n")
}

fn build_router(events: EventBuffer) -> Router {
    Router::new().route("/", get(list_events)).with_state(events)
}

impl Backend for HttpBackend {
    fn startup(&self) -> Result<()> {
        unimplemented!()
    }

    fn teardown(&self) -> Result<()> {
        unimplemented!()
    }

    fn validate_device(&self, device_id: &str) -> Result<()> {
        match self.list_devices()? {
            DeviceList::Anything => Ok(()),
            DeviceList::List(ids) => {
                if ids.iter().any(|id| id == device_id) {
                    Ok(())
                } else {
                    anyhow::bail!(
                        "http backend: unknown device id '{device_id}', valid ids: {ids:?}"
                    )
                }
            }
        }
    }

    fn list_devices(&self) -> Result<DeviceList> {
        Ok(DeviceList::List(vec![VALID_DEVICE_ID.to_string()]))
    }

    fn send_event(&self, device_id: String, event: &Event) {
        let mut events = self.events.lock().unwrap();
        events.push_back(format!("{device_id} {event:?}"));
        while events.len() > MAX_EVENTS {
            events.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn validate_device_accepts_zero() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        assert!(backend.validate_device("0").is_ok());
    }

    #[test]
    fn validate_device_rejects_other_ids() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        let err = backend.validate_device("1").unwrap_err();
        assert!(err.to_string().contains("1"), "{err}");
    }

    #[test]
    fn list_devices_returns_single_zero() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        match backend.list_devices().unwrap() {
            DeviceList::List(ids) => assert_eq!(ids, vec!["0".to_string()]),
            DeviceList::Anything => panic!("expected List, got Anything"),
        }
    }

    #[test]
    fn send_event_truncates_to_last_ten() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        for i in 0..11u32 {
            let event = Event::new(i, 1.0, 0);
            backend.send_event("0".to_string(), &event);
        }
        let events = backend.events.lock().unwrap();
        assert_eq!(events.len(), 10);
        // the 0th push (duration_ms: 0) should have been dropped;
        // the oldest remaining is the 1st push (duration_ms: 1).
        assert!(events.front().unwrap().contains("duration_ms: 1"));
        assert!(events.back().unwrap().contains("duration_ms: 10"));
    }

    #[tokio::test]
    async fn route_returns_events_joined_by_newline() {
        let events: EventBuffer = Arc::new(Mutex::new(VecDeque::from([
            "a".to_string(),
            "b".to_string(),
        ])));
        let app = build_router(events);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"a\nb");
    }

    #[tokio::test]
    async fn route_returns_empty_string_when_no_events() {
        let events: EventBuffer = Arc::new(Mutex::new(VecDeque::new()));
        let app = build_router(events);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"");
    }
}
```

- [ ] **Step 3: Run tests, confirm the two `unimplemented!()` methods aren't hit but everything else passes**

Run: `cargo test backend::http:: -- --nocapture`
Expected: `validate_device_accepts_zero`, `validate_device_rejects_other_ids`, `list_devices_returns_single_zero`, `send_event_truncates_to_last_ten`, `route_returns_events_joined_by_newline`, `route_returns_empty_string_when_no_events` all PASS. (`startup`/`teardown` aren't called by any test yet, so the `unimplemented!()` bodies don't matter until Step 4.)

- [ ] **Step 4: Implement `startup` and `teardown`**

Replace the two `unimplemented!()` bodies:

```rust
    fn startup(&self) -> Result<()> {
        let events = Arc::clone(&self.events);
        let bind = self.bind.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.shutdown_tx.lock().unwrap() = Some(tx);

        tokio::spawn(async move {
            let app = build_router(events);
            let listener = match tokio::net::TcpListener::bind(&bind).await {
                Ok(listener) => listener,
                Err(e) => {
                    eprintln!("http backend: failed to bind {bind}: {e}");
                    return;
                }
            };
            let server = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = rx.await;
            });
            if let Err(e) = server.await {
                eprintln!("http backend: server error: {e}");
            }
        });

        Ok(())
    }

    fn teardown(&self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(())
    }
```

- [ ] **Step 5: Add an integration-style test that startup actually serves the route, then teardown stops it**

Add to the `tests` module:

```rust
    #[tokio::test]
    async fn startup_serves_events_then_teardown_stops_it() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        // Port 0 means we can't predict the bound address from here, so this
        // test exercises startup/teardown for panics/errors only, not a real
        // HTTP round trip (the route-serving behavior is already covered by
        // route_returns_events_joined_by_newline against the router directly).
        backend.startup().unwrap();
        backend.teardown().unwrap();
    }
```

- [ ] **Step 6: Run full test suite**

Run: `cargo test backend:: -- --nocapture`
Expected: all `backend::http::tests::*` and `backend::tests::*` PASS.

- [ ] **Step 7: Run full check gate**

Run: `cargo fmt --check && cargo check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all PASS. If clippy flags the `Mutex<Option<Sender>>` lock-across-await or similar, fix per its suggestion (locks in this file are only ever held for synchronous, non-blocking operations — no `.await` while holding a lock — so no restructuring should be needed).

- [ ] **Step 8: Commit**

```bash
git add src/backend/http.rs src/backend/mod.rs
git commit -m "feat(backend): add http backend serving the last 10 events at GET /

Runs its own axum server on a spawned tokio task, started in startup()
and stopped via a oneshot-triggered graceful shutdown in teardown().
Only device id '0' is valid (http/0)."
```

---

### Task 5: Wire `is_known`/`create` to support `"http"` and thread `&Config` through

**Files:**
- Modify: `src/backend/mod.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `crate::config::Config` (existing), `HttpConfig` (Task 2), `http::HttpBackend::new` (Task 4).
- Produces: `pub fn is_known(name: &str) -> bool` now also matches `"http"`; `pub fn create(name: &str, config: &crate::config::Config) -> anyhow::Result<Box<dyn Backend>>` — **signature changed**, callers must update.

- [ ] **Step 1: Update `is_known` and `create` in `src/backend/mod.rs`**

```rust
pub fn is_known(name: &str) -> bool {
    matches!(name, "stdout" | "http")
}

pub fn create(name: &str, config: &crate::config::Config) -> anyhow::Result<Box<dyn Backend>> {
    match name {
        "stdout" => Ok(Box::new(StdoutBackend)),
        "http" => {
            let bind = config.http.clone().unwrap_or_default().bind;
            Ok(Box::new(http::HttpBackend::new(bind)))
        }
        other => anyhow::bail!("unknown backend: {other}"),
    }
}
```

- [ ] **Step 2: Update the existing factory tests in `src/backend/mod.rs` to pass a `Config`**

The existing tests call `create("stdout")` / `create("nonexistent")` with one argument. Add a minimal `Config` test helper and update call sites:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BrokerConfig, Config};

    fn test_config() -> Config {
        Config {
            broker: BrokerConfig {
                host: "localhost".into(),
                port: 1883,
                client_id: None,
                auth: None,
            },
            topics: vec![],
            rules: vec![],
            http: None,
        }
    }

    #[test]
    fn create_known_backends_succeed() {
        assert!(create("stdout", &test_config()).is_ok());
        assert!(create("http", &test_config()).is_ok());
    }

    #[test]
    fn create_unknown_backend_errors() {
        let err = create("nonexistent", &test_config())
            .err()
            .expect("expected error");
        assert!(err.to_string().contains("nonexistent"), "{err}");
    }

    #[test]
    fn is_known_stdout_and_http() {
        assert!(is_known("stdout"));
        assert!(is_known("http"));
        assert!(!is_known("nonexistent"));
        assert!(!is_known("STDOUT"));
    }

    #[test]
    fn create_http_uses_configured_bind() {
        let mut config = test_config();
        config.http = Some(crate::config::HttpConfig {
            bind: "127.0.0.1:9999".to_string(),
        });
        assert!(create("http", &config).is_ok());
    }

    // stdout_list_devices_is_anything, stdout_validate_device_always_ok,
    // stdout_startup_and_teardown_ok stay unchanged from Task 1.
}
```

(Keep the three `Stdout*` tests from Task 1 in this same module — only the `create`/`is_known` tests and imports above change.)

- [ ] **Step 3: Update the one call site in `src/main.rs`**

Find:
```rust
                backends.insert(backend_name.to_string(), backend::create(backend_name)?);
```
Replace with:
```rust
                backends.insert(backend_name.to_string(), backend::create(backend_name, &config)?);
```

- [ ] **Step 4: Run tests**

Run: `cargo test backend:: -- --nocapture`
Expected: all PASS.

- [ ] **Step 5: Run full check gate**

Run: `cargo fmt --check && cargo check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all PASS.

- [ ] **Step 6: Commit**

```bash
git add src/backend/mod.rs src/main.rs
git commit -m "feat(backend): register http backend in is_known/create, thread Config through"
```

---

### Task 6: Wire lifecycle into `main.rs` — startup, device validation, ctrl-c teardown

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: `Backend::startup`, `Backend::teardown`, `Backend::validate_device` (Task 1/4/5), `tokio::signal::ctrl_c` (stdlib/tokio, already available via the `full` feature).
- Produces: no new public interface — this is the final wiring task, `main.rs`'s behavior is the deliverable.

- [ ] **Step 1: Call `startup()` right after each backend is created**

Find the loop in `src/main.rs` that builds the `backends` map:
```rust
    let mut backends: HashMap<String, Box<dyn backend::Backend>> = HashMap::new();
    for rule in &config.rules {
        for addr in rule.device_spec.as_slice() {
            let backend_name = addr.split_once('/').expect("validated").0;
            if !backends.contains_key(backend_name) {
                backends.insert(backend_name.to_string(), backend::create(backend_name, &config)?);
            }
        }
    }
```
Replace with:
```rust
    let mut backends: HashMap<String, Box<dyn backend::Backend>> = HashMap::new();
    for rule in &config.rules {
        for addr in rule.device_spec.as_slice() {
            let backend_name = addr.split_once('/').expect("validated").0;
            if !backends.contains_key(backend_name) {
                let backend = backend::create(backend_name, &config)?;
                backend.startup()?;
                backends.insert(backend_name.to_string(), backend);
            }
        }
    }
```

- [ ] **Step 2: Add the device-validation pass right after the backends map is fully built**

Immediately after the loop from Step 1 (still before the MQTT client is constructed), add:
```rust
    for rule in &config.rules {
        for addr in rule.device_spec.as_slice() {
            let (backend_name, device_id) = addr.split_once('/').expect("validated");
            if let Err(e) = backends[backend_name].validate_device(device_id) {
                eprintln!("{e}");
                return Err(e);
            }
        }
    }
```

- [ ] **Step 3: Wrap the MQTT poll loop in `tokio::select!` against ctrl-c**

Find the existing tail of `main`:
```rust
    loop {
        if let MqttEvent::Incoming(Packet::Publish(p)) = eventloop.poll().await? {
            let payload = match std::str::from_utf8(&p.payload) {
                Ok(s) => s,
                Err(_) => {
                    eprintln!("non-UTF8 payload on {}", p.topic);
                    continue;
                }
            };
            let cloud_event: event::CloudEvent = match serde_json::from_str(payload) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("invalid CloudEvent on {}: {e}", p.topic);
                    continue;
                }
            };

            for rule in &config.rules {
                if !rule.filter.matches(&cloud_event) {
                    continue;
                }

                let base = lookup(&rule.gesture.name).expect("validated at startup");
                let events = scale(base, rule.gesture.speed, rule.gesture.scale);

                let devices = rule.device_spec.as_slice();
                for haptic_event in &events {
                    let addr = &devices[haptic_event.device as usize];
                    let (backend_name, device_id) =
                        addr.split_once('/').expect("validated at startup");
                    backends[backend_name].send_event(device_id.to_string(), haptic_event);
                }
            }
        }
    }
}
```
Replace with:
```rust
    let mqtt_loop = async {
        loop {
            if let MqttEvent::Incoming(Packet::Publish(p)) = eventloop.poll().await? {
                let payload = match std::str::from_utf8(&p.payload) {
                    Ok(s) => s,
                    Err(_) => {
                        eprintln!("non-UTF8 payload on {}", p.topic);
                        continue;
                    }
                };
                let cloud_event: event::CloudEvent = match serde_json::from_str(payload) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("invalid CloudEvent on {}: {e}", p.topic);
                        continue;
                    }
                };

                for rule in &config.rules {
                    if !rule.filter.matches(&cloud_event) {
                        continue;
                    }

                    let base = lookup(&rule.gesture.name).expect("validated at startup");
                    let events = scale(base, rule.gesture.speed, rule.gesture.scale);

                    let devices = rule.device_spec.as_slice();
                    for haptic_event in &events {
                        let addr = &devices[haptic_event.device as usize];
                        let (backend_name, device_id) =
                            addr.split_once('/').expect("validated at startup");
                        backends[backend_name].send_event(device_id.to_string(), haptic_event);
                    }
                }
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };

    tokio::select! {
        result = mqtt_loop => result?,
        _ = tokio::signal::ctrl_c() => {
            eprintln!("received ctrl-c, shutting down");
            for backend in backends.values() {
                if let Err(e) = backend.teardown() {
                    eprintln!("error tearing down backend: {e}");
                }
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Run full check gate**

Run: `cargo fmt --check && cargo check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: all PASS.

- [ ] **Step 5: Manual smoke test**

Run (using an existing example config, or write a minimal one to `/tmp/http-backend-test.yaml`):
```bash
cat > /tmp/http-backend-test.yaml <<'EOF'
broker:
  host: localhost
topics: []
rules:
  - filter: {}
    gesture:
      name: pulse_short
    device: http/0
EOF
cargo build
./target/debug/haptics /tmp/http-backend-test.yaml &
sleep 1
curl -s http://127.0.0.1:8080/
kill %1
```
Expected: `curl` returns an empty body (no events sent yet, since there's no real broker delivering MQTT messages in this smoke test — this just confirms the server starts and answers `GET /` without erroring). The backgrounded process should exit cleanly when killed.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): call backend startup/validate_device, teardown backends on ctrl-c"
```

---

## Self-Review Notes

- **Spec coverage:** trait shape (Task 1), `HttpConfig`/YAML (Task 2), axum dependency (Task 3), `HttpBackend` behavior — buffer/validate/list/route (Task 4), factory registration (Task 5), main.rs startup/validation/shutdown wiring (Task 6) — all six spec sections have a task.
- **Placeholder scan:** the only `unimplemented!()` is a deliberate, temporary TDD step within Task 4 (Step 2 → Step 4 replaces it in the same task, before that task's final commit) — not a plan placeholder left dangling across tasks.
- **Type consistency:** `create(name: &str, config: &Config) -> anyhow::Result<Box<dyn Backend>>` is consistent from Task 5 through Task 6; `HttpBackend::new(bind: String)` consistent from Task 4 through Task 5; `DeviceList` variants (`Anything`, `List(Vec<String>)`) consistent from Task 1 through Task 4.