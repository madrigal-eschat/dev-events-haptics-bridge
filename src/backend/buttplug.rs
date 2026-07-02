use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

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

#[derive(Debug, Clone, Default)]
struct DeviceInfo {
    actuators: Vec<Actuator>,
}

type DeviceCache = HashMap<DeviceLookup, Arc<DeviceInfo>>;

pub struct ButtplugBackend {
    pub config: ButtplugConfig,
    device_cache: Mutex<DeviceCache>,
}

impl ButtplugBackend {
    pub fn new(config: ButtplugConfig) -> Self {
        Self {
            config,
            device_cache: Mutex::new(HashMap::new()),
        }
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
        Ok(())
    }

    fn teardown(&self) -> Result<()> {
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
                    let Some(chosen) = chosen else {
                        resolved.push(raw);
                        continue;
                    };
                    selected.get_mut(&lookup).unwrap().insert(chosen);
                    resolved.push(format_device_id(&lookup, Some(chosen.index)));
                }
            }
        }

        Ok(resolved)
    }

    fn send_event(&self, _device_id: String, _event: &Event) {}
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

    let lookup = percent_decode(lookup)?;
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

fn percent_decode(input: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hi = *bytes.get(index + 1).ok_or_else(|| {
                    anyhow::anyhow!("invalid percent-encoding in device id '{input}'")
                })?;
                let lo = *bytes.get(index + 2).ok_or_else(|| {
                    anyhow::anyhow!("invalid percent-encoding in device id '{input}'")
                })?;
                let hi = hex_value(hi)?;
                let lo = hex_value(lo)?;
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

fn hex_value(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("invalid percent-encoding"),
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
        assert!(parse_device_id("broken%2").is_err());
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
}
