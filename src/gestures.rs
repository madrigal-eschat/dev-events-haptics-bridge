/// A single haptic event: apply magnitude to device for duration_ms milliseconds.
///
/// Same-device events in a gesture fire sequentially.
/// Different-device events fire in parallel (independent channels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub duration_ms: u32,
    pub magnitude: f32,
    pub device: u8,
}

impl Event {
    pub const fn new(duration_ms: u32, magnitude: f32, device: u8) -> Self {
        Self { duration_ms, magnitude, device }
    }
}

// ── Single device ─────────────────────────────────────────────────────────────

/// One 50 ms burst.
pub const PULSE_SHORT: &[Event] = &[
    Event::new(50,  1.0, 0),
];

/// One 300 ms burst.
pub const PULSE_MEDIUM: &[Event] = &[
    Event::new(300, 1.0, 0),
];

/// One 1000 ms burst.
pub const PULSE_LONG: &[Event] = &[
    Event::new(1000, 1.0, 0),
];

/// Two 50 ms pulses, 100 ms gap between.
pub const DOUBLE_SHORT: &[Event] = &[
    Event::new(50,  1.0, 0),
    Event::new(100, 0.0, 0),
    Event::new(50,  1.0, 0),
];

/// Three 50 ms pulses, 100 ms gaps between.
pub const TRIPLE_SHORT: &[Event] = &[
    Event::new(50,  1.0, 0),
    Event::new(100, 0.0, 0),
    Event::new(50,  1.0, 0),
    Event::new(100, 0.0, 0),
    Event::new(50,  1.0, 0),
];

/// Three 100 ms pulses, 200 ms gaps between — slower cadence.
pub const TRIPLE_SLOW: &[Event] = &[
    Event::new(100, 1.0, 0),
    Event::new(200, 0.0, 0),
    Event::new(100, 1.0, 0),
    Event::new(200, 0.0, 0),
    Event::new(100, 1.0, 0),
];

// ── Two devices ───────────────────────────────────────────────────────────────

/// One 300 ms burst on both devices simultaneously.
pub const BOTH_MEDIUM: &[Event] = &[
    Event::new(300,  1.0, 0),
    Event::new(300,  1.0, 1),
];

/// One 1000 ms burst on both devices simultaneously.
pub const BOTH_LONG: &[Event] = &[
    Event::new(1000, 1.0, 0),
    Event::new(1000, 1.0, 1),
];

/// 300 ms crossfade: device 0 fades out while device 1 fades in (constant total power).
/// 6 steps × 50 ms.
pub const CROSSFADE_MEDIUM: &[Event] = &[
    Event::new(50, 1.00, 0), Event::new(50, 0.00, 1),
    Event::new(50, 0.80, 0), Event::new(50, 0.20, 1),
    Event::new(50, 0.60, 0), Event::new(50, 0.40, 1),
    Event::new(50, 0.40, 0), Event::new(50, 0.60, 1),
    Event::new(50, 0.20, 0), Event::new(50, 0.80, 1),
    Event::new(50, 0.00, 0), Event::new(50, 1.00, 1),
];

/// 1000 ms crossfade: device 0 fades out while device 1 fades in (constant total power).
/// 10 steps × 100 ms.
pub const CROSSFADE_LONG: &[Event] = &[
    Event::new(100, 1.00, 0), Event::new(100, 0.00, 1),
    Event::new(100, 0.89, 0), Event::new(100, 0.11, 1),
    Event::new(100, 0.78, 0), Event::new(100, 0.22, 1),
    Event::new(100, 0.67, 0), Event::new(100, 0.33, 1),
    Event::new(100, 0.56, 0), Event::new(100, 0.44, 1),
    Event::new(100, 0.44, 0), Event::new(100, 0.56, 1),
    Event::new(100, 0.33, 0), Event::new(100, 0.67, 1),
    Event::new(100, 0.22, 0), Event::new(100, 0.78, 1),
    Event::new(100, 0.11, 0), Event::new(100, 0.89, 1),
    Event::new(100, 0.00, 0), Event::new(100, 1.00, 1),
];

/// Two 50 ms pulses on both devices simultaneously, 100 ms gap between.
pub const BOTH_DOUBLE_SHORT: &[Event] = &[
    Event::new(50,  1.0, 0), Event::new(50,  1.0, 1),
    Event::new(100, 0.0, 0), Event::new(100, 0.0, 1),
    Event::new(50,  1.0, 0), Event::new(50,  1.0, 1),
];

/// Two 50 ms pulses alternating: first device 0, then device 1.
/// Device 1 waits 150 ms (pulse + gap) before firing.
pub const ALTERNATE_DOUBLE_SHORT: &[Event] = &[
    Event::new(50,  1.0, 0),
    Event::new(150, 0.0, 1),
    Event::new(50,  1.0, 1),
];
