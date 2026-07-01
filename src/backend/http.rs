use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::routing::get;

use crate::backend::{Backend, DeviceList};
use crate::gestures::Event;

const MAX_EVENTS: usize = 10;
const VALID_DEVICE_ID: &str = "0";

type EventBuffer = Arc<Mutex<VecDeque<String>>>;

pub struct HttpBackend {
    events: EventBuffer,
    bind: String,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    bound_addr: Mutex<Option<std::net::SocketAddr>>,
}

impl HttpBackend {
    pub fn new(bind: String) -> Self {
        Self {
            events: Arc::new(Mutex::new(VecDeque::new())),
            bind,
            shutdown_tx: Mutex::new(None),
            bound_addr: Mutex::new(None),
        }
    }

    // Only read by tests (this crate has no lib target, so the non-test
    // build of the `haptics` bin sees this as unused without the allow).
    #[allow(dead_code)]
    fn bound_addr(&self) -> Option<std::net::SocketAddr> {
        *self.bound_addr.lock().unwrap()
    }
}

async fn list_events(State(events): State<EventBuffer>) -> String {
    let events = events.lock().unwrap();
    events.iter().cloned().collect::<Vec<_>>().join("\n")
}

fn build_router(events: EventBuffer) -> Router {
    Router::new()
        .route("/", get(list_events))
        .with_state(events)
}

impl Backend for HttpBackend {
    fn startup(&self) -> Result<()> {
        let std_listener = std::net::TcpListener::bind(&self.bind)
            .with_context(|| format!("http backend: failed to bind {}", self.bind))?;
        std_listener
            .set_nonblocking(true)
            .context("http backend: failed to set listener non-blocking")?;
        let bound_addr = std_listener.local_addr().ok();
        *self.bound_addr.lock().unwrap() = bound_addr;
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .context("http backend: failed to register listener with tokio runtime")?;

        let events = Arc::clone(&self.events);
        let (tx, rx) = tokio::sync::oneshot::channel();
        *self.shutdown_tx.lock().unwrap() = Some(tx);

        tokio::spawn(async move {
            let app = build_router(events);
            let server = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = rx.await;
            });
            if let Err(e) = server.await {
                eprintln!("http backend: server error: {e}");
            }
        });

        Ok(())
    }

    fn teardown(&self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.lock().unwrap().take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    fn validate_device(&self, device_id: &str) -> Result<()> {
        match self.list_devices()? {
            DeviceList::Anything => Ok(()),
            DeviceList::List(ids) => {
                if ids.iter().any(|id| id == device_id) {
                    Ok(())
                } else {
                    anyhow::bail!(
                        "http backend: unknown device id '{device_id}', valid ids: {ids:?}"
                    )
                }
            }
        }
    }

    fn list_devices(&self) -> Result<DeviceList> {
        Ok(DeviceList::List(vec![VALID_DEVICE_ID.to_string()]))
    }

    fn send_event(&self, device_id: String, event: &Event) {
        let mut events = self.events.lock().unwrap();
        events.push_back(format!("{device_id} {event:?}"));
        while events.len() > MAX_EVENTS {
            events.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[test]
    fn validate_device_accepts_zero() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        assert!(backend.validate_device("0").is_ok());
    }

    #[test]
    fn validate_device_rejects_other_ids() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        let err = backend.validate_device("1").unwrap_err();
        assert!(err.to_string().contains("1"), "{err}");
    }

    #[test]
    fn list_devices_returns_single_zero() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        match backend.list_devices().unwrap() {
            DeviceList::List(ids) => assert_eq!(ids, vec!["0".to_string()]),
            DeviceList::Anything => panic!("expected List, got Anything"),
        }
    }

    #[test]
    fn send_event_truncates_to_last_ten() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        for i in 0..11u32 {
            let event = Event::new(i, 1.0, 0);
            backend.send_event("0".to_string(), &event);
        }
        let events = backend.events.lock().unwrap();
        assert_eq!(events.len(), 10);
        // the 0th push (duration_ms: 0) should have been dropped;
        // the oldest remaining is the 1st push (duration_ms: 1).
        assert!(events.front().unwrap().contains("duration_ms: 1"));
        assert!(events.back().unwrap().contains("duration_ms: 10"));
    }

    #[tokio::test]
    async fn route_returns_events_joined_by_newline() {
        let events: EventBuffer = Arc::new(Mutex::new(VecDeque::from([
            "a".to_string(),
            "b".to_string(),
        ])));
        let app = build_router(events);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"a\nb");
    }

    #[tokio::test]
    async fn route_returns_empty_string_when_no_events() {
        let events: EventBuffer = Arc::new(Mutex::new(VecDeque::new()));
        let app = build_router(events);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"");
    }

    #[tokio::test]
    async fn startup_serves_events_then_teardown_stops_it() {
        let backend = HttpBackend::new("127.0.0.1:0".to_string());
        backend.startup().unwrap();
        let addr = backend
            .bound_addr()
            .expect("startup should have bound a port");

        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));

        // The spawned server task may not have reached accept() yet; retry briefly.
        let mut stream = None;
        for _ in 0..50 {
            match tokio::net::TcpStream::connect(addr).await {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            }
        }
        let mut stream = stream.expect("server should accept a connection shortly after startup");

        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(
            response.contains("0 Event"),
            "response body missing event line: {response}"
        );

        backend.teardown().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            tokio::net::TcpStream::connect(addr).await.is_err(),
            "listener should stop accepting connections after teardown"
        );
    }
}
