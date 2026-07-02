# Buttplug Client Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the buttplug backend's never-connected job queue with a real connection to an Intiface/Buttplug server, using the official `buttplug_client` crate (v10), so gestures dispatched to `buttplug/...` device addresses actually reach hardware.

**Architecture:** Two layers stay separate. The existing pure selection layer (`DeviceLookup`, `DeviceInfo`, `Actuator`, `choose_actuator`, percent-encoding) is unchanged in shape (one new field) and stays fully unit-testable without a network connection. A new connection-manager task (spawned from the existing worker thread's `run_worker`) owns the live `ButtplugClient`, publishes it via a `watch` channel, and is exercised end-to-end by a hand-rolled mock WebSocket server built from the real `buttplug_core` message structs (not raw JSON strings) in integration tests.

**Tech Stack:** `buttplug_client`, `buttplug_core`, `buttplug_transport_websocket_tungstenite` (all v10, official Buttplug Rust crates), `futures` (for `StreamExt`), `tokio-tungstenite` (dev-dependency, for the test mock server).

## Global Constraints

- Follow `docs/superpowers/specs/2026-07-02-buttplug-client-wiring-design.md` exactly; it supersedes the protocol/dependency sections of `docs/superpowers/specs/2026-06-30-buttplug-backend-design.md`.
- `cargo fmt`, `cargo check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must all pass before every commit (per `CLAUDE.md`).
- Rotate has no direction in protocol v4 — do not reintroduce `clockwise`.
- Device address wire format is unchanged: `buttplug/<index-or-percent-encoded-name>[/<feature-index>]`.
- Every connection attempt (including reconnects) must construct a fresh `ButtplugClient::new(...)` — instances are single-use once `connect()` is called, success or failure.
- All new async work runs on the existing current-thread runtime inside the worker's dedicated OS thread (`ButtplugBackend::startup_with_runtime`) — do not introduce a second runtime or thread pool.

---

## File Structure

- Modify: `Cargo.toml` — add `buttplug_client`, `buttplug_core`, `buttplug_transport_websocket_tungstenite`, `futures`; add `tokio-tungstenite` to `[dev-dependencies]`.
- Modify: `src/backend/buttplug.rs` — trim `ButtplugCommand`/`translate_event`, add `device_index` to `DeviceInfo`, add `classify_output_types`, add `is_secure_websocket`/`Backoff`, extend telemetry, rewrite `handle_job`, add `connection_manager`, rewire `run_worker`/`startup_with_runtime`.
- Create: `src/backend/buttplug_mock_server.rs` — test-only mock Buttplug WebSocket server, `#[cfg(test)]`-gated, `mod`-included from `buttplug.rs`'s test module.

---

### Task 1: Add buttplug client dependencies

**Files:**
- Modify: `Cargo.toml`

**Interfaces:**
- Produces: `buttplug_client`, `buttplug_core`, `buttplug_transport_websocket_tungstenite`, `futures` available as crates in `src/backend/buttplug.rs`; `tokio-tungstenite` available in `#[cfg(test)]` code.

- [ ] **Step 1: Edit `Cargo.toml`**

Change:

```toml
[dependencies]
anyhow = "1"
axum = "0.7"
env_logger = "0.11"
log = "0.4"
rumqttc = "0.24"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
tokio = { version = "1", features = ["full"] }

[dev-dependencies]
tower = { version = "0.4", features = ["util"] }
http-body-util = "0.1"
```

to:

```toml
[dependencies]
anyhow = "1"
axum = "0.7"
buttplug_client = "10"
buttplug_core = "10"
buttplug_transport_websocket_tungstenite = "10"
env_logger = "0.11"
futures = "0.3"
log = "0.4"
rumqttc = "0.24"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
strum = "0.26"
tokio = { version = "1", features = ["full"] }

[dev-dependencies]
tower = { version = "0.4", features = ["util"] }
http-body-util = "0.1"
tokio-tungstenite = "0.24"
```

- [ ] **Step 2: Fetch and check**

Run: `cargo check`
Expected: Compiles successfully (new deps unused yet is fine; `cargo check` only fails on unused-import warnings if `-D warnings` is passed, which it isn't here).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add buttplug_client and websocket transport dependencies"
```

---

### Task 2: Trim `ButtplugCommand`/`translate_event`, drop rotate direction

Protocol v4's `Rotate` output has no direction field (see design doc). Simplify our internal command enum to match, and delete the now-meaningless direction heuristic.

**Files:**
- Modify: `src/backend/buttplug.rs`

**Interfaces:**
- Produces: `ButtplugCommand::{Vibrate{magnitude: f32}, Linear{position: f32, duration_ms: u32}, Rotate{speed: f32}}`, `fn translate_event(kind: ActuatorKind, event: &Event) -> Option<ButtplugCommand>` (signature unchanged).

- [ ] **Step 1: Update the failing tests first**

Find and replace the three `translate_*_event_into_*_command` tests and delete `rotate_direction_is_even_slot_clockwise_and_negative_overrides`:

```rust
    #[test]
    fn translate_rotate_event_uses_magnitude_as_speed() {
        let event = Event::new(80, 0.75, 1);
        assert_eq!(
            translate_event(ActuatorKind::Rotate, &event),
            Some(ButtplugCommand::Rotate { speed: 0.75 })
        );

        let event = Event::new(80, -0.75, 0);
        assert_eq!(
            translate_event(ActuatorKind::Rotate, &event),
            Some(ButtplugCommand::Rotate { speed: 0.75 })
        );
    }

    #[test]
    fn translate_linear_event_into_linear_command() {
        let event = Event::new(80, 0.75, 0);
        assert_eq!(
            translate_event(ActuatorKind::Linear, &event),
            Some(ButtplugCommand::Linear {
                position: 0.75,
                duration_ms: 80,
            })
        );
    }

    #[test]
    fn translate_vibrate_event_into_vibrate_command() {
        let event = Event::new(80, 0.50, 0);
        assert_eq!(
            translate_event(ActuatorKind::Vibrate, &event),
            Some(ButtplugCommand::Vibrate { magnitude: 0.50 })
        );
    }
```

Delete the `rotate_direction_is_even_slot_clockwise_and_negative_overrides` test entirely (it tested `rotate_clockwise_from_event`, which is being deleted).

- [ ] **Step 2: Run tests to verify they fail to compile (types don't match yet)**

Run: `cargo test translate_ 2>&1 | tail -30`
Expected: Compile errors — `ButtplugCommand::Rotate` doesn't have a `speed`-only variant yet, `ButtplugCommand::Vibrate` still expects `frequency`.

- [ ] **Step 3: Trim `ButtplugCommand` and `translate_event`**

Replace:

```rust
#[derive(Debug, Clone, PartialEq)]
enum ButtplugCommand {
    Vibrate {
        magnitude: f32,
        frequency: f32,
    },
    Linear {
        position: f32,
        duration_ms: u32,
    },
    Rotate {
        speed: f32,
        clockwise: bool,
        duration_ms: u32,
    },
}
```

with:

```rust
#[derive(Debug, Clone, PartialEq)]
enum ButtplugCommand {
    Vibrate { magnitude: f32 },
    Linear { position: f32, duration_ms: u32 },
    Rotate { speed: f32 },
}
```

Replace:

```rust
fn translate_event(kind: ActuatorKind, event: &Event) -> Option<ButtplugCommand> {
    let magnitude = event.magnitude.abs().clamp(0.0, 1.0);
    Some(match kind {
        ActuatorKind::Vibrate => ButtplugCommand::Vibrate {
            magnitude,
            frequency: magnitude,
        },
        ActuatorKind::Linear => ButtplugCommand::Linear {
            position: magnitude,
            duration_ms: event.duration_ms,
        },
        ActuatorKind::Rotate => ButtplugCommand::Rotate {
            speed: magnitude,
            clockwise: rotate_clockwise_from_event(event),
            duration_ms: event.duration_ms,
        },
    })
}

fn rotate_clockwise_from_event(event: &Event) -> bool {
    !event.magnitude.is_sign_negative() && event.device.is_multiple_of(2)
}
```

with:

```rust
fn translate_event(kind: ActuatorKind, event: &Event) -> Option<ButtplugCommand> {
    let magnitude = event.magnitude.abs().clamp(0.0, 1.0);
    Some(match kind {
        ActuatorKind::Vibrate => ButtplugCommand::Vibrate { magnitude },
        ActuatorKind::Linear => ButtplugCommand::Linear {
            position: magnitude,
            duration_ms: event.duration_ms,
        },
        ActuatorKind::Rotate => ButtplugCommand::Rotate { speed: magnitude },
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test translate_ 2>&1 | tail -30`
Expected: `translate_rotate_event_uses_magnitude_as_speed`, `translate_linear_event_into_linear_command`, `translate_vibrate_event_into_vibrate_command` all PASS. No test named `rotate_direction_is_even_slot_clockwise_and_negative_overrides` should appear (it's deleted).

- [ ] **Step 5: Full test suite still green**

Run: `cargo test 2>&1 | tail -15`
Expected: all pass (99 → should still be ~98 after removing one test; exact count doesn't matter, just no failures).

- [ ] **Step 6: Commit**

```bash
git add src/backend/buttplug.rs
git commit -m "refactor: drop rotate direction from ButtplugCommand (protocol v4 has none)"
```

---

### Task 3: Add `device_index` to `DeviceInfo`, update test helpers

The selection-layer cache needs a way to find the real device later without holding a live handle to it. Add the server-assigned device index as plain data.

**Files:**
- Modify: `src/backend/buttplug.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: `struct DeviceInfo { device_index: u32, actuators: Vec<Actuator> }` (was `struct DeviceInfo { actuators: Vec<Actuator> }`), `fn seed_device(&self, lookup: DeviceLookup, device_index: u32, actuator_kinds: Vec<ActuatorKind>)`, `fn seed_device_aliases(&self, lookups: Vec<DeviceLookup>, device_index: u32, actuator_kinds: Vec<ActuatorKind>)`.

- [ ] **Step 1: Update `DeviceInfo` struct**

Replace:

```rust
#[derive(Debug, Clone, Default)]
struct DeviceInfo {
    actuators: Vec<Actuator>,
}
```

with:

```rust
#[derive(Debug, Clone, Default)]
struct DeviceInfo {
    device_index: u32,
    actuators: Vec<Actuator>,
}
```

- [ ] **Step 2: Update `seed_device`/`seed_device_aliases` test helpers to take a device index**

Replace:

```rust
    #[cfg(test)]
    fn seed_device(&self, lookup: DeviceLookup, actuator_kinds: Vec<ActuatorKind>) {
        self.seed_device_aliases(vec![lookup], actuator_kinds);
    }

    #[cfg(test)]
    fn seed_device_aliases(&self, lookups: Vec<DeviceLookup>, actuator_kinds: Vec<ActuatorKind>) {
        let actuators = actuator_kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| Actuator {
                index: index as u32,
                kind,
            })
            .collect();
        let device = Arc::new(DeviceInfo { actuators });
        let mut cache = self.device_cache.lock().unwrap();
        for lookup in lookups {
            cache.insert(lookup, Arc::clone(&device));
        }
    }
```

with:

```rust
    #[cfg(test)]
    fn seed_device(&self, lookup: DeviceLookup, device_index: u32, actuator_kinds: Vec<ActuatorKind>) {
        self.seed_device_aliases(vec![lookup], device_index, actuator_kinds);
    }

    #[cfg(test)]
    fn seed_device_aliases(
        &self,
        lookups: Vec<DeviceLookup>,
        device_index: u32,
        actuator_kinds: Vec<ActuatorKind>,
    ) {
        let actuators = actuator_kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| Actuator {
                index: index as u32,
                kind,
            })
            .collect();
        let device = Arc::new(DeviceInfo {
            device_index,
            actuators,
        });
        let mut cache = self.device_cache.lock().unwrap();
        for lookup in lookups {
            cache.insert(lookup, Arc::clone(&device));
        }
    }
```

- [ ] **Step 3: Fix every call site to pass a device index**

Run: `cargo build --tests 2>&1 | grep -B2 "seed_device"` to find every caller. Update each `backend.seed_device(lookup, kinds)` call to `backend.seed_device(lookup, 0, kinds)` and each `backend.seed_device_aliases(lookups, kinds)` to `backend.seed_device_aliases(lookups, 0, kinds)` — the device index value doesn't matter for the pure selection-logic tests (0 for all of them is fine; they don't exercise the real client lookup).

This affects these existing tests (verify each compiles after the fix): `resolve_device_ids_prefers_unused_actuators_by_priority`, `resolve_device_ids_falls_back_to_priority_when_all_are_taken`, `resolve_device_ids_leaves_unknown_and_zero_actuator_devices_unchanged`, `resolve_device_ids_leaves_unknown_explicit_actuator_unresolved`, `resolve_device_ids_leaves_explicit_actuator_on_zero_actuator_device_unresolved`, `resolve_device_ids_keeps_selection_scoped_to_each_lookup`, `startup_processes_queued_events_and_teardown_stops_processing`, `startup_failure_does_not_leave_backend_half_initialized`.

- [ ] **Step 4: Run full test suite**

Run: `cargo test 2>&1 | tail -15`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/backend/buttplug.rs
git commit -m "refactor: add device_index to DeviceInfo for real-device lookup later"
```

---

### Task 4: Add `classify_output_types` pure function

Given the set of `OutputType`s a real device feature supports, decide which `ActuatorKind` (if any) it maps to — same priority order the existing `choose_actuator` sort already assumes.

**Files:**
- Modify: `src/backend/buttplug.rs`

**Interfaces:**
- Consumes: `buttplug_core::message::OutputType` (new import).
- Produces: `fn classify_output_types(outputs: &[OutputType]) -> Option<ActuatorKind>`.

- [ ] **Step 1: Remove the now-stale dead-code allowance on `ActuatorKind`**

`ActuatorKind` is about to be used unconditionally by production code (in `classify_output_types`), not just under `#[cfg(test)]`. Replace:

```rust
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
enum ActuatorKind {
    Vibrate,
    Linear,
    Rotate,
}
```

with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
enum ActuatorKind {
    Vibrate,
    Linear,
    Rotate,
}
```

- [ ] **Step 2: Add the import**

Add near the top of `src/backend/buttplug.rs`, alongside the existing `use` block:

```rust
use buttplug_core::message::OutputType;
```

- [ ] **Step 3: Write the failing test**

Add to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn classify_output_types_prefers_vibrate() {
        assert_eq!(
            classify_output_types(&[OutputType::Vibrate, OutputType::Rotate]),
            Some(ActuatorKind::Vibrate)
        );
    }

    #[test]
    fn classify_output_types_falls_back_to_linear_then_rotate() {
        assert_eq!(
            classify_output_types(&[OutputType::HwPositionWithDuration]),
            Some(ActuatorKind::Linear)
        );
        assert_eq!(
            classify_output_types(&[OutputType::Rotate]),
            Some(ActuatorKind::Rotate)
        );
        assert_eq!(
            classify_output_types(&[OutputType::HwPositionWithDuration, OutputType::Rotate]),
            Some(ActuatorKind::Linear)
        );
    }

    #[test]
    fn classify_output_types_excludes_unsupported_outputs() {
        assert_eq!(
            classify_output_types(&[OutputType::Led, OutputType::Constrict]),
            None
        );
        assert_eq!(classify_output_types(&[]), None);
    }
```

- [ ] **Step 4: Run to verify it fails**

Run: `cargo test classify_output_types 2>&1 | tail -20`
Expected: compile error, `classify_output_types` not found.

- [ ] **Step 5: Implement**

Add near `translate_event`:

```rust
fn classify_output_types(outputs: &[OutputType]) -> Option<ActuatorKind> {
    if outputs.contains(&OutputType::Vibrate) {
        Some(ActuatorKind::Vibrate)
    } else if outputs.contains(&OutputType::HwPositionWithDuration) {
        Some(ActuatorKind::Linear)
    } else if outputs.contains(&OutputType::Rotate) {
        Some(ActuatorKind::Rotate)
    } else {
        None
    }
}
```

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test classify_output_types 2>&1 | tail -20`
Expected: 3 tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/backend/buttplug.rs
git commit -m "feat: classify real device feature output types into ActuatorKind"
```

---

### Task 5: Add `is_secure_websocket` and `Backoff` pure helpers

Small, independently-testable pieces the connection manager will use.

**Files:**
- Modify: `src/backend/buttplug.rs`

**Interfaces:**
- Produces: `fn is_secure_websocket(server: &str) -> bool`, `struct Backoff { .. }` with `fn new(max_ms: u64) -> Self`, `fn reset(&mut self)`, `fn next(&mut self) -> std::time::Duration`.

- [ ] **Step 1: Write failing tests**

```rust
    #[test]
    fn is_secure_websocket_detects_wss_scheme() {
        assert!(is_secure_websocket("wss://example.com:12345"));
        assert!(!is_secure_websocket("ws://example.com:12345"));
        assert!(!is_secure_websocket("ws://localhost:12345"));
    }

    #[test]
    fn backoff_doubles_up_to_max_then_holds() {
        let mut backoff = Backoff::new(8_000);
        assert_eq!(backoff.next(), Duration::from_millis(1_000));
        assert_eq!(backoff.next(), Duration::from_millis(2_000));
        assert_eq!(backoff.next(), Duration::from_millis(4_000));
        assert_eq!(backoff.next(), Duration::from_millis(8_000));
        assert_eq!(backoff.next(), Duration::from_millis(8_000));
    }

    #[test]
    fn backoff_reset_returns_to_initial_delay() {
        let mut backoff = Backoff::new(30_000);
        backoff.next();
        backoff.next();
        backoff.reset();
        assert_eq!(backoff.next(), Duration::from_millis(1_000));
    }

    #[test]
    fn backoff_max_below_initial_delay_is_clamped_to_max() {
        let mut backoff = Backoff::new(500);
        assert_eq!(backoff.next(), Duration::from_millis(500));
        assert_eq!(backoff.next(), Duration::from_millis(500));
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test backoff_ is_secure_websocket 2>&1 | tail -20`
Expected: compile errors, `Backoff`/`is_secure_websocket` not found.

- [ ] **Step 3: Implement**

Add near the top-level functions (after `format_device_id`, before the `percent_decode` helpers, or any similar spot):

```rust
fn is_secure_websocket(server: &str) -> bool {
    server.starts_with("wss://")
}

struct Backoff {
    initial_ms: u64,
    current_ms: u64,
    max_ms: u64,
}

impl Backoff {
    fn new(max_ms: u64) -> Self {
        let initial_ms = 1_000u64.min(max_ms.max(1));
        Self {
            initial_ms,
            current_ms: initial_ms,
            max_ms: max_ms.max(initial_ms),
        }
    }

    fn reset(&mut self) {
        self.current_ms = self.initial_ms;
    }

    fn next(&mut self) -> Duration {
        let delay = Duration::from_millis(self.current_ms);
        self.current_ms = (self.current_ms * 2).min(self.max_ms);
        delay
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test backoff_ is_secure_websocket 2>&1 | tail -20`
Expected: 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/backend/buttplug.rs
git commit -m "feat: add websocket scheme detection and reconnect backoff helpers"
```

---

### Task 6: Extend telemetry with `dropped_disconnected`/`dropped_unknown_device`

**Files:**
- Modify: `src/backend/buttplug.rs`

**Interfaces:**
- Produces: `ButtplugTelemetry` gains `dropped_disconnected: usize`, `dropped_unknown_device: usize`. `ButtplugBackend` gains `dropped_disconnected: Arc<AtomicUsize>`, `dropped_unknown_device: Arc<AtomicUsize>` (note: `Arc`-wrapped, unlike the existing plain `AtomicUsize` fields — these two get cloned into the worker's async tasks, same pattern as the existing `processed_jobs` field).

- [ ] **Step 1: Write failing test**

```rust
    #[test]
    fn telemetry_snapshot_starts_at_zero_for_new_counters() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        let telemetry = backend.telemetry_snapshot();
        assert_eq!(telemetry.dropped_disconnected, 0);
        assert_eq!(telemetry.dropped_unknown_device, 0);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test telemetry_snapshot_starts_at_zero 2>&1 | tail -20`
Expected: compile error, no field `dropped_disconnected` on `ButtplugTelemetry`.

- [ ] **Step 3: Extend `ButtplugTelemetry`**

Replace:

```rust
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ButtplugTelemetry {
    processed_jobs: usize,
    dropped_unstarted: usize,
    dropped_invalid_device: usize,
    dropped_full: usize,
}
```

with:

```rust
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ButtplugTelemetry {
    processed_jobs: usize,
    dropped_unstarted: usize,
    dropped_invalid_device: usize,
    dropped_full: usize,
    dropped_disconnected: usize,
    dropped_unknown_device: usize,
}
```

- [ ] **Step 4: Extend `ButtplugBackend` struct, `new()`, and `telemetry_snapshot()`**

Replace:

```rust
pub struct ButtplugBackend {
    pub config: ButtplugConfig,
    device_cache: Arc<Mutex<DeviceCache>>,
    state: Mutex<WorkerState>,
    processed_jobs: Arc<AtomicUsize>,
    dropped_unstarted: AtomicUsize,
    dropped_invalid_device: AtomicUsize,
    dropped_full: AtomicUsize,
}

impl ButtplugBackend {
    pub fn new(config: ButtplugConfig) -> Self {
        Self {
            config,
            device_cache: Arc::new(Mutex::new(HashMap::new())),
            state: Mutex::new(WorkerState::Stopped),
            processed_jobs: Arc::new(AtomicUsize::new(0)),
            dropped_unstarted: AtomicUsize::new(0),
            dropped_invalid_device: AtomicUsize::new(0),
            dropped_full: AtomicUsize::new(0),
        }
    }
```

with:

```rust
pub struct ButtplugBackend {
    pub config: ButtplugConfig,
    device_cache: Arc<Mutex<DeviceCache>>,
    state: Mutex<WorkerState>,
    processed_jobs: Arc<AtomicUsize>,
    dropped_unstarted: AtomicUsize,
    dropped_invalid_device: AtomicUsize,
    dropped_full: AtomicUsize,
    dropped_disconnected: Arc<AtomicUsize>,
    dropped_unknown_device: Arc<AtomicUsize>,
}

impl ButtplugBackend {
    pub fn new(config: ButtplugConfig) -> Self {
        Self {
            config,
            device_cache: Arc::new(Mutex::new(HashMap::new())),
            state: Mutex::new(WorkerState::Stopped),
            processed_jobs: Arc::new(AtomicUsize::new(0)),
            dropped_unstarted: AtomicUsize::new(0),
            dropped_invalid_device: AtomicUsize::new(0),
            dropped_full: AtomicUsize::new(0),
            dropped_disconnected: Arc::new(AtomicUsize::new(0)),
            dropped_unknown_device: Arc::new(AtomicUsize::new(0)),
        }
    }
```

Replace:

```rust
    pub fn telemetry_snapshot(&self) -> ButtplugTelemetry {
        ButtplugTelemetry {
            processed_jobs: self.processed_jobs.load(Ordering::SeqCst),
            dropped_unstarted: self.dropped_unstarted.load(Ordering::SeqCst),
            dropped_invalid_device: self.dropped_invalid_device.load(Ordering::SeqCst),
            dropped_full: self.dropped_full.load(Ordering::SeqCst),
        }
    }
```

with:

```rust
    pub fn telemetry_snapshot(&self) -> ButtplugTelemetry {
        ButtplugTelemetry {
            processed_jobs: self.processed_jobs.load(Ordering::SeqCst),
            dropped_unstarted: self.dropped_unstarted.load(Ordering::SeqCst),
            dropped_invalid_device: self.dropped_invalid_device.load(Ordering::SeqCst),
            dropped_full: self.dropped_full.load(Ordering::SeqCst),
            dropped_disconnected: self.dropped_disconnected.load(Ordering::SeqCst),
            dropped_unknown_device: self.dropped_unknown_device.load(Ordering::SeqCst),
        }
    }
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test telemetry_snapshot_starts_at_zero 2>&1 | tail -20`
Expected: PASS. Also run full suite (`cargo test 2>&1 | tail -15`) since existing telemetry tests construct `ButtplugTelemetry` — check whether any test builds one with a struct literal (search `ButtplugTelemetry {`); if any does, it will fail to compile until the two new fields are added there too. (As of this plan's writing, no test constructs `ButtplugTelemetry` directly — they all go through `telemetry_snapshot()`/`telemetry()` — so this should just work.)

- [ ] **Step 6: Commit**

```bash
git add src/backend/buttplug.rs
git commit -m "feat: add dropped_disconnected/dropped_unknown_device telemetry counters"
```

---

### Task 7: Rewrite `handle_job` for real dispatch

`handle_job` now needs the live client (if any) to actually send commands, and increments the two new counters instead of just logging.

**Files:**
- Modify: `src/backend/buttplug.rs`

**Interfaces:**
- Consumes: `buttplug_client::{ButtplugClient, device::{ClientDeviceCommandValue, ClientDeviceOutputCommand}}` (new imports), `classify_output_types` (Task 4), `Backoff`/`is_secure_websocket` (Task 5, used later in Task 8, not here).
- Produces: `fn handle_job(device_cache: &Arc<Mutex<DeviceCache>>, client: Option<&Arc<ButtplugClient>>, dropped_disconnected: &AtomicUsize, dropped_unknown_device: &AtomicUsize, job: GestureJob)` (was `fn handle_job(device_cache: &Arc<Mutex<DeviceCache>>, job: GestureJob)` — signature changes, all 3 call sites in `run_worker` must be updated in Task 8).

- [ ] **Step 1: Add imports**

Add alongside the existing `use` block:

```rust
use buttplug_client::ButtplugClient;
use buttplug_client::device::{ClientDeviceCommandValue, ClientDeviceOutputCommand};
```

- [ ] **Step 2: Replace `handle_job`**

Replace the entire current function:

```rust
fn handle_job(device_cache: &Arc<Mutex<DeviceCache>>, job: GestureJob) {
    let cache = device_cache.lock().unwrap();
    let Some(device) = cache.get(&job.lookup) else {
        let known: Vec<&DeviceLookup> = cache.keys().collect();
        log::error!(
            "buttplug backend: unknown device lookup {:?} (known devices: {:?})",
            job.lookup,
            known
        );
        return;
    };

    let actuator = match job.actuator_index {
        Some(index) => device
            .actuators
            .iter()
            .copied()
            .find(|actuator| actuator.index == index),
        None => choose_actuator(&device.actuators, None),
    };

    let Some(actuator) = actuator else {
        eprintln!(
            "buttplug backend: device {:?} has no usable actuators",
            job.lookup
        );
        return;
    };

    let Some(command) = translate_event(actuator.kind, &job.event) else {
        eprintln!(
            "buttplug backend: unsupported actuator kind {:?} for device {:?}",
            actuator.kind, job.lookup
        );
        return;
    };

    log::debug!(
        "buttplug backend: dispatch {:?} actuator {} as {:?}",
        job.lookup, actuator.index, command
    );
}
```

with:

```rust
fn handle_job(
    device_cache: &Arc<Mutex<DeviceCache>>,
    client: Option<&Arc<ButtplugClient>>,
    dropped_disconnected: &AtomicUsize,
    dropped_unknown_device: &AtomicUsize,
    job: GestureJob,
) {
    let device_index = {
        let cache = device_cache.lock().unwrap();
        let Some(device) = cache.get(&job.lookup) else {
            let known: Vec<&DeviceLookup> = cache.keys().collect();
            log::error!(
                "buttplug backend: unknown device lookup {:?} (known devices: {:?})",
                job.lookup,
                known
            );
            return;
        };

        let actuator = match job.actuator_index {
            Some(index) => device
                .actuators
                .iter()
                .copied()
                .find(|actuator| actuator.index == index),
            None => choose_actuator(&device.actuators, None),
        };

        let Some(actuator) = actuator else {
            log::warn!(
                "buttplug backend: device {:?} has no usable actuators",
                job.lookup
            );
            return;
        };

        let Some(command) = translate_event(actuator.kind, &job.event) else {
            log::warn!(
                "buttplug backend: unsupported actuator kind {:?} for device {:?}",
                actuator.kind, job.lookup
            );
            return;
        };

        log::debug!(
            "buttplug backend: dispatch {:?} actuator {} as {:?}",
            job.lookup, actuator.index, command
        );

        (device.device_index, actuator.index, command)
    };
    let (device_index, feature_index, command) = device_index;

    let Some(client) = client else {
        let dropped = dropped_disconnected.fetch_add(1, Ordering::SeqCst) + 1;
        log::warn!(
            "buttplug backend: dropped job for device {:?} (not connected, dropped_disconnected={dropped})",
            job.lookup
        );
        return;
    };

    let Some(device) = client.devices().get(&device_index).cloned() else {
        let dropped = dropped_unknown_device.fetch_add(1, Ordering::SeqCst) + 1;
        log::warn!(
            "buttplug backend: device index {device_index} not present on server (dropped_unknown_device={dropped})"
        );
        return;
    };

    let Some(feature) = device.device_features().get(&feature_index).cloned() else {
        let dropped = dropped_unknown_device.fetch_add(1, Ordering::SeqCst) + 1;
        log::warn!(
            "buttplug backend: feature {feature_index} not present on device {device_index} (dropped_unknown_device={dropped})"
        );
        return;
    };

    let output_command = match command {
        ButtplugCommand::Vibrate { magnitude } => {
            ClientDeviceOutputCommand::Vibrate(ClientDeviceCommandValue::Percent(magnitude as f64))
        }
        ButtplugCommand::Rotate { speed } => {
            ClientDeviceOutputCommand::Rotate(ClientDeviceCommandValue::Percent(speed as f64))
        }
        ButtplugCommand::Linear {
            position,
            duration_ms,
        } => ClientDeviceOutputCommand::HwPositionWithDuration(
            ClientDeviceCommandValue::Percent(position as f64),
            duration_ms,
        ),
    };

    tokio::spawn(async move {
        if let Err(e) = feature.run_output(&output_command).await {
            log::warn!(
                "buttplug backend: send failed for device {device_index} feature {feature_index}: {e}"
            );
        }
    });
}
```

This won't compile yet — `run_worker` still calls `handle_job(&device_cache, job)` with the old 2-argument signature at two call sites, and there's no connection manager to produce a `client: Option<&Arc<ButtplugClient>>` from. Don't commit yet; continue straight into the next step, which rewrites `run_worker` and fixes both call sites in the same commit.

- [ ] **Step 3: Add the connection manager**

This is the core of the feature: a background async task that owns the live `ButtplugClient`, reconnects with backoff, scans for devices, and keeps the selection-layer `DeviceCache` in sync with reality. It publishes the current connected client (if any) via a `tokio::sync::watch` channel that the job-drain loop reads from on every job.

Add these imports alongside the existing ones:

```rust
use buttplug_client::connector::ButtplugRemoteClientConnector;
use buttplug_client::serializer::ButtplugClientJSONSerializer;
use buttplug_client::{ButtplugClientDevice, ButtplugClientEvent};
use buttplug_transport_websocket_tungstenite::ButtplugWebsocketClientTransport;
use futures::StreamExt;
use tokio::sync::watch;
```

Add this function (it builds a fresh connector for one connection attempt — insecure or secure based on the configured scheme):

```rust
fn build_connector(
    server: &str,
) -> ButtplugRemoteClientConnector<ButtplugWebsocketClientTransport, ButtplugClientJSONSerializer>
{
    let transport = if is_secure_websocket(server) {
        ButtplugWebsocketClientTransport::new_secure_connector(server, false)
    } else {
        ButtplugWebsocketClientTransport::new_insecure_connector(server)
    };
    ButtplugRemoteClientConnector::<ButtplugWebsocketClientTransport, ButtplugClientJSONSerializer>::new(
        transport,
    )
}
```

Add a helper that turns one `ButtplugClientDevice` into our selection-layer cache entries (device index, all its usable actuators classified via `classify_output_types`):

```rust
fn device_info_from_client_device(device: &ButtplugClientDevice) -> DeviceInfo {
    let actuators = device
        .device_features()
        .iter()
        .filter_map(|(feature_index, feature)| {
            let outputs: Vec<OutputType> = OutputType::iter()
                .filter(|output_type| feature.feature().contains_output(*output_type))
                .collect();
            classify_output_types(&outputs).map(|kind| Actuator {
                index: *feature_index,
                kind,
            })
        })
        .collect();
    DeviceInfo {
        device_index: device.index(),
        actuators,
    }
}
```

`OutputType::iter()` needs the `strum::IntoEnumIterator` trait in scope (the type derives `EnumIter` upstream, per `buttplug_core`'s `device_feature.rs`; `strum` was added to `Cargo.toml` in Task 1). Add the import:

```rust
use strum::IntoEnumIterator;
```

Add the cache-resync function (full rebuild from `client.devices()`, per the design doc's default):

```rust
fn resync_device_cache(device_cache: &Arc<Mutex<DeviceCache>>, client: &ButtplugClient) {
    let mut new_cache: DeviceCache = HashMap::new();
    for (_, device) in client.devices() {
        let info = Arc::new(device_info_from_client_device(&device));
        new_cache.insert(DeviceLookup::ByIndex(device.index()), Arc::clone(&info));
        new_cache.insert(DeviceLookup::ByName(device.name().clone()), info);
    }
    let count = new_cache.len();
    *device_cache.lock().unwrap() = new_cache;
    log::info!("buttplug backend: device cache resynced ({count} lookup entries)");
}
```

Add the connection manager itself:

```rust
async fn connection_manager(
    config: ButtplugConfig,
    device_cache: Arc<Mutex<DeviceCache>>,
    client_tx: watch::Sender<Option<Arc<ButtplugClient>>>,
) {
    let mut backoff = Backoff::new(config.max_backoff_ms);
    loop {
        let client = ButtplugClient::new("haptics-bridge");
        let connector = build_connector(&config.server);
        let connect_result = tokio::time::timeout(
            Duration::from_millis(config.connection_timeout_ms),
            client.connect(connector),
        )
        .await;

        match connect_result {
            Ok(Ok(())) => {
                log::info!("buttplug backend: connected to {}", config.server);
                backoff.reset();
                let client = Arc::new(client);
                resync_device_cache(&device_cache, &client);
                let _ = client_tx.send(Some(Arc::clone(&client)));
                run_connected_session(&client, &device_cache, &config).await;
                let _ = client_tx.send(None);
                log::warn!("buttplug backend: disconnected from {}", config.server);
            }
            Ok(Err(e)) => {
                log::warn!("buttplug backend: connect to {} failed: {e}", config.server);
            }
            Err(_) => {
                log::warn!(
                    "buttplug backend: connect to {} timed out after {}ms",
                    config.server, config.connection_timeout_ms
                );
            }
        }

        let delay = backoff.next();
        log::info!("buttplug backend: reconnecting to {} in {delay:?}", config.server);
        tokio::time::sleep(delay).await;
    }
}

async fn run_connected_session(
    client: &Arc<ButtplugClient>,
    device_cache: &Arc<Mutex<DeviceCache>>,
    config: &ButtplugConfig,
) {
    let mut events = client.event_stream();
    if let Err(e) = client.start_scanning().await {
        log::warn!("buttplug backend: start_scanning failed: {e}");
    }
    let mut scan_interval = tokio::time::interval(Duration::from_millis(config.scan_interval_ms));
    scan_interval.tick().await; // first tick fires immediately; we already scanned above

    loop {
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(ButtplugClientEvent::DeviceAdded(device)) => {
                        log::info!("buttplug backend: device added: {}", device.name());
                        resync_device_cache(device_cache, client);
                    }
                    Some(ButtplugClientEvent::DeviceRemoved(device)) => {
                        log::info!("buttplug backend: device removed: {}", device.name());
                        resync_device_cache(device_cache, client);
                    }
                    Some(ButtplugClientEvent::ServerDisconnect) => {
                        log::warn!("buttplug backend: server disconnected");
                        return;
                    }
                    Some(_) => {}
                    None => {
                        log::warn!("buttplug backend: event stream ended");
                        return;
                    }
                }
            }
            _ = scan_interval.tick() => {
                if let Err(e) = client.start_scanning().await {
                    log::warn!("buttplug backend: start_scanning failed: {e}");
                }
            }
        }
        if !client.connected() {
            return;
        }
    }
}
```

- [ ] **Step 4: Rewire `run_worker` to spawn the connection manager and feed `handle_job`**

Replace the entire current `run_worker` function:

```rust
async fn run_worker(
    mut rx: mpsc::Receiver<GestureJob>,
    mut shutdown_rx: oneshot::Receiver<()>,
    device_cache: Arc<Mutex<DeviceCache>>,
    processed_jobs: Arc<AtomicUsize>,
) {
    let mut shutdown_requested = false;
    loop {
        if shutdown_requested {
            match rx.try_recv() {
                Ok(job) => {
                    processed_jobs.fetch_add(1, Ordering::SeqCst);
                    handle_job(&device_cache, job);
                    continue;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        tokio::select! {
            _ = &mut shutdown_rx => {
                eprintln!("buttplug backend: shutdown requested");
                shutdown_requested = true;
            }
            maybe_job = rx.recv() => {
                let Some(job) = maybe_job else {
                    break;
                };
                processed_jobs.fetch_add(1, Ordering::SeqCst);
                handle_job(&device_cache, job);
            }
        }
    }
}
```

with:

```rust
async fn run_worker(
    mut rx: mpsc::Receiver<GestureJob>,
    mut shutdown_rx: oneshot::Receiver<()>,
    device_cache: Arc<Mutex<DeviceCache>>,
    processed_jobs: Arc<AtomicUsize>,
    dropped_disconnected: Arc<AtomicUsize>,
    dropped_unknown_device: Arc<AtomicUsize>,
    config: ButtplugConfig,
) {
    let (client_tx, client_rx) = watch::channel(None);
    let connection_task = tokio::spawn(connection_manager(
        config,
        Arc::clone(&device_cache),
        client_tx,
    ));

    let mut shutdown_requested = false;
    loop {
        if shutdown_requested {
            match rx.try_recv() {
                Ok(job) => {
                    processed_jobs.fetch_add(1, Ordering::SeqCst);
                    handle_job(
                        &device_cache,
                        client_rx.borrow().as_ref(),
                        &dropped_disconnected,
                        &dropped_unknown_device,
                        job,
                    );
                    continue;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        tokio::select! {
            _ = &mut shutdown_rx => {
                log::info!("buttplug backend: shutdown requested");
                shutdown_requested = true;
            }
            maybe_job = rx.recv() => {
                let Some(job) = maybe_job else {
                    break;
                };
                processed_jobs.fetch_add(1, Ordering::SeqCst);
                handle_job(
                    &device_cache,
                    client_rx.borrow().as_ref(),
                    &dropped_disconnected,
                    &dropped_unknown_device,
                    job,
                );
            }
        }
    }

    connection_task.abort();
}
```

- [ ] **Step 5: Thread the new parameters through `startup_with_runtime`**

Replace:

```rust
        let device_cache = Arc::clone(&self.device_cache);
        let processed_jobs = Arc::clone(&self.processed_jobs);
        let worker = std::thread::Builder::new()
            .name("buttplug-backend".to_string())
            .spawn(move || {
                runtime.block_on(run_worker(rx, shutdown_rx, device_cache, processed_jobs));
            })?;
```

with:

```rust
        let device_cache = Arc::clone(&self.device_cache);
        let processed_jobs = Arc::clone(&self.processed_jobs);
        let dropped_disconnected = Arc::clone(&self.dropped_disconnected);
        let dropped_unknown_device = Arc::clone(&self.dropped_unknown_device);
        let config = self.config.clone();
        let worker = std::thread::Builder::new()
            .name("buttplug-backend".to_string())
            .spawn(move || {
                runtime.block_on(run_worker(
                    rx,
                    shutdown_rx,
                    device_cache,
                    processed_jobs,
                    dropped_disconnected,
                    dropped_unknown_device,
                    config,
                ));
            })?;
```

- [ ] **Step 6: Fix test-hygiene — avoid real-network noise in lifecycle tests**

Tests that call the real `ButtplugBackend::startup()` (not the `install_running_worker`/`install_test_sender` bypasses) now spawn a real `connection_manager` that will try to connect to whatever `config.server` says. `ButtplugConfig::default()`'s `ws://localhost:12345` will fail fast in CI (nothing listens there) but there's no reason to depend on that — pin it to a guaranteed-fail-fast, deliberately-invalid address with a short timeout instead. Add this helper to the `#[cfg(test)] mod tests` block, near the top:

```rust
    fn no_server_config() -> ButtplugConfig {
        ButtplugConfig {
            server: "ws://127.0.0.1:0".to_string(),
            connection_timeout_ms: 50,
            max_backoff_ms: 100,
            scan_interval_ms: 60_000,
        }
    }
```

Update these tests to use `ButtplugBackend::new(no_server_config())` instead of `ButtplugBackend::new(ButtplugConfig::default())`: `startup_and_teardown_are_safe_before_connection`, `startup_processes_queued_events_and_teardown_stops_processing`, `startup_failure_does_not_leave_backend_half_initialized`, `startup_is_atomic_under_concurrent_calls`. (Tests using `install_running_worker`/`install_test_sender` never call real `startup()`/`run_worker` — leave those on `ButtplugConfig::default()`, it's inert there.)

- [ ] **Step 7: Run the full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: all pass. `startup_processes_queued_events_and_teardown_stops_processing` in particular should still show `processed_jobs == 16` — the job-drain loop increments `processed_jobs` on every dequeue regardless of connection state, unchanged from before.

- [ ] **Step 8: Lint and format**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings 2>&1 | tail -40`
Expected: clean. Fix anything clippy flags (e.g. needless clones) before proceeding — don't suppress with `#[allow]` unless the lint is a false positive.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml src/backend/buttplug.rs
git commit -m "feat: wire buttplug backend to a real Intiface connection

Adds a connection manager that connects with exponential backoff,
scans for devices, keeps the selection-layer device cache in sync
via a full rebuild on every DeviceAdded/DeviceRemoved event, and
dispatches gestures to the live device/feature via a watch channel
published from the connection manager to the job-drain loop."
```

---

### Task 8: Build the mock Buttplug WebSocket server

Rather than hand-writing raw protocol JSON (fragile — the client validates incoming messages against a JSON schema), construct real `buttplug_core` message structs and let `serde_json` serialize them. `ButtplugServerMessageV4`/`ButtplugClientMessageV4` are plain externally-tagged enums (`{"VariantName": {...}}`), which is exactly the wire format the protocol uses, confirmed against the upstream JSON serializer test fixtures.

**Files:**
- Create: `src/backend/buttplug_mock_server.rs`
- Modify: `src/backend/buttplug.rs` (add `#[cfg(test)] mod buttplug_mock_server;` near the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub struct MockButtplugServer { pub addr: std::net::SocketAddr, pub output_cmds: tokio::sync::mpsc::UnboundedReceiver<buttplug_core::message::OutputCommand> }`, `pub async fn start() -> MockButtplugServer`, `pub async fn start_on(addr: std::net::SocketAddr) -> MockButtplugServer`, `pub fn stop(self)` (drops the listener/connection task, aborting it).

- [ ] **Step 1: Create the mock server file**

```rust
//! Minimal Buttplug JSON-over-WebSocket mock server, test-only.
//!
//! Speaks just enough of the protocol (RequestServerInfo, RequestDeviceList,
//! StartScanning, OutputCmd) to drive `ButtplugBackend`'s connection manager
//! end-to-end without a real Intiface server. Responses are built from real
//! `buttplug_core` message structs and serialized with serde_json, rather
//! than hand-written JSON, so the wire format always matches what the
//! client's schema validator expects.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use buttplug_core::message::{
    ButtplugClientMessageV4, ButtplugMessage, ButtplugServerMessageV4, DeviceFeature,
    DeviceFeatureOutput, DeviceFeatureOutputValueProperties, DeviceListV4, DeviceMessageInfoV4,
    OkV0, OutputCommand, ServerInfoV4,
};
use buttplug_core::util::range::RangeInclusive;
use buttplug_core::util::small_vec_enum_map::SmallVecEnumMap;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

pub struct MockButtplugServer {
    pub addr: SocketAddr,
    pub output_cmds: mpsc::UnboundedReceiver<OutputCommand>,
    task: JoinHandle<()>,
}

impl MockButtplugServer {
    pub fn stop(self) {
        self.task.abort();
    }
}

fn fake_vibrator_device_list() -> DeviceListV4 {
    let output = SmallVecEnumMap::from(vec![DeviceFeatureOutput::Vibrate(
        DeviceFeatureOutputValueProperties::new(RangeInclusive::new(0, 20)),
    )]);
    let mut features: BTreeMap<u32, DeviceFeature> = BTreeMap::new();
    features.insert(
        0,
        DeviceFeature::new(0, "Vibrator", &output, &SmallVecEnumMap::from(Vec::new())),
    );
    let info = DeviceMessageInfoV4::new(0, "Test Vibrator", &None, 0, &features);
    DeviceListV4::new(vec![info])
}

pub async fn start() -> MockButtplugServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock server");
    start_with_listener(listener).await
}

pub async fn start_on(addr: SocketAddr) -> MockButtplugServer {
    let listener = TcpListener::bind(addr).await.expect("bind mock server on fixed addr");
    start_with_listener(listener).await
}

async fn start_with_listener(listener: TcpListener) -> MockButtplugServer {
    let addr = listener.local_addr().expect("mock server local addr");
    let (output_tx, output_rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        let (mut write, mut read) = ws.split();

        while let Some(Ok(Message::Text(text))) = read.next().await {
            let Ok(requests) = serde_json::from_str::<Vec<ButtplugClientMessageV4>>(&text) else {
                continue;
            };

            let mut responses: Vec<ButtplugServerMessageV4> = Vec::new();
            for request in requests {
                let id = request.id();
                match request {
                    ButtplugClientMessageV4::RequestServerInfo(_) => {
                        responses.push(ButtplugServerMessageV4::ServerInfo(ServerInfoV4::new(
                            "Mock Buttplug Server",
                            4,
                            0,
                            0,
                        )));
                    }
                    ButtplugClientMessageV4::RequestDeviceList(_) => {
                        let mut list = fake_vibrator_device_list();
                        list.set_id(id);
                        responses.push(ButtplugServerMessageV4::DeviceList(list));
                    }
                    ButtplugClientMessageV4::OutputCmd(cmd) => {
                        let _ = output_tx.send(cmd.command());
                        responses.push(ButtplugServerMessageV4::Ok(OkV0::new(id)));
                    }
                    _ => {
                        responses.push(ButtplugServerMessageV4::Ok(OkV0::new(id)));
                    }
                }
            }

            let Ok(json) = serde_json::to_string(&responses) else {
                continue;
            };
            if write.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    });

    MockButtplugServer {
        addr,
        output_cmds: output_rx,
        task,
    }
}
```

If any struct/method name here doesn't compile against the exact `buttplug_core`/`buttplug_client` version that resolves (this was verified against v10.0.3 source on GitHub, but a patch release could rename something): check `~/.cargo/registry/src/*/buttplug_core-*/src/message/` for the current definitions of `ServerInfoV4::new`, `DeviceListV4::new`, `DeviceMessageInfoV4::new`, `DeviceFeature::new`, and `OutputCmdV4::command()` (a `#[getset(get_copy = "pub")]` on the `command` field — confirm it's `Copy`-returning or switch to `.clone()` if it returns a reference).

- [ ] **Step 2: Register the module**

In `src/backend/buttplug.rs`, near the bottom (after the closing brace of `mod tests`, or as a sibling test-only module), add:

```rust
#[cfg(test)]
mod buttplug_mock_server;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo test --no-run 2>&1 | tail -40`
Expected: compiles cleanly (nothing exercises it yet — that's Task 9). Fix any type/method mismatches per Step 1's fallback note before moving on.

- [ ] **Step 4: Commit**

```bash
git add src/backend/buttplug.rs src/backend/buttplug_mock_server.rs
git commit -m "test: add mock Buttplug WebSocket server for integration tests"
```

---

### Task 9: Integration tests against the mock server

**Files:**
- Modify: `src/backend/buttplug.rs` (add tests to the existing `#[cfg(test)] mod tests` block)

**Interfaces:**
- Consumes: `buttplug_mock_server::{start, start_on, MockButtplugServer}` (Task 8), `ButtplugBackend::{new, startup, teardown, send_event}` (existing), `Event::new` (existing).

- [ ] **Step 1: Write the connect + discover + dispatch test**

```rust
    #[tokio::test]
    async fn connects_discovers_and_dispatches_to_mock_server() {
        let mut server = buttplug_mock_server::start().await;
        let config = ButtplugConfig {
            server: format!("ws://{}", server.addr),
            connection_timeout_ms: 2_000,
            max_backoff_ms: 1_000,
            scan_interval_ms: 60_000,
        };
        let backend = ButtplugBackend::new(config);
        backend.startup().unwrap();

        // Device discovery happens asynchronously after connect; poll briefly.
        let mut resolved = None;
        for _ in 0..100 {
            let ids = backend.resolve_device_ids(&["0".to_string()]);
            if let Ok(ids) = ids
                && ids.first().is_some_and(|id| id != "0")
            {
                resolved = Some(ids);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            resolved.is_some(),
            "expected device '0' to resolve to a real actuator id once discovered"
        );

        backend.send_event(resolved.unwrap()[0].clone(), &Event::new(50, 0.8, 0));

        let received = tokio::time::timeout(Duration::from_secs(2), server.output_cmds.recv())
            .await
            .expect("mock server should receive an OutputCmd within 2s")
            .expect("output_cmds channel should not close");
        match received {
            OutputCommand::Vibrate(value) => {
                assert!(value.value() > 0, "expected a nonzero vibrate value");
            }
            other => panic!("expected Vibrate output command, got {other:?}"),
        }

        backend.teardown().unwrap();
        server.stop();
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test connects_discovers_and_dispatches_to_mock_server -- --nocapture 2>&1 | tail -60`
Expected: PASS. If it fails at the "expected device '0' to resolve" assertion, check: (a) the mock server actually replied to `RequestDeviceList` (add `eprintln!` temporarily in the mock server's match arms to confirm messages are being received/matched), (b) `resync_device_cache` (Task 7) is being called after connect. If it fails at the "OutputCmd within 2s" step, check `handle_job`'s `client.devices().get(&device_index)` — confirm the resolved device id's index matches what the mock server reported (`0`, from `fake_vibrator_device_list`).

- [ ] **Step 3: Write the reconnect test**

```rust
    #[tokio::test]
    async fn reconnects_with_backoff_after_server_restart() {
        let server = buttplug_mock_server::start().await;
        let addr = server.addr;
        let config = ButtplugConfig {
            server: format!("ws://{addr}"),
            connection_timeout_ms: 500,
            max_backoff_ms: 300,
            scan_interval_ms: 60_000,
        };
        let backend = ButtplugBackend::new(config);
        backend.startup().unwrap();

        // Wait for the first connection to establish (device resolves).
        let mut first_resolved = false;
        for _ in 0..100 {
            if let Ok(ids) = backend.resolve_device_ids(&["0".to_string()])
                && ids.first().is_some_and(|id| id != "0")
            {
                first_resolved = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(first_resolved, "expected initial connection to discover the device");

        // Kill the server, wait past a backoff cycle, then bring it back on the same port.
        server.stop();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let _second_server = buttplug_mock_server::start_on(addr).await;

        // The connection manager should reconnect and rediscover the device on its own.
        let mut reconnected = false;
        for _ in 0..150 {
            if let Ok(ids) = backend.resolve_device_ids(&["0".to_string()])
                && ids.first().is_some_and(|id| id != "0")
            {
                reconnected = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(reconnected, "expected reconnection to rediscover the device after restart");

        backend.teardown().unwrap();
    }
```

- [ ] **Step 4: Run it**

Run: `cargo test reconnects_with_backoff_after_server_restart -- --nocapture 2>&1 | tail -60`
Expected: PASS. If rebinding the same port fails intermittently (OS-dependent `TIME_WAIT` behavior), increase the sleep after `server.stop()` to comfortably exceed `max_backoff_ms`, or note the flake and rerun — this is a known tradeoff of testing real socket reuse rather than mocking the transport layer.

- [ ] **Step 5: Run the full suite, fmt, clippy**

Run: `cargo fmt && cargo clippy --all-targets -- -D warnings 2>&1 | tail -40 && cargo test 2>&1 | tail -30`
Expected: everything clean and green.

- [ ] **Step 6: Commit**

```bash
git add src/backend/buttplug.rs
git commit -m "test: cover connect/discover/dispatch and reconnect against mock server"
```

---

### Task 10: Final verification pass

**Files:** none (verification only).

- [ ] **Step 1: Full clean build**

Run: `cargo clean && cargo build 2>&1 | tail -30`
Expected: builds successfully with the new dependencies resolved.

- [ ] **Step 2: Full gate**

Run: `cargo fmt --check && cargo check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | tail -60`
Expected: all four pass (matches the CI gate documented in `CLAUDE.md`).

- [ ] **Step 3: Manual smoke test against a real Intiface instance (optional but recommended)**

If Intiface Central is available: run `RUST_LOG=debug ./target/debug/haptics <config with a buttplug rule>` pointed at it, confirm `buttplug backend: connected to ...` and `buttplug backend: device cache resynced (...)` appear in the logs, and that a dispatched gesture reaches a real or simulated device. Not required for the plan to be considered complete (no test harness for real hardware), but worth doing once before calling this shippable.

- [ ] **Step 4: Update the design spec status**

Edit `docs/superpowers/specs/2026-07-02-buttplug-client-wiring-design.md`, change the header line:

```
**Status:** Draft — pending review
```

to:

```
**Status:** Implemented
```

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-07-02-buttplug-client-wiring-design.md
git commit -m "docs: mark buttplug client wiring spec as implemented"
```
