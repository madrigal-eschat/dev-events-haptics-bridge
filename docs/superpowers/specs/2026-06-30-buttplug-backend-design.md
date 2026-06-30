# Buttplug Backend Design

**Date:** 2026-06-30  
**Scope:** Add async-first WebSocket client backend for Intiface/buttplug.io  
**Status:** Design approved, ready for implementation

---

## Overview

This spec describes adding a **buttplug.io backend** to haptics-bridge that connects to an Intiface Central/Server instance via WebSocket. The backend will:

- Connect to Intiface with exponential backoff reconnection
- Scan available devices every 30 seconds
- Support flexible device addressing (by numeric index or name)
- Queue gestures asynchronously without blocking the main thread
- Log errors and continue gracefully (fire-and-forget semantics)

---

## Architecture

### New Files

```
src/backend/
  ├── mod.rs (existing, updated to include buttplug)
  └── buttplug.rs (new)
      ├── Connection manager (async task)
      ├── Device cache and scanning logic
      ├── Buttplug protocol serialization
      └── WebSocket I/O
```

### Modified Files

- `src/backend.rs` — Update `is_known()` and `create()` to support "buttplug" backend
- `src/main.rs` — Spawn connection manager task on startup (after config validation)
- `Cargo.toml` — Add `tokio-tungstenite` for WebSocket client

### Config Extension

Add optional `buttplug` section to YAML config:

```yaml
buttplug:
  server: "ws://localhost:12345"  # Default: ws://localhost:12345
  connection_timeout_ms: 5000     # Default: 5000
  max_backoff_ms: 30000           # Default: 30000
  scan_interval_ms: 30000         # Default: 30000
```

All fields optional with sensible defaults. Connection failure at startup does not halt the bridge; background task will keep reconnecting.

---

## Component Design

### 1. `ButtplugBackend` (Sync Facade)

Implements the `Backend` trait (sync, non-blocking):

```rust
pub struct ButtplugBackend {
    tx: mpsc::Sender<GestureJob>,
}

impl Backend for ButtplugBackend {
    fn send_event(&self, device_id: String, event: &Event) {
        // Parse device_id (e.g., "buttplug/0" or "buttplug/Lovense Nora")
        // Extract ID part, create GestureJob, send to channel
        // If channel full or closed: log error and return (fire-and-forget)
    }
}
```

### 2. `ConnectionManager` (Async Background Task)

Runs in a dedicated tokio task, manages:

- **WebSocket connection:** Connect, reconnect with backoff, maintain connection state
- **Device scanning:** Every 30s, query Intiface for device list and cache `(lookup → device_id)` mappings
- **Job queue:** Receive `GestureJob` from the sync backend, dispatch to Intiface
- **Error resilience:** All errors logged; no panics; connection failures trigger automatic reconnection

**Device Cache:**

```rust
type DeviceCache = HashMap<DeviceLookup, u32>;

enum DeviceLookup {
    ByIndex(u32),
    ByName(String),
}
```

On scan: query Intiface's device list, populate cache with all devices (numeric indices + names). On gesture dispatch: look up device in cache by parsed `DeviceLookup`; if not found, log warning and skip.

### 3. Gesture Translation

Buttplug devices support:
- **LinearCmd** — position (0.0–1.0) over duration
- **VibrateCmd** — frequency and magnitude
- **RotateCmd** — speed and direction

`Event` has `(duration_ms, magnitude, device_idx)`. Map:
- If device is linear: `LinearCmd(position=magnitude, duration_ms)`
- If device is vibration: `VibrateCmd(frequency=derived_from_magnitude, magnitude)`
- If device is rotary: `RotateCmd(speed=magnitude, direction=forward)`

Device capability detection happens during scan (cache includes device type). Unsupported command types logged as warnings.

### 4. Connection Lifecycle

**Startup:**
1. Read buttplug config (or use defaults)
2. Spawn `ConnectionManager` task
3. Task attempts connection immediately; if fails, backoff and retry
4. Bridge continues operating; queue jobs even if disconnected (logged as errors)

**Reconnection:**
- Exponential backoff: 1s → 2s → 4s → 8s → ... → capped at `max_backoff_ms` (default 30s)
- Backoff resets to 1s on successful connect
- On reconnect: rescan devices immediately

**Shutdown:**
- Main thread drop of `ButtplugBackend` closes the mpsc sender
- `ConnectionManager` detects channel closure, closes WebSocket, exits task
- Clean shutdown, no resource leaks

---

## Data Flow

