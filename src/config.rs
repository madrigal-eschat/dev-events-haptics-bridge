use crate::{backend, event::CloudEvent, gestures};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub broker: BrokerConfig,
    pub topics: Vec<String>,
    pub rules: Vec<Rule>,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        for rule in &self.rules {
            let devices = rule.device_spec.as_slice();
            if devices.is_empty() {
                bail!("rule has empty devices list");
            }
            for addr in devices {
                let backend_name = addr.split_once('/').context("device must be BACKEND/ID")?.0;
                if !backend::is_known(backend_name) {
                    bail!("unknown backend '{}' in device '{}'", backend_name, addr);
                }
            }
            let gesture_events = match gestures::lookup(&rule.gesture.name) {
                Some(g) => g,
                None => bail!("unknown gesture '{}'", rule.gesture.name),
            };
            if rule.gesture.speed <= 0.0 {
                bail!(
                    "rules[].gesture.speed must be > 0 (got {} for gesture '{}')",
                    rule.gesture.speed,
                    rule.gesture.name,
                );
            }
            if rule.gesture.scale <= 0.0 {
                bail!(
                    "rules[].gesture.scale must be > 0 (got {} for gesture '{}')",
                    rule.gesture.scale,
                    rule.gesture.name,
                );
            }
            let max_idx = gesture_events
                .iter()
                .map(|e| e.device as usize)
                .max()
                .unwrap_or(0);
            let needed = max_idx + 1;
            if devices.len() != needed {
                bail!(
                    "gesture '{}' needs {} device(s) (indices 0..={}) but rule specifies {}",
                    rule.gesture.name,
                    needed,
                    max_idx,
                    devices.len(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct BrokerConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub client_id: Option<String>,
    pub auth: Option<AuthConfig>,
}

fn default_port() -> u16 {
    1883
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    pub filter: Filter,
    pub gesture: GestureConfig,
    #[serde(flatten)]
    pub device_spec: DeviceSpec,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum DeviceSpec {
    One { device: String },
    Many { devices: Vec<String> },
}

impl DeviceSpec {
    pub fn as_slice(&self) -> &[String] {
        match self {
            DeviceSpec::One { device } => std::slice::from_ref(device),
            DeviceSpec::Many { devices } => devices,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct Filter {
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub source: Option<String>,
    pub sourcetype: Option<String>,
    pub subject: Option<String>,
    pub data: Option<HashMap<String, serde_json::Value>>,
}

impl Filter {
    pub fn matches(&self, event: &CloudEvent) -> bool {
        if let Some(pat) = &self.type_
            && !glob_match(pat, &event.type_)
        {
            return false;
        }
        if let Some(pat) = &self.source
            && !glob_match(pat, &event.source)
        {
            return false;
        }
        if let Some(pat) = &self.sourcetype
            && !glob_match(pat, &event.sourcetype)
        {
            return false;
        }
        if let Some(pat) = &self.subject {
            match &event.subject {
                None => return false,
                Some(s) => {
                    if !glob_match(pat, s) {
                        return false;
                    }
                }
            }
        }
        if let Some(data_filter) = &self.data {
            for (key, expected) in data_filter {
                if event.data.get(key) != Some(expected) {
                    return false;
                }
            }
        }
        true
    }
}

/// `*` matches any sequence of characters (including `/`).
fn glob_match(pattern: &str, value: &str) -> bool {
    match pattern.find('*') {
        None => pattern == value,
        Some(star_pos) => {
            let prefix = &pattern[..star_pos];
            let rest_pat = &pattern[star_pos + 1..];
            if !value.starts_with(prefix) {
                return false;
            }
            let after = &value[prefix.len()..];
            let mut pos = 0;
            loop {
                if glob_match(rest_pat, &after[pos..]) {
                    return true;
                }
                match after[pos..].chars().next() {
                    None => return false,
                    Some(c) => pos += c.len_utf8(),
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GestureConfig {
    pub name: String,
    #[serde(default = "one")]
    pub scale: f32,
    #[serde(default = "one")]
    pub speed: f32,
}

fn one() -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(device: &str, gesture: &str, speed: f32, scale: f32) -> Rule {
        Rule {
            device_spec: DeviceSpec::One {
                device: device.into(),
            },
            filter: Filter::default(),
            gesture: GestureConfig {
                name: gesture.into(),
                speed,
                scale,
            },
        }
    }

    fn rule2(d0: &str, d1: &str, gesture: &str) -> Rule {
        Rule {
            device_spec: DeviceSpec::Many {
                devices: vec![d0.into(), d1.into()],
            },
            filter: Filter::default(),
            gesture: GestureConfig {
                name: gesture.into(),
                speed: 1.0,
                scale: 1.0,
            },
        }
    }

    fn cfg(rules: Vec<Rule>) -> Config {
        Config {
            broker: BrokerConfig {
                host: "localhost".into(),
                port: 1883,
                client_id: None,
                auth: None,
            },
            topics: vec![],
            rules,
        }
    }

    fn valid() -> Rule {
        rule("stdout/0", "pulse_short", 1.0, 1.0)
    }

    fn valid2() -> Rule {
        rule2("stdout/0", "stdout/1", "stop_all")
    }

    // ── Config::validate ───────────────────────────────────────────────────

    #[test]
    fn validate_valid_rule() {
        assert!(cfg(vec![valid()]).validate().is_ok());
    }

    #[test]
    fn validate_empty_rules() {
        assert!(cfg(vec![]).validate().is_ok());
    }

    #[test]
    fn validate_missing_slash_in_device() {
        let err = cfg(vec![rule("stdout", "pulse_short", 1.0, 1.0)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("BACKEND/ID"));
    }

    #[test]
    fn validate_unknown_backend() {
        let err = cfg(vec![rule("nfc/0", "pulse_short", 1.0, 1.0)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("nfc"));
    }

    #[test]
    fn validate_unknown_gesture() {
        let err = cfg(vec![rule("stdout/0", "nonexistent", 1.0, 1.0)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn validate_speed_zero() {
        let err = cfg(vec![rule("stdout/0", "pulse_short", 0.0, 1.0)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("speed"));
    }

    #[test]
    fn validate_speed_negative() {
        let err = cfg(vec![rule("stdout/0", "pulse_short", -1.0, 1.0)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("speed"));
    }

    #[test]
    fn validate_scale_zero() {
        let err = cfg(vec![rule("stdout/0", "pulse_short", 1.0, 0.0)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("scale"));
    }

    #[test]
    fn validate_scale_negative() {
        let err = cfg(vec![rule("stdout/0", "pulse_short", 1.0, -0.5)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("scale"));
    }

    #[test]
    fn validate_two_device_gesture_valid() {
        assert!(cfg(vec![valid2()]).validate().is_ok());
    }

    #[test]
    fn validate_device_count_too_few() {
        // stop_all needs 2 devices but only 1 given
        let err = cfg(vec![rule("stdout/0", "stop_all", 1.0, 1.0)])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("needs 2"));
    }

    #[test]
    fn validate_device_count_too_many() {
        // pulse_short needs 1 device but 2 given
        let err = cfg(vec![rule2("stdout/0", "stdout/1", "pulse_short")])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("needs 1"));
    }

    #[test]
    fn validate_empty_devices_list() {
        let r = Rule {
            device_spec: DeviceSpec::Many { devices: vec![] },
            filter: Filter::default(),
            gesture: GestureConfig {
                name: "pulse_short".into(),
                speed: 1.0,
                scale: 1.0,
            },
        };
        let err = cfg(vec![r]).validate().unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    fn event(
        type_: &str,
        source: &str,
        sourcetype: &str,
        subject: Option<&str>,
        data: serde_json::Value,
    ) -> CloudEvent {
        CloudEvent {
            id: "test-id".into(),
            type_: type_.into(),
            source: source.into(),
            sourcetype: sourcetype.into(),
            subject: subject.map(Into::into),
            data,
        }
    }

    fn any_event() -> CloudEvent {
        event(
            "devevents.task.failed",
            "editor/jeff/vscode",
            "editor",
            None,
            json!({}),
        )
    }

    // ── glob_match ─────────────────────────────────────────────────────────

    #[test]
    fn glob_exact_match() {
        assert!(glob_match("foo", "foo"));
        assert!(!glob_match("foo", "bar"));
        assert!(!glob_match("foo", "foobar"));
    }

    #[test]
    fn glob_star_only_matches_anything() {
        assert!(glob_match("*", ""));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", "with/slashes"));
    }

    #[test]
    fn glob_prefix_star() {
        assert!(glob_match("foo*", "foobar"));
        assert!(glob_match("foo*", "foo"));
        assert!(!glob_match("foo*", "bar"));
    }

    #[test]
    fn glob_suffix_star() {
        assert!(glob_match("*bar", "foobar"));
        assert!(glob_match("*bar", "bar"));
        assert!(!glob_match("*bar", "foo"));
    }

    #[test]
    fn glob_middle_star() {
        assert!(glob_match("foo*baz", "foobarbaz"));
        assert!(glob_match("foo*baz", "foobaz")); // zero chars
        assert!(!glob_match("foo*baz", "foobaz_wrong"));
    }

    #[test]
    fn glob_star_crosses_slash() {
        assert!(glob_match("editor/*/vscode", "editor/jeff/vscode"));
        assert!(glob_match(
            "editor/*/vscode",
            "editor/jeff/jetbrains/vscode"
        ));
        assert!(!glob_match("editor/*/vscode", "editor/jeff/pycharm"));
    }

    #[test]
    fn glob_multiple_stars() {
        assert!(glob_match("a*b*c", "axbyc"));
        assert!(glob_match("a*b*c", "abc")); // zero-width matches
        assert!(!glob_match("a*b*c", "axc")); // missing b
    }

    #[test]
    fn glob_empty_pattern_matches_empty_only() {
        assert!(glob_match("", ""));
        assert!(!glob_match("", "foo"));
    }

    // ── Filter::matches ────────────────────────────────────────────────────

    #[test]
    fn filter_empty_matches_any_event() {
        let f = Filter::default();
        assert!(f.matches(&any_event()));
    }

    #[test]
    fn filter_type_exact_match() {
        let f = Filter {
            type_: Some("devevents.task.failed".into()),
            ..Default::default()
        };
        assert!(f.matches(&any_event()));
    }

    #[test]
    fn filter_type_no_match() {
        let f = Filter {
            type_: Some("devevents.task.succeeded".into()),
            ..Default::default()
        };
        assert!(!f.matches(&any_event()));
    }

    #[test]
    fn filter_type_glob() {
        let f = Filter {
            type_: Some("devevents.task.*".into()),
            ..Default::default()
        };
        assert!(f.matches(&any_event()));
        assert!(f.matches(&event(
            "devevents.task.succeeded",
            "s",
            "editor",
            None,
            json!({})
        )));
        assert!(!f.matches(&event(
            "devevents.file.saved",
            "s",
            "editor",
            None,
            json!({})
        )));
    }

    #[test]
    fn filter_source_glob() {
        let f = Filter {
            source: Some("editor/*".into()),
            ..Default::default()
        };
        assert!(f.matches(&any_event()));
        assert!(!f.matches(&event(
            "t",
            "service/gitlab/gitlab.com",
            "service",
            None,
            json!({})
        )));
    }

    #[test]
    fn filter_sourcetype_exact() {
        let f = Filter {
            sourcetype: Some("service".into()),
            ..Default::default()
        };
        assert!(!f.matches(&any_event())); // any_event is "editor"
        assert!(f.matches(&event("t", "service/gitlab", "service", None, json!({}))));
    }

    #[test]
    fn filter_subject_event_has_none_returns_false() {
        let f = Filter {
            subject: Some("~/projects/foo".into()),
            ..Default::default()
        };
        assert!(!f.matches(&any_event())); // any_event has no subject
    }

    #[test]
    fn filter_subject_matches() {
        let f = Filter {
            subject: Some("~/projects/*".into()),
            ..Default::default()
        };
        let e = event("t", "s", "editor", Some("~/projects/my-app"), json!({}));
        assert!(f.matches(&e));
    }

    #[test]
    fn filter_data_key_matches() {
        let f = Filter {
            data: Some([("exit_code".into(), json!(1))].into()),
            ..Default::default()
        };
        let e = event("t", "s", "editor", None, json!({ "exit_code": 1 }));
        assert!(f.matches(&e));
    }

    #[test]
    fn filter_data_key_value_mismatch() {
        let f = Filter {
            data: Some([("exit_code".into(), json!(1))].into()),
            ..Default::default()
        };
        let e = event("t", "s", "editor", None, json!({ "exit_code": 2 }));
        assert!(!f.matches(&e));
    }

    #[test]
    fn filter_data_key_missing_from_event() {
        let f = Filter {
            data: Some([("exit_code".into(), json!(1))].into()),
            ..Default::default()
        };
        let e = event("t", "s", "editor", None, json!({}));
        assert!(!f.matches(&e));
    }

    #[test]
    fn filter_multiple_fields_all_must_match() {
        let f = Filter {
            type_: Some("devevents.task.failed".into()),
            sourcetype: Some("editor".into()),
            ..Default::default()
        };
        assert!(f.matches(&any_event()));

        let f2 = Filter {
            type_: Some("devevents.task.failed".into()),
            sourcetype: Some("service".into()),
            ..Default::default()
        };
        assert!(!f2.matches(&any_event()));
    }
}
