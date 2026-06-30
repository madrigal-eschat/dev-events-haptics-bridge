# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo check          # type-check without building
cargo fmt            # format (run before committing)
cargo fmt --check    # check formatting (CI gate)
cargo clippy --all-targets -- -D warnings   # lint (pre-commit gate)
cargo test           # run all tests
cargo test <name>    # run a single test by name substring
cargo build          # build binary (output: target/debug/haptics)
```

CI runs `cargo fmt --check`, `cargo check`, and `cargo test` — all three must pass before committing. A pre-commit hook enforces `cargo fmt`, `cargo check`, `cargo clippy`, and `cargo test` automatically.

## Architecture

The binary subscribes to an MQTT broker, matches incoming CloudEvents against rules from a YAML config file, and dispatches haptic gestures to hardware backends.

**Data flow**: MQTT message → deserialize `CloudEvent` → match against `Rule[]` → look up gesture → `gestures::scale()` → `Backend::send_event()`

### Key modules

- **`config.rs`** — YAML schema (`Config`, `BrokerConfig`, `Rule`, `Filter`, `GestureConfig`) plus `Config::validate()` (checks backend names, gesture names, and numeric ranges at startup) and `Filter::matches()` (glob-based CloudEvent matching)
- **`gestures.rs`** — static gesture definitions (`&[Event]` slices) and `scale(speed, magnitude)` which stretches/compresses timing and scales magnitude
- **`backend.rs`** — `Backend` trait; currently only `stdout` backend exists; `is_known()` and `create()` are the extension points for new backends
- **`player.rs`** — thread-safe haptic sequencer (`Player`) with a condvar-based worker thread; supports `queue`, `interrupt` (prepend + abort current), and `clear` (stop + emit zero-magnitude); **not yet wired into main**
- **`event.rs`** — `CloudEvent` deserialization struct (CloudEvents spec subset, `sourcetype` is a project-specific extension field)
- **`main.rs`** — startup, MQTT loop, rule dispatch; one backend instance per unique backend name in the rule list

### Device addressing

`device` field in a rule is `BACKEND/ID` (e.g. `stdout/0`). The backend name is the part before `/`; the ID is passed through to `Backend::send_event`.

### Gestures

`gestures::Event` has `(duration_ms, magnitude, device: u8)`. Events with the same `device` index fire sequentially; different device indices fire in parallel. `scale()` divides durations by speed and multiplies magnitudes (clamped to `[0.0, 1.0]`).

### Releases

CI uses semantic-release on push to `main`. Commit messages must follow Conventional Commits — enforced by commitlint on PRs. The release workflow publishes a `haptics` binary as a GitHub release asset.
