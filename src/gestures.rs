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

// ── Lookup ────────────────────────────────────────────────────────────────────

/// Returns the named gesture, or `None` if unknown.
pub fn lookup(name: &str) -> Option<&'static [Event]> {
    match name {
        "pulse_short"          => Some(PULSE_SHORT),
        "pulse_medium"         => Some(PULSE_MEDIUM),
        "pulse_long"           => Some(PULSE_LONG),
        "double_short"         => Some(DOUBLE_SHORT),
        "triple_short"         => Some(TRIPLE_SHORT),
        "triple_slow"          => Some(TRIPLE_SLOW),
        "both_medium"          => Some(BOTH_MEDIUM),
        "both_long"            => Some(BOTH_LONG),
        "crossfade_medium"     => Some(CROSSFADE_MEDIUM),
        "crossfade_long"       => Some(CROSSFADE_LONG),
        "both_double_short"    => Some(BOTH_DOUBLE_SHORT),
        "alternate_double_short" => Some(ALTERNATE_DOUBLE_SHORT),
        _ => None,
    }
}

// ── Modification ──────────────────────────────────────────────────────────────

/// Returns a modified copy of a gesture.
///
/// `speed`: `> 1.0` faster, `< 1.0` slower; must be positive and non-zero.
/// `magnitude_scale`: multiplied per event, clamped to `[0.0, 1.0]`.
pub fn scale(events: &[Event], speed: f32, magnitude_scale: f32) -> Vec<Event> {
    assert!(speed > 0.0, "speed must be positive and non-zero");
    events
        .iter()
        .map(|&e| Event {
            duration_ms: ((e.duration_ms as f32) / speed).round() as u32,
            magnitude: (e.magnitude * magnitude_scale).clamp(0.0, 1.0),
            device: e.device,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── lookup ─────────────────────────────────────────────────────────────

    #[test]
    fn lookup_all_named_gestures() {
        let names = [
            "pulse_short", "pulse_medium", "pulse_long",
            "double_short", "triple_short", "triple_slow",
            "both_medium", "both_long",
            "crossfade_medium", "crossfade_long",
            "both_double_short", "alternate_double_short",
        ];
        for name in names {
            assert!(lookup(name).is_some(), "lookup({name}) returned None");
        }
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("nonexistent").is_none());
        assert!(lookup("PULSE_SHORT").is_none()); // case-sensitive
        assert!(lookup("").is_none());
    }

    // ── scale ──────────────────────────────────────────────────────────────

    #[test]
    fn scale_identity() {
        let input = &[Event::new(100, 0.5, 0)];
        let out = scale(input, 1.0, 1.0);
        assert_eq!(out, vec![Event::new(100, 0.5, 0)]);
    }

    #[test]
    fn scale_empty_input() {
        assert_eq!(scale(&[], 2.0, 0.5), vec![]);
    }

    #[test]
    fn scale_speed_halves_duration() {
        let out = scale(&[Event::new(100, 1.0, 0)], 2.0, 1.0);
        assert_eq!(out[0].duration_ms, 50);
    }

    #[test]
    fn scale_speed_doubles_duration() {
        let out = scale(&[Event::new(100, 1.0, 0)], 0.5, 1.0);
        assert_eq!(out[0].duration_ms, 200);
    }

    #[test]
    fn scale_magnitude_factor() {
        let out = scale(&[Event::new(100, 1.0, 0)], 1.0, 0.5);
        assert!((out[0].magnitude - 0.5).abs() < 1e-6);
    }

    #[test]
    fn scale_magnitude_clamped_above_one() {
        let out = scale(&[Event::new(100, 0.8, 0)], 1.0, 2.0);
        assert_eq!(out[0].magnitude, 1.0);
    }

    #[test]
    fn scale_magnitude_clamped_below_zero() {
        let out = scale(&[Event::new(100, 0.5, 0)], 1.0, -1.0);
        assert_eq!(out[0].magnitude, 0.0);
    }

    #[test]
    fn scale_preserves_device() {
        let out = scale(&[Event::new(50, 1.0, 7)], 1.0, 1.0);
        assert_eq!(out[0].device, 7);
    }

    #[test]
    #[should_panic(expected = "speed must be positive and non-zero")]
    fn scale_panics_on_zero_speed() {
        scale(&[Event::new(100, 1.0, 0)], 0.0, 1.0);
    }

    #[test]
    #[should_panic(expected = "speed must be positive and non-zero")]
    fn scale_panics_on_negative_speed() {
        scale(&[Event::new(100, 1.0, 0)], -1.0, 1.0);
    }
}
