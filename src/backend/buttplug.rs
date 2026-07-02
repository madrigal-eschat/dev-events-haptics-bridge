use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Result, bail};
use tokio::sync::{mpsc, oneshot};

use crate::backend::{Backend, DeviceList};
use crate::config::ButtplugConfig;
use crate::gestures::Event;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeviceLookup {
    ByIndex(u32),
    ByName(String),
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
enum ActuatorKind {
    Vibrate,
    Linear,
    Rotate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Actuator {
    index: u32,
    kind: ActuatorKind,
}

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

#[derive(Debug, Clone)]
struct GestureJob {
    lookup: DeviceLookup,
    actuator_index: Option<u32>,
    event: Event,
}

#[derive(Debug, Clone, Default)]
struct DeviceInfo {
    actuators: Vec<Actuator>,
}

type DeviceCache = HashMap<DeviceLookup, Arc<DeviceInfo>>;

pub struct ButtplugBackend {
    pub config: ButtplugConfig,
    device_cache: Arc<Mutex<DeviceCache>>,
    tx: Mutex<Option<mpsc::Sender<GestureJob>>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    processed_jobs: Arc<AtomicUsize>,
}

impl ButtplugBackend {
    pub fn new(config: ButtplugConfig) -> Self {
        Self {
            config,
            device_cache: Arc::new(Mutex::new(HashMap::new())),
            tx: Mutex::new(None),
            shutdown_tx: Mutex::new(None),
            processed_jobs: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn startup_with_runtime<F>(&self, build_runtime: F) -> Result<()>
    where
        F: FnOnce() -> Result<tokio::runtime::Runtime>,
    {
        if self.tx.lock().unwrap().is_some() {
            return Ok(());
        }

        let runtime = build_runtime()?;
        let (tx, rx) = mpsc::channel(64);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let device_cache = Arc::clone(&self.device_cache);
        let processed_jobs = Arc::clone(&self.processed_jobs);
        std::thread::Builder::new()
            .name("buttplug-backend".to_string())
            .spawn(move || {
                runtime.block_on(run_worker(rx, shutdown_rx, device_cache, processed_jobs));
            })?;

        *self.tx.lock().unwrap() = Some(tx);
        *self.shutdown_tx.lock().unwrap() = Some(shutdown_tx);

        Ok(())
    }

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
}

impl Backend for ButtplugBackend {
    fn startup(&self) -> Result<()> {
        self.startup_with_runtime(|| {
            Ok(
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?,
            )
        })
    }

    fn teardown(&self) -> Result<()> {
        self.tx.lock().unwrap().take();
        if let Some(shutdown_tx) = self.shutdown_tx.lock().unwrap().take() {
            let _ = shutdown_tx.send(());
        }
        Ok(())
    }

    fn validate_device(&self, device_id: &str) -> Result<()> {
        parse_device_id(device_id).map(|_| ())
    }

    fn list_devices(&self) -> Result<DeviceList> {
        Ok(DeviceList::Anything)
    }

    fn resolve_device_ids(&self, device_ids: &[String]) -> Result<Vec<String>> {
        let parsed: Vec<(String, DeviceLookup, Option<u32>)> = device_ids
            .iter()
            .map(|device_id| {
                parse_device_id(device_id)
                    .map(|(lookup, actuator)| (device_id.clone(), lookup, actuator))
            })
            .collect::<Result<_>>()?;

        let cache = self.device_cache.lock().unwrap();
        let mut selected: HashMap<DeviceLookup, HashSet<Actuator>> = HashMap::new();

        for (_, lookup, actuator) in &parsed {
            if let Some(actuator) = actuator
                && let Some(device) = cache.get(lookup)
            {
                let reservations = selected.entry(lookup.clone()).or_default();
                for device_actuator in device
                    .actuators
                    .iter()
                    .copied()
                    .filter(|device_actuator| device_actuator.index == *actuator)
                {
                    reservations.insert(device_actuator);
                }
            }
        }

        let mut resolved = Vec::with_capacity(parsed.len());
        for (raw, lookup, actuator) in parsed {
            let Some(device) = cache.get(&lookup) else {
                resolved.push(raw);
                continue;
            };

            if device.actuators.is_empty() {
                resolved.push(raw);
                continue;
            }

            match actuator {
                Some(actuator) => {
                    if !device
                        .actuators
                        .iter()
                        .any(|device_actuator| device_actuator.index == actuator)
                    {
                        resolved.push(raw);
                        continue;
                    }
                    resolved.push(format_device_id(&lookup, Some(actuator)));
                }
                None => {
                    let chosen = choose_actuator(
                        &device.actuators,
                        selected.entry(lookup.clone()).or_default(),
                    );
                    debug_assert!(
                        chosen.is_some(),
                        "choose_actuator returned None for non-empty device actuators in device id '{raw}'"
                    );
                    let chosen = chosen.unwrap_or_else(|| device.actuators[0]);
                    selected.get_mut(&lookup).unwrap().insert(chosen);
                    resolved.push(format_device_id(&lookup, Some(chosen.index)));
                }
            }
        }

        Ok(resolved)
    }

    fn send_event(&self, device_id: String, event: &Event) {
        let (lookup, actuator_index) = match parse_device_id(&device_id) {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("buttplug backend: {err}");
                return;
            }
        };

        let job = GestureJob {
            lookup,
            actuator_index,
            event: *event,
        };

        let Some(sender) = self.tx.lock().unwrap().as_ref().cloned() else {
            eprintln!("buttplug backend: dropped event because backend is not started");
            return;
        };

        if let Err(err) = sender.try_send(job) {
            eprintln!("buttplug backend: failed to queue event for '{device_id}': {err}");
        }
    }
}

async fn run_worker(
    mut rx: mpsc::Receiver<GestureJob>,
    mut shutdown_rx: oneshot::Receiver<()>,
    device_cache: Arc<Mutex<DeviceCache>>,
    processed_jobs: Arc<AtomicUsize>,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                eprintln!("buttplug backend: shutdown requested");
                break;
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

fn handle_job(device_cache: &Arc<Mutex<DeviceCache>>, job: GestureJob) {
    let cache = device_cache.lock().unwrap();
    let Some(device) = cache.get(&job.lookup) else {
        eprintln!("buttplug backend: unknown device lookup {:?}", job.lookup);
        return;
    };

    let selected = HashSet::new();
    let actuator = match job.actuator_index {
        Some(index) => device
            .actuators
            .iter()
            .copied()
            .find(|actuator| actuator.index == index),
        None => choose_actuator(&device.actuators, &selected),
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

    eprintln!(
        "buttplug backend: dispatch {:?} actuator {} as {:?}",
        job.lookup, actuator.index, command
    );
}

fn translate_event(kind: ActuatorKind, event: &Event) -> Option<ButtplugCommand> {
    let magnitude = event.magnitude.clamp(0.0, 1.0);
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
            clockwise: true,
            duration_ms: event.duration_ms,
        },
    })
}

fn parse_device_id(device_id: &str) -> Result<(DeviceLookup, Option<u32>)> {
    let (lookup, actuator) = match device_id.rsplit_once('/') {
        Some((lookup, actuator)) => {
            let actuator = actuator.parse::<u32>().map_err(|_| {
                anyhow::anyhow!("invalid actuator index in device id '{device_id}'")
            })?;
            (lookup, Some(actuator))
        }
        None => (device_id, None),
    };

    if lookup.contains('/') {
        bail!("invalid device id '{device_id}'");
    }

    let lookup = percent_decode(lookup, device_id)?;
    if lookup.is_empty() {
        bail!("empty device lookup in device id '{device_id}'");
    }
    let lookup = match lookup.parse::<u32>() {
        Ok(index) => DeviceLookup::ByIndex(index),
        Err(_) => DeviceLookup::ByName(lookup),
    };

    Ok((lookup, actuator))
}

fn format_device_id(lookup: &DeviceLookup, actuator: Option<u32>) -> String {
    let lookup = match lookup {
        DeviceLookup::ByIndex(index) => index.to_string(),
        DeviceLookup::ByName(name) => percent_encode(name),
    };
    match actuator {
        Some(actuator) => format!("{lookup}/{actuator}"),
        None => lookup,
    }
}

fn choose_actuator(actuators: &[Actuator], selected: &HashSet<Actuator>) -> Option<Actuator> {
    let mut actuators = actuators.to_vec();
    actuators.sort_by_key(|actuator| (actuator.kind, actuator.index));

    if let Some(actuator) = actuators
        .iter()
        .find(|actuator| !selected.contains(actuator))
    {
        return Some(*actuator);
    }

    actuators.first().copied()
}

fn percent_decode(input: &str, device_id: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hi = *bytes.get(index + 1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid percent-encoding in device lookup '{input}' (device id '{device_id}')"
                    )
                })?;
                let lo = *bytes.get(index + 2).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid percent-encoding in device lookup '{input}' (device id '{device_id}')"
                    )
                })?;
                let hi = hex_value(hi, input, device_id)?;
                let lo = hex_value(lo, input, device_id)?;
                decoded.push((hi << 4) | lo);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    Ok(String::from_utf8(decoded)?)
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        if is_unreserved(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

fn is_unreserved(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
    )
}

