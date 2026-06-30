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
                backends.insert(backend_name.to_string(), backend::create(backend_name)?);
            }
        }
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

                let devices = rule.device_spec.as_slice();
                for haptic_event in &events {
                    let addr = &devices[haptic_event.device as usize];
                    let (backend_name, device_id) =
                        addr.split_once('/').expect("validated at startup");
                    backends[backend_name].send_event(device_id.to_string(), haptic_event);
                }
            }
        }
    }
}
