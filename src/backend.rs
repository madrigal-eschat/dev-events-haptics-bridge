use crate::gestures::Event;

pub trait Backend: Send + Sync {
    fn send_event(&self, device_id: String, event: &Event);
}

pub struct StdoutBackend;

impl Backend for StdoutBackend {
    fn send_event(&self, device_id: String, event: &Event) {
        println!("{device_id} {event:?}");
    }
}

pub fn is_known(name: &str) -> bool {
    matches!(name, "stdout")
}

pub fn create(name: &str) -> anyhow::Result<Box<dyn Backend>> {
    match name {
        "stdout" => Ok(Box::new(StdoutBackend)),
        other => anyhow::bail!("unknown backend: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_known_backends_succeed() {
        assert!(create("stdout").is_ok());
    }

    #[test]
    fn create_unknown_backend_errors() {
        let err = create("nonexistent").err().expect("expected error");
        assert!(err.to_string().contains("nonexistent"), "{err}");
    }

    #[test]
    fn is_known_stdout() {
        assert!(is_known("stdout"));
        assert!(!is_known("nonexistent"));
        assert!(!is_known("STDOUT"));
    }
}