fn hex_value(byte: u8, input: &str, device_id: &str) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!(
            "invalid hex digit '{}' in device lookup '{input}' (device id '{device_id}')",
            byte as char
        ),
    }
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    #[test]
    fn startup_and_teardown_are_safe_before_connection() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        assert!(backend.startup().is_ok());
        assert!(backend.teardown().is_ok());
    }

    #[test]
    fn list_devices_returns_anything_until_device_discovery_exists() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        assert!(matches!(
            backend.list_devices().unwrap(),
            DeviceList::Anything
        ));
    }

    #[test]
    fn validate_device_accepts_valid_buttplug_ids() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        assert!(backend.validate_device("Lovense%20Nora/2").is_ok());
        assert!(backend.validate_device("42").is_ok());
    }

    #[test]
    fn validate_device_rejects_invalid_buttplug_ids() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        assert!(backend.validate_device("broken%2").is_err());
        assert!(backend.validate_device("7/not-a-number").is_err());
        assert!(backend.validate_device("foo/bar/1").is_err());
    }

    #[test]
    fn validate_device_rejects_empty_lookup() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        assert!(backend.validate_device("").is_err());
        assert!(backend.validate_device("/2").is_err());
    }

    #[test]
    fn parse_device_id_decodes_lookup_and_actuator_index() {
        let (lookup, actuator) = parse_device_id("Lovense%20Nora/2").unwrap();
        assert_eq!(lookup, DeviceLookup::ByName("Lovense Nora".into()));
        assert_eq!(actuator, Some(2));
    }

    #[test]
    fn parse_device_id_round_trips_percent_encoded_slash_in_lookup() {
        let (lookup, actuator) = parse_device_id("Lovense%2FNora/1").unwrap();
        assert_eq!(lookup, DeviceLookup::ByName("Lovense/Nora".into()));
        assert_eq!(actuator, Some(1));
        assert_eq!(format_device_id(&lookup, actuator), "Lovense%2FNora/1");
    }

    #[test]
    fn parse_device_id_classifies_numeric_lookups_as_indices() {
        let (lookup, actuator) = parse_device_id("42").unwrap();
        assert_eq!(lookup, DeviceLookup::ByIndex(42));
        assert_eq!(actuator, None);
    }

    #[test]
    fn parse_device_id_rejects_bad_percent_encoding() {
        let err = parse_device_id("broken%2").unwrap_err().to_string();
        assert!(err.contains("device lookup 'broken%2'"));
        assert!(err.contains("device id 'broken%2'"));
    }

    #[test]
    fn parse_device_id_rejects_bad_percent_encoding_with_invalid_hex_digit() {
        let err = parse_device_id("broken%2Z").unwrap_err().to_string();
        assert!(err.contains("device lookup 'broken%2Z'"));
        assert!(err.contains("invalid hex digit 'Z'"));
    }

    #[test]
    fn resolve_device_ids_prefers_unused_actuators_by_priority() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(
            DeviceLookup::ByIndex(7),
            vec![
                ActuatorKind::Rotate,
                ActuatorKind::Linear,
                ActuatorKind::Vibrate,
            ],
        );

        let ids = vec!["7".to_string(), "7/1".to_string(), "7".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(
            resolved,
            vec!["7/2".to_string(), "7/1".to_string(), "7/0".to_string()]
        );
    }

    #[test]
    fn resolve_device_ids_falls_back_to_priority_when_all_are_taken() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(
            DeviceLookup::ByName("Lovense Nora".into()),
            vec![ActuatorKind::Vibrate, ActuatorKind::Linear],
        );

        let ids = vec![
            "Lovense%20Nora/1".to_string(),
            "Lovense%20Nora/0".to_string(),
            "Lovense%20Nora".to_string(),
        ];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(
            resolved,
            vec![
                "Lovense%20Nora/1".to_string(),
                "Lovense%20Nora/0".to_string(),
                "Lovense%20Nora/0".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_device_ids_leaves_unknown_and_zero_actuator_devices_unchanged() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(DeviceLookup::ByIndex(3), vec![]);

        let ids = vec!["Missing".to_string(), "3".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(resolved, ids);
    }

    #[test]
    fn resolve_device_ids_leaves_unknown_explicit_actuator_unresolved() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(
            DeviceLookup::ByIndex(7),
            vec![ActuatorKind::Vibrate, ActuatorKind::Linear],
        );

        let ids = vec!["7/9".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(resolved, ids);
    }

    #[test]
    fn resolve_device_ids_leaves_explicit_actuator_on_zero_actuator_device_unresolved() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(DeviceLookup::ByIndex(3), vec![]);

        let ids = vec!["3/0".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(resolved, ids);
    }

    #[test]
    fn resolve_device_ids_keeps_selection_scoped_to_each_lookup() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device_aliases(
            vec![
                DeviceLookup::ByIndex(7),
                DeviceLookup::ByName("Lovense Nora".into()),
            ],
            vec![ActuatorKind::Vibrate, ActuatorKind::Linear],
        );

        let ids = vec!["7".to_string(), "Lovense%20Nora".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(
            resolved,
            vec!["7/0".to_string(), "Lovense%20Nora/0".to_string()]
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
            Some(ButtplugCommand::Vibrate {
                magnitude: 0.50,
                frequency: 0.50,
            })
        );
    }

    #[test]
    fn startup_processes_queued_events_and_teardown_stops_processing() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(DeviceLookup::ByIndex(0), vec![ActuatorKind::Vibrate]);

        backend.startup().unwrap();
        assert_eq!(backend.processed_jobs.load(Ordering::SeqCst), 0);

        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));

        for _ in 0..100 {
            if backend.processed_jobs.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(backend.processed_jobs.load(Ordering::SeqCst), 1);

        backend.teardown().unwrap();
        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(backend.processed_jobs.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn startup_failure_does_not_leave_backend_half_initialized() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(DeviceLookup::ByIndex(0), vec![ActuatorKind::Vibrate]);

        let err = backend
            .startup_with_runtime(|| Err(anyhow::anyhow!("runtime build failed")))
            .unwrap_err();
        assert!(err.to_string().contains("runtime build failed"));
        assert!(backend.tx.lock().unwrap().is_none());
        assert!(backend.shutdown_tx.lock().unwrap().is_none());

        backend.startup().unwrap();
        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));

        for _ in 0..100 {
            if backend.processed_jobs.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(backend.processed_jobs.load(Ordering::SeqCst), 1);
        backend.teardown().unwrap();
    }
}
