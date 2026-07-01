use crate::gestures::Event;

pub mod http;

pub enum DeviceList {
    Anything,
    List(Vec<String>),
}

pub trait Backend: Send + Sync {
    fn startup(&self) -> anyhow::Result<()>;
    fn teardown(&self) -> anyhow::Result<()>;

    /// On failure, the returned error is printed to the console (stderr) by
    /// the caller (see main.rs's device validation pass) — implementors just
    /// return a descriptive `Err`, they don't print anything themselves.
    fn validate_device(&self, device_id: &str) -> anyhow::Result<()>;

    fn list_devices(&self) -> anyhow::Result<DeviceList>;
    fn send_event(&self, device_id: String, event: &Event);
}

pub struct StdoutBackend;

impl Backend for StdoutBackend {
    fn startup(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn teardown(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn validate_device(&self, device_id: &str) -> anyhow::Result<()> {
        match self.list_devices()? {
            DeviceList::Anything => Ok(()),
            DeviceList::List(ids) => {
                anyhow::bail!("stdout backend: unknown device id '{device_id}', valid ids: {ids:?}")
            }
        }
    }

    fn list_devices(&self) -> anyhow::Result<DeviceList> {
        Ok(DeviceList::Anything)
    }

    fn send_event(&self, device_id: String, event: &Event) {
        println!("{device_id} {event:?}");
    }
}

pub fn is_known(name: &str) -> bool {
    matches!(name, "stdout" | "http")
}

pub fn create(name: &str, config: &crate::config::Config) -> anyhow::Result<Box<dyn Backend>> {
    match name {
        "stdout" => Ok(Box::new(StdoutBackend)),
        "http" => {
            let bind = config.http.clone().unwrap_or_default().bind;
            Ok(Box::new(http::HttpBackend::new(bind)))
        }
        other => anyhow::bail!("unknown backend: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BrokerConfig, Config};

    fn test_config() -> Config {
        Config {
            broker: BrokerConfig {
                host: "localhost".into(),
                port: 1883,
                client_id: None,
                auth: None,
            },
            topics: vec![],
            rules: vec![],
            http: None,
        }
    }

    #[test]
    fn create_known_backends_succeed() {
        assert!(create("stdout", &test_config()).is_ok());
        assert!(create("http", &test_config()).is_ok());
    }

    #[test]
    fn create_unknown_backend_errors() {
        let err = create("nonexistent", &test_config())
            .err()
            .expect("expected error");
        assert!(err.to_string().contains("nonexistent"), "{err}");
    }

    #[test]
    fn is_known_stdout_and_http() {
        assert!(is_known("stdout"));
        assert!(is_known("http"));
        assert!(!is_known("nonexistent"));
        assert!(!is_known("STDOUT"));
    }

    #[test]
    fn create_http_uses_configured_bind() {
        let mut config = test_config();
        config.http = Some(crate::config::HttpConfig {
            bind: "127.0.0.1:9999".to_string(),
        });
        assert!(create("http", &config).is_ok());
    }

    #[test]
    fn stdout_list_devices_is_anything() {
        let backend = StdoutBackend;
        assert!(matches!(
            backend.list_devices().unwrap(),
            DeviceList::Anything
        ));
    }

    #[test]
    fn stdout_validate_device_always_ok() {
        let backend = StdoutBackend;
        assert!(backend.validate_device("anything").is_ok());
        assert!(backend.validate_device("").is_ok());
    }

    #[test]
    fn stdout_startup_and_teardown_ok() {
        let backend = StdoutBackend;
        assert!(backend.startup().is_ok());
        assert!(backend.teardown().is_ok());
    }
}
