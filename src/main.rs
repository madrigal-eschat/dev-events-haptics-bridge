pub mod backend;
pub mod config;
pub mod event;
pub mod gestures;
pub mod player;

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use rumqttc::{AsyncClient, Event as MqttEvent, MqttOptions, Packet, QoS};

use config::{Config, Rule};
use gestures::{lookup, scale};

fn validate_rules(rules: &[Rule]) -> Result<()> {
    for rule in rules {
        let backend_name = rule.device.split_once('/')
            .context("device must be BACKEND/ID")?.0;
        if !backend::is_known(backend_name) {
            bail!("unknown backend '{}' in device '{}'", backend_name, rule.device);
        }
        if lookup(&rule.gesture.name).is_none() {
            bail!("unknown gesture '{}' in rule for device '{}'", rule.gesture.name, rule.device);
        }
        if rule.gesture.speed <= 0.0 {
            bail!("rules[].gesture.speed must be > 0 (got {} for gesture '{}' on device '{}')",
                rule.gesture.speed, rule.gesture.name, rule.device);
        }
        if rule.gesture.scale <= 0.0 {
            bail!("rules[].gesture.scale must be > 0 (got {} for gesture '{}' on device '{}')",
                rule.gesture.scale, rule.gesture.name, rule.device);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = std::env::args().nth(1)
        .context("usage: haptics <config.yaml>")?;
    let config_str = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {config_path}"))?;
    let config: Config = serde_yaml::from_str(&config_str)
        .context("failed to parse config")?;

    validate_rules(&config.rules)?;

    // Build one backend instance per unique backend name.
    let mut backends: HashMap<String, Box<dyn backend::Backend>> = HashMap::new();
    for rule in &config.rules {
        let backend_name = rule.device.split_once('/').expect("validated").0;
        if !backends.contains_key(backend_name) {
            backends.insert(backend_name.to_string(), backend::create(backend_name)?);
        }
    }

    let client_id = config.broker.client_id.as_deref().unwrap_or("haptics-bridge");
    let mut opts = MqttOptions::new(client_id, &config.broker.host, config.broker.port);
    if let Some(auth) = &config.broker.auth {
        opts.set_credentials(&auth.username, &auth.password);
    }

    let (client, mut eventloop) = AsyncClient::new(opts, 64);
    for topic in &config.topics {
        client.subscribe(topic, QoS::AtMostOnce).await?;
    }

    loop {
        match eventloop.poll().await? {
            MqttEvent::Incoming(Packet::Publish(p)) => {
                let payload = match std::str::from_utf8(&p.payload) {
                    Ok(s) => s,
                    Err(_) => { eprintln!("non-UTF8 payload on {}", p.topic); continue; }
                };
                let cloud_event: event::CloudEvent = match serde_json::from_str(payload) {
                    Ok(e) => e,
                    Err(e) => { eprintln!("invalid CloudEvent on {}: {e}", p.topic); continue; }
                };

                for rule in &config.rules {
                    if !rule.filter.matches(&cloud_event) { continue; }

                    let base = lookup(&rule.gesture.name).expect("validated at startup");
                    let events = scale(base, rule.gesture.speed, rule.gesture.scale);

                    let (backend_name, device_id) = rule.device.split_once('/').expect("validated at startup");
                    let backend = &backends[backend_name];
                    for event in &events {
                        backend.send_event(device_id.to_string(), event);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Filter, GestureConfig};

    fn rule(device: &str, gesture: &str, speed: f32, scale: f32) -> Rule {
        Rule {
            device: device.into(),
            filter: Filter::default(),
            gesture: GestureConfig { name: gesture.into(), speed, scale },
        }
    }

    fn valid() -> Rule { rule("stdout/0", "pulse_short", 1.0, 1.0) }

    #[test]
    fn validate_valid_rule() {
        assert!(validate_rules(&[valid()]).is_ok());
    }

    #[test]
    fn validate_empty_rules() {
        assert!(validate_rules(&[]).is_ok());
    }

    #[test]
    fn validate_missing_slash_in_device() {
        let err = validate_rules(&[rule("stdout", "pulse_short", 1.0, 1.0)]).unwrap_err();
        assert!(err.to_string().contains("BACKEND/ID"));
    }

    #[test]
    fn validate_unknown_backend() {
        let err = validate_rules(&[rule("nfc/0", "pulse_short", 1.0, 1.0)]).unwrap_err();
        assert!(err.to_string().contains("nfc"));
    }

    #[test]
    fn validate_unknown_gesture() {
        let err = validate_rules(&[rule("stdout/0", "nonexistent", 1.0, 1.0)]).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn validate_speed_zero() {
        let err = validate_rules(&[rule("stdout/0", "pulse_short", 0.0, 1.0)]).unwrap_err();
        assert!(err.to_string().contains("speed"));
    }

    #[test]
    fn validate_speed_negative() {
        let err = validate_rules(&[rule("stdout/0", "pulse_short", -1.0, 1.0)]).unwrap_err();
        assert!(err.to_string().contains("speed"));
    }

    #[test]
    fn validate_scale_zero() {
        let err = validate_rules(&[rule("stdout/0", "pulse_short", 1.0, 0.0)]).unwrap_err();
        assert!(err.to_string().contains("scale"));
    }

    #[test]
    fn validate_scale_negative() {
        let err = validate_rules(&[rule("stdout/0", "pulse_short", 1.0, -0.5)]).unwrap_err();
        assert!(err.to_string().contains("scale"));
    }
}
