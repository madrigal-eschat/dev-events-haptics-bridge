# haptics-bridge

MQTT-driven haptics dispatcher. Subscribes to a broker, matches incoming
CloudEvents against rules from a YAML config, and plays haptic gestures on
one or more hardware backends.

**Data flow**: MQTT message → deserialize `CloudEvent` → match against
`Rule[]` → look up gesture → scale timing/magnitude → send to backend.

## Building

```bash
cargo build --release
```

Binary is output at `target/release/haptics`.

## Running

```bash
haptics <config.yaml>
```

`RUST_LOG` controls log verbosity (via `env_logger`), e.g. `RUST_LOG=debug haptics config.yaml`.

## Config

```yaml
broker:
  host: localhost
  port: 1883          # optional, default 1883
  client_id: my-bridge  # optional, default "haptics-bridge"
  auth:                # optional
    username: user
    password: pass

topics:
  - devevents/#

rules:
  - filter:
      type: devevents.task.failed   # glob, matches CloudEvent `type`
      source: editor/*              # glob, matches CloudEvent `source`
      sourcetype: editor            # glob, matches CloudEvent `sourcetype`
      subject: "~/projects/*"       # glob, matches CloudEvent `subject`
      data:                         # exact match against CloudEvent `data` fields
        exit_code: 1
    gesture:
      name: pulse_short
      speed: 1.0    # optional, default 1.0; stretches/compresses timing
      scale: 1.0    # optional, default 1.0; scales magnitude, clamped to [0, 1]
    device: stdout/0   # or `devices: [stdout/0, stdout/1]` for multi-device gestures

http:                 # optional; enables the HTTP backend
  bind: "127.0.0.1:8080"   # optional, default shown

buttplug:              # optional; enables the buttplug (Intiface) backend
  server: "ws://localhost:12345"   # optional, default shown
  connection_timeout_ms: 5000      # optional, default shown, must be > 0
  max_backoff_ms: 30000            # optional, default shown
  scan_interval_ms: 30000          # optional, default shown, must be > 0
```

All filter fields in a rule are optional and ANDed together; an omitted
field matches anything. Filter patterns support `*` as a wildcard that can
match across `/`. A rule's device count must match the number of device
slots the chosen gesture uses.

### Device addressing

A device is addressed as `BACKEND/ID`, e.g. `stdout/0` or
`buttplug/Lovense Edge/0`. The backend name is the part before the first
`/`; the remainder is passed through to that backend.

### Backends

- `stdout` — prints events to stdout; no config needed.
- `http` — exposes an HTTP endpoint for external consumers; configured via the `http:` section.
- `buttplug` — connects to an Intiface/buttplug server over WebSocket; configured via the `buttplug:` section.

### Gestures

Built-in gesture names: `pulse_short`, `pulse_medium`, `pulse_long`,
`double_short`, `triple_short`, `triple_slow`, `both_medium`, `both_long`,
`crossfade_medium`, `crossfade_long`, `both_double_short`,
`alternate_double_short`, `stop`, `stop_all`.

Gestures using more than one device index (e.g. `both_medium`, `stop_all`)
require a matching number of devices in the rule via `devices: [...]`.

## Development

```bash
cargo check                                 # type-check
cargo fmt                                   # format
cargo fmt --check                           # format check (CI gate)
cargo clippy --all-targets -- -D warnings   # lint
cargo test                                  # run tests
cargo test <name>                           # run a single test by name substring
```

A pre-commit hook enforces `cargo fmt`, `cargo check`, `cargo clippy`, and
`cargo test`. CI runs `cargo fmt --check`, `cargo check`, and `cargo test`.

Commit messages must follow [Conventional Commits](https://www.conventionalcommits.org/)
— enforced by commitlint on PRs. Releases are automated via semantic-release
on push to `main`, publishing a `haptics` binary as a GitHub release asset.

## License

See [LICENSE](LICENSE).
