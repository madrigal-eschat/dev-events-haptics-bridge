use anyhow::Result;

use crate::backend::{Backend, DeviceList};
use crate::config::ButtplugConfig;
use crate::gestures::Event;

pub struct ButtplugBackend {
    pub config: ButtplugConfig,
}

impl ButtplugBackend {
    pub fn new(config: ButtplugConfig) -> Self {
        Self { config }
    }
}

impl Backend for ButtplugBackend {
    fn startup(&self) -> Result<()> {
        Ok(())
    }

    fn teardown(&self) -> Result<()> {
        Ok(())
    }

    fn validate_device(&self, _device_id: &str) -> Result<()> {
        Ok(())
    }

    fn list_devices(&self) -> Result<DeviceList> {
        Ok(DeviceList::Anything)
    }

    fn resolve_device_ids(&self, device_ids: &[String]) -> Result<Vec<String>> {
        Ok(device_ids.to_vec())
    }

    fn send_event(&self, _device_id: String, _event: &Event) {}
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
    fn resolve_device_ids_preserves_input_shape_for_now() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        let ids = vec!["0".to_string(), "Lovense%20Nora".to_string()];
        assert_eq!(backend.resolve_device_ids(&ids).unwrap(), ids);
    }
}
