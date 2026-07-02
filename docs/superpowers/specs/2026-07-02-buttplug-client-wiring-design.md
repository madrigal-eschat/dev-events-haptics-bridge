# Buttplug Backend: Wire Up Real Connection

**Date:** 2026-07-02
**Scope:** Replace the buttplug backend's hand-rolled, never-connected protocol stub with a real connection to Intiface/Buttplug servers using the official `buttplug_client` crate.
**Status:** Draft — pending review
**Supersedes:** Protocol/dependency sections of `2026-06-30-buttplug-backend-design.md`. That spec's `ButtplugBackend` sync facade, `WorkerState` machine, `DeviceLookup`, percent-encoding, and device-address format (`BACKEND/lookup/actuator`) are all still correct and unchanged — only the never-implemented "ConnectionManager" pieces (WebSocket I/O, device scan, protocol serialization, command translation) are being replaced.

---

## Why

`src/backend/buttplug.rs` currently has a fully-tested job queue, device cache, and actuator-selection model — but nothing ever populates the device cache or sends a command anywhere. `handle_job` looks a device up, builds a `ButtplugCommand`, and `eprintln!`/`log::debug!`s it. No network I/O happens. This spec wires up the missing half: an actual connection to an Intiface server, device discovery, and real command dispatch.

## What changed since the original spec

The original spec (2026-06-30) predates this investigation and assumed:
1. Hand-rolled Buttplug JSON protocol over `tokio-tungstenite`.
2. Protocol v2-style `VibrateCmd`/`RotateCmd`/`LinearCmd` with `RotateCmd{speed, clockwise, duration_ms}`.

Investigation of the current (v10.0.3) `buttplug` crate ecosystem found:
1. **Use the official `buttplug_client` crate** instead of hand-rolling the protocol — it, plus `buttplug_transport_websocket_tungstenite`, provide `ButtplugClient`/`ButtplugClientDevice` with connect/scan/device-list/command-send already implemented and protocol-version-tracked upstream.
2. **Protocol v4's `OutputCmd` has no rotate direction** — `OutputCommand::Rotate` is documented "Single Direction Rotation Speed". The old spec's `clockwise` field has nothing to bind to. This design drops rotate direction; `Rotate` becomes magnitude-only, matching `Vibrate`.
3. **`ButtplugClient` instances are single-use**: `connect()` takes an internal `mpsc::Receiver` out of a `Mutex<Option<_>>` before attempting the connection; if the connect attempt fails, the receiver is already gone and that client can never connect. Every connection attempt (including reconnects after backoff) needs a fresh `ButtplugClient::new(...)`.
4. **`ButtplugClientDevice` has no public constructor.** Our existing unit tests build fake devices directly (`seed_device`/`seed_device_aliases`) to test actuator-selection logic without a server. That pattern can't produce a real `ButtplugClientDevice`. Design keeps our own `DeviceInfo`/`Actuator` cache (plain data, still test-constructible) separate from the live client; the live client is only consulted at the final "send the command" step.

---

## Dependencies

Add to `Cargo.toml`:

```toml
buttplug_client = "10"
buttplug_core = "10"
buttplug_transport_websocket_tungstenite = "10"
futures = "0.3"   # for event_stream StreamExt
```

`buttplug` (the meta-crate bundling client+server) is **not** used — we only need the client-side pieces, keeping the dependency tree smaller and matching how upstream's own client examples import (`buttplug_client::...`, not `buttplug::client::...`).

---

## Architecture

### Two layers, kept separate

1. **Selection layer (pure, unit-tested, unchanged in shape):**
   `DeviceCache = HashMap<DeviceLookup, Arc<DeviceInfo>>`, `DeviceInfo { device_index: u32, actuators: Vec<Actuator> }`, `Actuator { index: u32, kind: ActuatorKind }`, `ActuatorKind { Vibrate, Linear, Rotate }`. `choose_actuator`, `parse_device_id`, `format_device_id`, percent-encoding — all unchanged. `device_index` is a new field (the server-assigned device index needed to look the live device back up); tests that don't care about it just set it to `0`.

2. **Dispatch layer (needs a live `ButtplugClient`):**
   At send time, given a resolved `(device_index, feature_index, kind)`, look up the real device via `client.devices().get(&device_index)`, then `.device_features().get(&feature_index)`, then call `.run_output(&cmd)`. This hop only exists inside the connection manager and is covered by the mock-server integration test, not unit tests.

### Classifying a real device's features into our `ActuatorKind`

For each `ClientDeviceFeature` on a newly-seen device, classify in priority order (same priority the existing `choose_actuator` sort already assumes — `Vibrate < Linear < Rotate`):

