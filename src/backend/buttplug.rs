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
