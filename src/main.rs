pub mod backend;
pub mod config;
pub mod event;
pub mod gestures;
pub mod player;

use std::collections::HashMap;

use anyhow::{Context, Result};
use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS};

use config::Config;
use gestures::{lookup, scale};

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .context("usage: haptics <config.yaml>")?;
    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {config_path}"))?;
    let config: Config = serde_yaml::from_str(&config_str).context("failed to parse config")?;

    config.validate()?;

    // Build one backend instance per unique backend name.
    let mut backends: HashMap<String, Box<dyn backend::Backend>> = HashMap::new();
    for rule in &config.rules {
        for addr in rule.device_spec.as_slice() {
            let backend_name = addr.split_once('/').expect("validated").0;
            if !backends.contains_key(backend_name) {
                backends.insert(
                    backend_name.to_string(),
                    backend::create(backend_name, &config)?,
                );
            }
        }
    }

    for rule in &config.rules {
        for addr in rule.device_spec.as_slice() {
            let (backend_name, device_id) = addr.split_once('/').expect("validated");
            if let Err(e) = backends[backend_name].validate_device(device_id) {
                eprintln!("{e}");
                return Err(e);
            }
        }
    }

    for backend in backends.values() {
        backend.startup()?;
    }

    let client_id = config
        .broker
        .client_id
        .as_deref()
        .unwrap_or("haptics-bridge");
    let mut opts = MqttOptions::new(client_id, &config.broker.host, config.broker.port);
    if let Some(auth) = &config.broker.auth {
        opts.set_credentials(&auth.username, &auth.password);
    }

    let (client, mut eventloop) = AsyncClient::new(opts, 64);
    for topic in &config.topics {
        client.subscribe(topic, QoS::AtMostOnce).await?;
    }

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
                    dispatch_rule(&backends, rule.device_spec.as_slice(), &events)?;
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

fn dispatch_rule(
    backends: &HashMap<String, Box<dyn backend::Backend>>,
    devices: &[String],
    events: &[gestures::Event],
) -> Result<()> {
    let backend_name = devices
        .first()
        .and_then(|addr| addr.split_once('/'))
        .expect("validated at startup")
        .0;
    let resolved_devices = backends[backend_name].resolve_device_ids(devices)?;
    for haptic_event in events {
        let device_id = &resolved_devices[haptic_event.device as usize];
        backends[backend_name].send_event(device_id.to_string(), haptic_event);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct RecordingBackend {
        resolve_mode: ResolveMode,
        resolve_calls: Arc<Mutex<Vec<Vec<String>>>>,
        sent_device_ids: Arc<Mutex<Vec<String>>>,
    }

    #[derive(Clone, Copy, Default)]
    enum ResolveMode {
        #[default]
        PreservePrefix,
        StripPrefix,
    }

    impl backend::Backend for RecordingBackend {
        fn startup(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn teardown(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn validate_device(&self, _device_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> anyhow::Result<backend::DeviceList> {
            Ok(backend::DeviceList::Anything)
        }

        fn resolve_device_ids(&self, device_ids: &[String]) -> anyhow::Result<Vec<String>> {
            self.resolve_calls.lock().unwrap().push(device_ids.to_vec());
            Ok(match self.resolve_mode {
                ResolveMode::PreservePrefix => device_ids.to_vec(),
                ResolveMode::StripPrefix => device_ids
                    .iter()
                    .map(|device_id| {
                        device_id
                            .split_once('/')
                            .map(|(_, id)| id.to_string())
                            .unwrap_or_else(|| device_id.clone())
                    })
                    .collect(),
            })
        }

        fn send_event(&self, device_id: String, _event: &gestures::Event) {
            self.sent_device_ids.lock().unwrap().push(device_id);
        }
    }

    #[test]
    fn rule_device_ids_with_backend_prefix_are_preserved_end_to_end() {
        let backend = RecordingBackend::default();
        let handle = backend.clone();

        let mut backends: HashMap<String, Box<dyn backend::Backend>> = HashMap::new();
        backends.insert("stdout".to_string(), Box::new(backend));

        let devices = vec!["stdout/0".to_string(), "stdout/1".to_string()];
        let events = vec![
            gestures::Event::new(50, 1.0, 0),
            gestures::Event::new(50, 1.0, 1),
        ];

        dispatch_rule(&backends, &devices, &events).unwrap();

        assert_eq!(handle.resolve_calls.lock().unwrap().as_slice(), &[devices]);
        assert_eq!(
            handle.sent_device_ids.lock().unwrap().as_slice(),
            &["stdout/0".to_string(), "stdout/1".to_string()]
        );
    }

    #[test]
    fn rule_device_ids_without_backend_prefix_still_use_original_backend() {
        let backend = RecordingBackend {
            resolve_mode: ResolveMode::StripPrefix,
            ..Default::default()
        };
        let handle = backend.clone();

        let mut backends: HashMap<String, Box<dyn backend::Backend>> = HashMap::new();
        backends.insert("buttplug".to_string(), Box::new(backend));

        let devices = vec!["buttplug/0".to_string(), "buttplug/1".to_string()];
        let events = vec![
            gestures::Event::new(50, 1.0, 0),
            gestures::Event::new(50, 1.0, 1),
        ];

        dispatch_rule(&backends, &devices, &events).unwrap();

        assert_eq!(handle.resolve_calls.lock().unwrap().as_slice(), &[devices]);
        assert_eq!(
            handle.sent_device_ids.lock().unwrap().as_slice(),
            &["0".to_string(), "1".to_string()]
        );
    }
}