```
┌─ Startup ─────────────────────────────┐
│ Config loaded                         │
│ Config::validate() runs               │
│ spawn_connection_manager() called     │
│ ConnectionManager task starts         │
│ Attempts WebSocket connect            │
│ (may fail; backoff/retry)             │
└───────────────────────────────────────┘
                  ↓
┌─ Steady State ────────────────────────┐
│ Device scan every 30s                 │
│ Main thread dispatches gestures       │
│ Gestures queued via mpsc::send()      │
│ ConnectionManager dequeues jobs       │
│ Look up device, send buttplug command │
│ Errors logged, task continues         │
└───────────────────────────────────────┘
                  ↓
┌─ On Gesture Dispatch ─────────────────┐
│ Rule matches → Backend::send_event()  │
│ Parse device_id (e.g., "buttplug/0")  │
│ Extract lookup (ByIndex or ByName)    │
│ Create GestureJob { lookup, event }   │
│ mpsc::send(job)                       │
│ Return immediately (fire-and-forget)  │
│                                       │
│ ConnectionManager (async):            │
│ Receive job from queue                │
│ Look up device in cache               │
│ If not found: log warning, skip       │
│ Translate Event → buttplug command    │
│ Send to WebSocket                     │
│ On error: log, continue               │
└───────────────────────────────────────┘
```

---

## Error Handling

### Connection Errors
- **Initial connect fails:** Backoff loop, keep retrying silently
- **Connection drops:** Log error, next job will trigger reconnect attempt
- **Reconnect succeeds:** Reset backoff timer, rescan devices

### Device Errors
- **Device not in cache:** Log warning, skip gesture (no error return)
- **Device type unsupported:** Log warning, skip command type (or attempt generic vibrate fallback)

### Queue Errors
- **Sender closed (bridge shutting down):** Log info, exit cleanly
- **Channel full (backpressure):** Log warning, drop job (indicates slow connection; may need tuning)

### WebSocket I/O Errors
- All I/O errors logged with context (connection ID, last command sent, etc.)
- No panics; task restarts connection attempt

---

## Testing

### Unit Tests
- **Device lookup:** Parse `"buttplug/0"`, `"buttplug/Lovense Nora"`, invalid formats
- **Event translation:** Verify `Event → LinearCmd`, `Event → VibrateCmd`, capability-based fallback
- **Cache logic:** Insert/lookup/evict device mappings
- **Backoff calculation:** Verify exponential backoff formula and max cap

### Integration Tests
- **Mock server:** Use `tokio-tungstenite` to spin up a test WebSocket server
- **Connection happy path:** Server running → connect → scan devices → dispatch gesture → verify command sent
- **Reconnection:** Server stops → client backsoff → server restarts → client reconnects → rescan
- **Device not found:** Dispatch gesture to non-existent device → verify logged, no panic
- **Backend factory:** Verify `is_known("buttplug")` returns true, `create("buttplug")` succeeds, unknown backends still error

### Existing Tests
- Extend `backend::tests` to include buttplug in factory tests
- Ensure all existing config validation and backend tests pass

---

## Implementation Phases

1. **Phase 1:** Core connection manager
   - WebSocket connection, backoff logic, lifecycle management
   - Mock tests for connection behavior

2. **Phase 2:** Device scanning and cache
   - Buttplug protocol for device list queries
   - Integration tests with mock server

3. **Phase 3:** Gesture translation and dispatch
   - Event → buttplug command mapping
   - Handle device capabilities

4. **Phase 4:** Config integration and polish
   - YAML schema for buttplug config
   - Error logging and observability
   - Startup integration with main.rs

---

## Dependencies

- `tokio-tungstenite` (WebSocket client)
- `serde_json` (already in Cargo.toml; used for buttplug protocol messages)
- `tokio` (already in Cargo.toml; features=["full"])

---

## Non-Goals

- Intiface Core embedding (connect to external server only)
- Device pairing/permissions (assume pre-paired in Intiface)
- Custom gesture creation (use existing `gestures.rs` models)
- Error return propagation (fire-and-forget, log only)
- Web dashboard or monitoring (logging only)

---

## Success Criteria

1. ✓ Bridge connects to Intiface on startup (or backsoff gracefully if unavailable)
2. ✓ Gestures dispatched via buttplug backend reach connected devices
3. ✓ Device lookup works by index (e.g., `buttplug/0`) and name (e.g., `buttplug/Lovense Nora`)
4. ✓ Connection recovers automatically from server restart
5. ✓ All errors logged; no panics or hanging threads
6. ✓ Existing tests pass; new integration tests cover core flows
7. ✓ Config validation rejects invalid backend names early (existing behavior preserved)