```
if feature.feature().contains_output(OutputType::Vibrate)               -> ActuatorKind::Vibrate
else if feature.feature().contains_output(OutputType::HwPositionWithDuration) -> ActuatorKind::Linear
else if feature.feature().contains_output(OutputType::Rotate)           -> ActuatorKind::Rotate
else                                                                      -> feature excluded (unsupported output type, e.g. Constrict/Led/Spray/Temperature/Oscillate/Position)
```

A feature supporting multiple of these three (unusual) is classified by the first match in that order, same as before.

### Connection manager (replaces the current no-op inside `run_worker`)

Runs inside the existing dedicated OS thread + current-thread `tokio::Runtime` (unchanged `WorkerState`/`startup`/`teardown` machinery). Loop:

```
loop {
    if shutdown requested -> break
    client = ButtplugClient::new("haptics-bridge")   // fresh every attempt
    connector = build_connector(&config.server)       // ws:// -> new_insecure_connector, wss:// -> new_secure_connector
    match timeout(config.connection_timeout_ms, client.connect(connector)).await {
        Ok(Ok(())) => {
            log::info!("buttplug backend: connected to {}", config.server);
            backoff.reset();
            run_connected_session(&client, &device_cache, &mut job_rx, &mut shutdown_rx, &config).await;
            // returns when disconnected or shutdown requested
        }
        Ok(Err(e)) => log::warn!("buttplug backend: connect failed: {e}"),
        Err(_timeout) => log::warn!("buttplug backend: connect timed out after {}ms", config.connection_timeout_ms),
    }
    if shutdown requested -> break
    sleep(backoff.next()).await   // 1s, 2s, 4s, ... capped at max_backoff_ms
}
```

`run_connected_session`:
- Subscribes to `client.event_stream()` **after** `connect()` returns (per the actual upstream example ordering — `connect()`'s internal handshake already populates `client.devices()` via `HandleDeviceList`, so nothing is lost by subscribing after; the stream is only needed for *later* changes).
- Immediately resyncs the device cache from `client.devices()` (full rebuild — see below) to seed initial state.
- Calls `client.start_scanning()` once, then re-calls it every `scan_interval_ms` on a `tokio::time::interval`. We do not call `stop_scanning()` — real BLE managers apply their own scan windows/timeouts; we just periodically ask for another scan pass.
- `tokio::select!`s over: job queue (drain and dispatch — see below), event stream (on any `DeviceAdded`/`DeviceRemoved`, resync cache from `client.devices()`), scan interval tick (`start_scanning()`, log errors), shutdown signal, and `ServerDisconnect`/`ClientDisconnect`-equivalent event (break out of the session loop so the outer loop reconnects).

**Cache resync = full rebuild** (default, no response received to confirm — flag for review): on any device-list-affecting event, rebuild the whole `DeviceCache` from `client.devices()` and atomically swap it in under the existing `Mutex`. Device count is small (a handful of toys); this avoids incremental-update bugs (name collisions on add, partial state on remove) for negligible cost.

**Per-job dispatch = spawned, not awaited inline** (default, no response received to confirm — flag for review): draining the job queue, each job's actual `run_output(...).await` is `tokio::spawn`ed rather than awaited in the select loop, so one slow/stuck device can't delay gesture delivery to other devices. Errors from the spawned send are logged from within the task. This matches the existing fire-and-forget contract (`Backend::send_event` already doesn't wait for backend acks).

### Job dispatch (`handle_job`, replacing the current stub)

```
fn handle_job(cache: &DeviceCache, client: &ButtplugClient, job: GestureJob) {
    let Some(device_info) = cache.get(&job.lookup) else { log unknown-lookup + list known devices (existing behavior); return };
    let actuator = resolve actuator via job.actuator_index or choose_actuator(...)  // unchanged logic
    let Some(actuator) = actuator else { log no-usable-actuators; return };
    let Some(cmd) = translate_event(actuator.kind, &job.event) else { log unsupported-kind; return };  // unchanged
    let Some(device) = client.devices().get(&device_info.device_index) else { log device-vanished (race), drop; return };
    let Some(feature) = device.device_features().get(&actuator.index) else { log feature-vanished (race), drop; return };
    tokio::spawn(async move {
        if let Err(e) = feature.run_output(&cmd.into()).await {
            log::warn!("buttplug backend: send failed for device {device_index} feature {actuator_index}: {e}");
        }
    });
}
```

`ButtplugCommand` (our existing internal enum: `Vibrate{magnitude,frequency}`, `Linear{position,duration_ms}`, `Rotate{speed,clockwise,duration_ms}`) is simplified — `Rotate` drops `clockwise` and `frequency` on `Vibrate` is dropped too (the real protocol has no separate frequency parameter; only a single scalar value per output). New shape:

```rust
enum ButtplugCommand {
    Vibrate { magnitude: f32 },
    Linear { position: f32, duration_ms: u32 },
    Rotate { speed: f32 },
}
```

Conversion to `ClientDeviceOutputCommand`:
- `Vibrate{magnitude}` → `ClientDeviceOutputCommand::Vibrate(ClientDeviceCommandValue::Percent(magnitude as f64))`
- `Rotate{speed}` → `ClientDeviceOutputCommand::Rotate(ClientDeviceCommandValue::Percent(speed as f64))`
- `Linear{position, duration_ms}` → `ClientDeviceOutputCommand::HwPositionWithDuration(ClientDeviceCommandValue::Percent(position as f64), duration_ms)`

`translate_event` stays the same shape (`ActuatorKind` in, `ButtplugCommand` out, driven off `Event{duration_ms, magnitude, device}`), just with the trimmed fields above. `rotate_clockwise_from_event` and its tests are deleted.

### Telemetry

Extend `ButtplugTelemetry` with two new counters (existing four — `processed_jobs`, `dropped_unstarted`, `dropped_invalid_device`, `dropped_full` — are unchanged and still mean what they mean today):

- `dropped_disconnected` — job reached the worker but there was no active connection at the time (mirrors the connection-manager equivalent of today's `dropped_unstarted`, which only tracks "worker thread not running" not "running but not connected").
- `dropped_unknown_device` — device or feature vanished from `client.devices()` between resolve and dispatch (race with a `DeviceRemoved` event).

---

## Config

No changes — `ButtplugConfig{server, connection_timeout_ms, max_backoff_ms, scan_interval_ms}` already has everything needed (`src/config.rs`). `server` is used as-is for the connector address; scheme (`ws://` vs `wss://`) picks `new_insecure_connector`/`new_secure_connector`.

---

## Testing

### Unit tests (unchanged approach, still no network)
- All existing `resolve_device_ids`/`choose_actuator`/`parse_device_id`/`percent_encode`/telemetry tests keep working unmodified against the selection layer.
- `translate_event` tests updated for the trimmed `ButtplugCommand` shape; `rotate_direction_is_even_slot_clockwise_and_negative_overrides` deleted along with `rotate_clockwise_from_event`.
- New: feature-classification unit test (`Vibrate`+`HwPositionWithDuration`+`Rotate`+unsupported-type-excluded) — this one doesn't need a real device, just a function `classify(&[OutputType]) -> Option<ActuatorKind>` taking the raw output-type set, which is what the real classification code calls per feature.

### Integration tests (new, default picked without confirmation — flag for review)
A small hand-rolled mock WebSocket server (`tokio-tungstenite`, test-only code, not a new runtime dependency — already pulled in transitively) that speaks just enough Buttplug JSON to drive the connection manager end-to-end:
- Responds to `RequestServerInfo` with `ServerInfo`.
- Responds to `RequestDeviceList` with a `DeviceList` containing one fake device with a `Vibrate` feature.
- Responds to `OutputCmd` with `Ok`.
- Optionally emits a `DeviceAdded`/`DeviceRemoved` push message to test cache resync.

Covers: connect → device appears in cache → gesture dispatched → mock server received the expected `OutputCmd` JSON. Also: mock server closes connection → client reconnects with backoff → mock server comes back → device rediscovered.

### Existing tests
`startup_and_teardown_are_safe_before_connection` and friends (worker lifecycle tests) — still valid, since `WorkerState` transitions are unchanged; they just now exercise the new connection-manager loop instead of the old no-op `run_worker`. Config validation and backend-factory tests unaffected.

---

## Non-Goals (carried over from original spec, still true)

- Intiface Core embedding (connect to external server only).
- Device pairing/permissions (assume pre-paired in Intiface).
- Rotate direction (removed — protocol doesn't support it, see above).
- Web dashboard or monitoring beyond logging.
- Sensor/input features (battery, RSSI, etc.) — output-only, matches current scope.

## Success Criteria

1. Bridge connects to a real Intiface Central/Engine instance on startup, or backs off and retries if unavailable.
2. Gestures dispatched via the buttplug backend reach connected devices (verified against real hardware or Intiface's test device manager manually; verified end-to-end against the mock server in CI).
3. Device lookup still works by index and by name, addresses unchanged (`buttplug/0`, `buttplug/Lovense%20Nora`, `buttplug/Lovense%20Nora/1`).
4. Connection recovers automatically after a server restart (fresh `ButtplugClient` per attempt, exponential backoff, cache resynced on reconnect).
5. All errors logged; no panics or hanging threads; teardown remains clean (existing `WorkerState` shutdown path unchanged).
6. Existing unit test suite passes with only the noted `ButtplugCommand`/rotate-direction changes; new integration test covers connect/scan/dispatch/reconnect against the mock server.
