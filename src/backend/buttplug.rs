use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use buttplug_client::ButtplugClient;
use buttplug_client::connector::ButtplugRemoteClientConnector;
use buttplug_client::device::{ClientDeviceCommandValue, ClientDeviceOutputCommand};
use buttplug_client::serializer::ButtplugClientJSONSerializer;
use buttplug_client::{ButtplugClientDevice, ButtplugClientEvent};
use buttplug_core::message::OutputType;
use buttplug_transport_websocket_tungstenite::ButtplugWebsocketClientTransport;
use futures::StreamExt;
use strum::IntoEnumIterator;
use tokio::sync::{mpsc, oneshot, watch};

use crate::backend::{Backend, DeviceList};
use crate::config::ButtplugConfig;
use crate::gestures::Event;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DeviceLookup {
    ByIndex(u32),
    ByName(String),
}

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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ButtplugTelemetry {
    processed_jobs: usize,
    dropped_unstarted: usize,
    dropped_invalid_device: usize,
    dropped_full: usize,
    dropped_disconnected: usize,
    dropped_unknown_device: usize,
}

#[derive(Debug, Clone, PartialEq)]
enum ButtplugCommand {
    Vibrate { magnitude: f32 },
    Linear { position: f32, duration_ms: u32 },
    Rotate { speed: f32 },
}

#[derive(Debug, Clone)]
struct GestureJob {
    lookup: DeviceLookup,
    actuator_index: Option<u32>,
    event: Event,
}

#[derive(Debug, Clone, Default)]
struct DeviceInfo {
    device_index: u32,
    actuators: Vec<Actuator>,
}

type DeviceCache = HashMap<DeviceLookup, Arc<DeviceInfo>>;

#[derive(Debug, Default)]
enum WorkerState {
    #[default]
    Stopped,
    Running {
        tx: mpsc::Sender<GestureJob>,
        shutdown_tx: oneshot::Sender<()>,
        worker: std::thread::JoinHandle<()>,
    },
    Stopping {
        worker: Option<std::thread::JoinHandle<()>>,
    },
}

pub struct ButtplugBackend {
    pub config: ButtplugConfig,
    device_cache: Arc<Mutex<DeviceCache>>,
    state: Mutex<WorkerState>,
    processed_jobs: Arc<AtomicUsize>,
    dropped_unstarted: AtomicUsize,
    dropped_invalid_device: AtomicUsize,
    dropped_full: AtomicUsize,
    dropped_disconnected: Arc<AtomicUsize>,
    dropped_unknown_device: Arc<AtomicUsize>,
}

impl ButtplugBackend {
    pub fn new(config: ButtplugConfig) -> Self {
        Self {
            config,
            device_cache: Arc::new(Mutex::new(HashMap::new())),
            state: Mutex::new(WorkerState::Stopped),
            processed_jobs: Arc::new(AtomicUsize::new(0)),
            dropped_unstarted: AtomicUsize::new(0),
            dropped_invalid_device: AtomicUsize::new(0),
            dropped_full: AtomicUsize::new(0),
            dropped_disconnected: Arc::new(AtomicUsize::new(0)),
            dropped_unknown_device: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn startup_with_runtime<F>(&self, build_runtime: F) -> Result<()>
    where
        F: FnOnce() -> Result<tokio::runtime::Runtime>,
    {
        self.reclaim_stopped_worker();

        let mut state = self.state.lock().unwrap();
        match &*state {
            WorkerState::Running { .. } => return Ok(()),
            WorkerState::Stopping { .. } => {
                bail!("buttplug backend: shutdown still in progress");
            }
            WorkerState::Stopped => {}
        }

        let runtime = build_runtime()?;
        let (tx, rx) = mpsc::channel(64);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let device_cache = Arc::clone(&self.device_cache);
        let processed_jobs = Arc::clone(&self.processed_jobs);
        let dropped_disconnected = Arc::clone(&self.dropped_disconnected);
        let dropped_unknown_device = Arc::clone(&self.dropped_unknown_device);
        let config = self.config.clone();
        let worker = std::thread::Builder::new()
            .name("buttplug-backend".to_string())
            .spawn(move || {
                runtime.block_on(run_worker(
                    rx,
                    shutdown_rx,
                    device_cache,
                    processed_jobs,
                    dropped_disconnected,
                    dropped_unknown_device,
                    config,
                ));
            })?;

        *state = WorkerState::Running {
            tx,
            shutdown_tx,
            worker,
        };

        Ok(())
    }

    fn reclaim_stopped_worker(&self) {
        let worker = {
            let mut state = self.state.lock().unwrap();
            match &mut *state {
                WorkerState::Stopping { worker }
                    if worker.as_ref().is_some_and(|worker| worker.is_finished()) =>
                {
                    let worker = worker.take().unwrap();
                    *state = WorkerState::Stopped;
                    Some(worker)
                }
                _ => None,
            }
        };

        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    fn wait_for_worker_shutdown(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;

        loop {
            self.reclaim_stopped_worker();

            if matches!(*self.state.lock().unwrap(), WorkerState::Stopped) {
                return Ok(());
            }

            if Instant::now() >= deadline {
                let telemetry = self.telemetry_snapshot();
                eprintln!(
                    "buttplug backend: timed out waiting for worker shutdown after {timeout:?} (telemetry: {telemetry:?})"
                );
                bail!(
                    "buttplug backend: worker shutdown timed out after {timeout:?} (telemetry: {telemetry:?})"
                );
            }

            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(test)]
    fn seed_device(
        &self,
        lookup: DeviceLookup,
        device_index: u32,
        actuator_kinds: Vec<ActuatorKind>,
    ) {
        self.seed_device_aliases(vec![lookup], device_index, actuator_kinds);
    }

    #[cfg(test)]
    fn seed_device_aliases(
        &self,
        lookups: Vec<DeviceLookup>,
        device_index: u32,
        actuator_kinds: Vec<ActuatorKind>,
    ) {
        let actuators = actuator_kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| Actuator {
                index: index as u32,
                kind,
            })
            .collect();
        let device = Arc::new(DeviceInfo {
            device_index,
            actuators,
        });
        let mut cache = self.device_cache.lock().unwrap();
        for lookup in lookups {
            cache.insert(lookup, Arc::clone(&device));
        }
    }
}

impl Backend for ButtplugBackend {
    fn startup(&self) -> Result<()> {
        self.startup_with_runtime(|| {
            Ok(tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?)
        })
    }

    fn teardown(&self) -> Result<()> {
        let shutdown_requested = {
            let mut state = self.state.lock().unwrap();
            match std::mem::replace(&mut *state, WorkerState::Stopped) {
                WorkerState::Running {
                    tx,
                    shutdown_tx,
                    worker,
                } => {
                    drop(tx);
                    let _ = shutdown_tx.send(());
                    *state = WorkerState::Stopping {
                        worker: Some(worker),
                    };
                    true
                }
                WorkerState::Stopping { worker } => {
                    *state = WorkerState::Stopping { worker };
                    false
                }
                WorkerState::Stopped => false,
            }
        };

        if shutdown_requested {
            self.wait_for_worker_shutdown(Duration::from_millis(500))?;
        } else {
            self.reclaim_stopped_worker();
            if matches!(*self.state.lock().unwrap(), WorkerState::Stopping { .. }) {
                bail!("buttplug backend: shutdown still in progress");
            }
        }

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
                && let Some(device_actuator) = device
                    .actuators
                    .iter()
                    .copied()
                    .find(|device_actuator| device_actuator.index == *actuator)
            {
                selected
                    .entry(lookup.clone())
                    .or_default()
                    .insert(device_actuator);
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
                        Some(selected.entry(lookup.clone()).or_default()),
                    );
                    debug_assert!(
                        chosen.is_some(),
                        "choose_actuator returned None for non-empty device actuators in device id '{raw}'"
                    );
                    let chosen = chosen.unwrap_or_else(|| device.actuators[0]);
                    selected.get_mut(&lookup).unwrap().insert(chosen);
                    resolved.push(format_device_id(&lookup, Some(chosen.index)));
                }
            }
        }

        Ok(resolved)
    }

    fn send_event(&self, device_id: String, event: &Event) {
        let (lookup, actuator_index) = match parse_device_id(&device_id) {
            Ok(parsed) => parsed,
            Err(err) => {
                let dropped_invalid_device =
                    self.dropped_invalid_device.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!(
                    "buttplug backend: {err} (dropped_invalid_device={dropped_invalid_device})"
                );
                return;
            }
        };

        let job = GestureJob {
            lookup,
            actuator_index,
            event: *event,
        };

        let state = self.state.lock().unwrap();
        let Some(sender) = (match &*state {
            WorkerState::Running { tx, .. } => Some(tx),
            WorkerState::Stopped | WorkerState::Stopping { .. } => None,
        }) else {
            let dropped_unstarted = self.dropped_unstarted.fetch_add(1, Ordering::SeqCst) + 1;
            eprintln!(
                "buttplug backend: dropped event because backend is not started (device_id='{device_id}', dropped_unstarted={dropped_unstarted})"
            );
            return;
        };

        if let Err(err) = sender.try_send(job) {
            let dropped_full = self.dropped_full.fetch_add(1, Ordering::SeqCst) + 1;
            eprintln!(
                "buttplug backend: failed to queue event for '{device_id}' (dropped_full={dropped_full}): {err}"
            );
        }
    }
}

async fn run_worker(
    mut rx: mpsc::Receiver<GestureJob>,
    mut shutdown_rx: oneshot::Receiver<()>,
    device_cache: Arc<Mutex<DeviceCache>>,
    processed_jobs: Arc<AtomicUsize>,
    dropped_disconnected: Arc<AtomicUsize>,
    dropped_unknown_device: Arc<AtomicUsize>,
    config: ButtplugConfig,
) {
    let (client_tx, client_rx) = watch::channel(None);
    let connection_task = tokio::spawn(connection_manager(
        config,
        Arc::clone(&device_cache),
        client_tx,
    ));

    let mut shutdown_requested = false;
    loop {
        if shutdown_requested {
            match rx.try_recv() {
                Ok(job) => {
                    processed_jobs.fetch_add(1, Ordering::SeqCst);
                    handle_job(
                        &device_cache,
                        client_rx.borrow().as_ref(),
                        &dropped_disconnected,
                        &dropped_unknown_device,
                        job,
                    );
                    continue;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        tokio::select! {
            _ = &mut shutdown_rx => {
                log::info!("buttplug backend: shutdown requested");
                shutdown_requested = true;
            }
            maybe_job = rx.recv() => {
                let Some(job) = maybe_job else {
                    break;
                };
                processed_jobs.fetch_add(1, Ordering::SeqCst);
                handle_job(
                    &device_cache,
                    client_rx.borrow().as_ref(),
                    &dropped_disconnected,
                    &dropped_unknown_device,
                    job,
                );
            }
        }
    }

    connection_task.abort();
}

fn handle_job(
    device_cache: &Arc<Mutex<DeviceCache>>,
    client: Option<&Arc<ButtplugClient>>,
    dropped_disconnected: &AtomicUsize,
    dropped_unknown_device: &AtomicUsize,
    job: GestureJob,
) {
    let device_index = {
        let cache = device_cache.lock().unwrap();
        let Some(device) = cache.get(&job.lookup) else {
            let known: Vec<&DeviceLookup> = cache.keys().collect();
            log::error!(
                "buttplug backend: unknown device lookup {:?} (known devices: {:?})",
                job.lookup,
                known
            );
            return;
        };

        let actuator = match job.actuator_index {
            Some(index) => device
                .actuators
                .iter()
                .copied()
                .find(|actuator| actuator.index == index),
            None => choose_actuator(&device.actuators, None),
        };

        let Some(actuator) = actuator else {
            log::warn!(
                "buttplug backend: device {:?} has no usable actuators",
                job.lookup
            );
            return;
        };

        let Some(command) = translate_event(actuator.kind, &job.event) else {
            log::warn!(
                "buttplug backend: unsupported actuator kind {:?} for device {:?}",
                actuator.kind,
                job.lookup
            );
            return;
        };

        log::debug!(
            "buttplug backend: dispatch {:?} actuator {} as {:?}",
            job.lookup,
            actuator.index,
            command
        );

        (device.device_index, actuator.index, command)
    };
    let (device_index, feature_index, command) = device_index;

    let Some(client) = client else {
        let dropped = dropped_disconnected.fetch_add(1, Ordering::SeqCst) + 1;
        log::warn!(
            "buttplug backend: dropped job for device {:?} (not connected, dropped_disconnected={dropped})",
            job.lookup
        );
        return;
    };

    let Some(device) = client.devices().get(&device_index).cloned() else {
        let dropped = dropped_unknown_device.fetch_add(1, Ordering::SeqCst) + 1;
        log::warn!(
            "buttplug backend: device index {device_index} not present on server (dropped_unknown_device={dropped})"
        );
        return;
    };

    let Some(feature) = device.device_features().get(&feature_index).cloned() else {
        let dropped = dropped_unknown_device.fetch_add(1, Ordering::SeqCst) + 1;
        log::warn!(
            "buttplug backend: feature {feature_index} not present on device {device_index} (dropped_unknown_device={dropped})"
        );
        return;
    };

    let output_command = match command {
        ButtplugCommand::Vibrate { magnitude } => {
            ClientDeviceOutputCommand::Vibrate(ClientDeviceCommandValue::Percent(magnitude as f64))
        }
        ButtplugCommand::Rotate { speed } => {
            ClientDeviceOutputCommand::Rotate(ClientDeviceCommandValue::Percent(speed as f64))
        }
        ButtplugCommand::Linear {
            position,
            duration_ms,
        } => ClientDeviceOutputCommand::HwPositionWithDuration(
            ClientDeviceCommandValue::Percent(position as f64),
            duration_ms,
        ),
    };

    tokio::spawn(async move {
        if let Err(e) = feature.run_output(&output_command).await {
            log::warn!(
                "buttplug backend: send failed for device {device_index} feature {feature_index}: {e}"
            );
        }
    });
}

fn build_connector(
    server: &str,
) -> ButtplugRemoteClientConnector<ButtplugWebsocketClientTransport, ButtplugClientJSONSerializer> {
    let transport = if is_secure_websocket(server) {
        ButtplugWebsocketClientTransport::new_secure_connector(server, false)
    } else {
        ButtplugWebsocketClientTransport::new_insecure_connector(server)
    };
    ButtplugRemoteClientConnector::<ButtplugWebsocketClientTransport, ButtplugClientJSONSerializer>::new(
        transport,
    )
}

fn device_info_from_client_device(device: &ButtplugClientDevice) -> DeviceInfo {
    let actuators = device
        .device_features()
        .iter()
        .filter_map(|(feature_index, feature)| {
            let outputs: Vec<OutputType> = OutputType::iter()
                .filter(|output_type| feature.feature().contains_output(*output_type))
                .collect();
            classify_output_types(&outputs).map(|kind| Actuator {
                index: *feature_index,
                kind,
            })
        })
        .collect();
    DeviceInfo {
        device_index: device.index(),
        actuators,
    }
}

fn resync_device_cache(device_cache: &Arc<Mutex<DeviceCache>>, client: &ButtplugClient) {
    let mut new_cache: DeviceCache = HashMap::new();
    for (_, device) in client.devices() {
        let info = Arc::new(device_info_from_client_device(&device));
        new_cache.insert(DeviceLookup::ByIndex(device.index()), Arc::clone(&info));
        new_cache.insert(DeviceLookup::ByName(device.name().clone()), info);
    }
    let count = new_cache.len();
    *device_cache.lock().unwrap() = new_cache;
    log::info!("buttplug backend: device cache resynced ({count} lookup entries)");
}

async fn connection_manager(
    config: ButtplugConfig,
    device_cache: Arc<Mutex<DeviceCache>>,
    client_tx: watch::Sender<Option<Arc<ButtplugClient>>>,
) {
    let mut backoff = Backoff::new(config.max_backoff_ms);
    loop {
        let client = ButtplugClient::new("haptics-bridge");
        let connector = build_connector(&config.server);
        let connect_result = tokio::time::timeout(
            Duration::from_millis(config.connection_timeout_ms),
            client.connect(connector),
        )
        .await;

        match connect_result {
            Ok(Ok(())) => {
                log::info!("buttplug backend: connected to {}", config.server);
                backoff.reset();
                let client = Arc::new(client);
                resync_device_cache(&device_cache, &client);
                let _ = client_tx.send(Some(Arc::clone(&client)));
                run_connected_session(&client, &device_cache, &config).await;
                let _ = client_tx.send(None);
                log::warn!("buttplug backend: disconnected from {}", config.server);
            }
            Ok(Err(e)) => {
                log::warn!("buttplug backend: connect to {} failed: {e}", config.server);
            }
            Err(_) => {
                log::warn!(
                    "buttplug backend: connect to {} timed out after {}ms",
                    config.server,
                    config.connection_timeout_ms
                );
            }
        }

        let delay = backoff.next();
        log::info!(
            "buttplug backend: reconnecting to {} in {delay:?}",
            config.server
        );
        tokio::time::sleep(delay).await;
    }
}

async fn run_connected_session(
    client: &Arc<ButtplugClient>,
    device_cache: &Arc<Mutex<DeviceCache>>,
    config: &ButtplugConfig,
) {
    let mut events = client.event_stream();
    if let Err(e) = client.start_scanning().await {
        log::warn!("buttplug backend: start_scanning failed: {e}");
    }
    let mut scan_interval = tokio::time::interval(Duration::from_millis(config.scan_interval_ms));
    scan_interval.tick().await; // first tick fires immediately; we already scanned above

    loop {
        tokio::select! {
            event = events.next() => {
                match event {
                    Some(ButtplugClientEvent::DeviceAdded(device)) => {
                        log::info!("buttplug backend: device added: {}", device.name());
                        resync_device_cache(device_cache, client);
                    }
                    Some(ButtplugClientEvent::DeviceRemoved(device)) => {
                        log::info!("buttplug backend: device removed: {}", device.name());
                        resync_device_cache(device_cache, client);
                    }
                    Some(ButtplugClientEvent::ServerDisconnect) => {
                        log::warn!("buttplug backend: server disconnected");
                        return;
                    }
                    Some(_) => {}
                    None => {
                        log::warn!("buttplug backend: event stream ended");
                        return;
                    }
                }
            }
            _ = scan_interval.tick() => {
                if let Err(e) = client.start_scanning().await {
                    log::warn!("buttplug backend: start_scanning failed: {e}");
                }
            }
        }
        if !client.connected() {
            return;
        }
    }
}

impl ButtplugBackend {
    pub fn telemetry_snapshot(&self) -> ButtplugTelemetry {
        ButtplugTelemetry {
            processed_jobs: self.processed_jobs.load(Ordering::SeqCst),
            dropped_unstarted: self.dropped_unstarted.load(Ordering::SeqCst),
            dropped_invalid_device: self.dropped_invalid_device.load(Ordering::SeqCst),
            dropped_full: self.dropped_full.load(Ordering::SeqCst),
            dropped_disconnected: self.dropped_disconnected.load(Ordering::SeqCst),
            dropped_unknown_device: self.dropped_unknown_device.load(Ordering::SeqCst),
        }
    }

    #[cfg(test)]
    fn telemetry(&self) -> ButtplugTelemetry {
        self.telemetry_snapshot()
    }

    #[cfg(test)]
    fn install_test_sender(&self, tx: mpsc::Sender<GestureJob>) {
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        let worker = std::thread::spawn(|| {});
        *self.state.lock().unwrap() = WorkerState::Running {
            tx,
            shutdown_tx,
            worker,
        };
    }
}

fn classify_output_types(outputs: &[OutputType]) -> Option<ActuatorKind> {
    if outputs.contains(&OutputType::Vibrate) {
        Some(ActuatorKind::Vibrate)
    } else if outputs.contains(&OutputType::HwPositionWithDuration) {
        Some(ActuatorKind::Linear)
    } else if outputs.contains(&OutputType::Rotate) {
        Some(ActuatorKind::Rotate)
    } else {
        None
    }
}

fn translate_event(kind: ActuatorKind, event: &Event) -> Option<ButtplugCommand> {
    let magnitude = event.magnitude.abs().clamp(0.0, 1.0);
    Some(match kind {
        ActuatorKind::Vibrate => ButtplugCommand::Vibrate { magnitude },
        ActuatorKind::Linear => ButtplugCommand::Linear {
            position: magnitude,
            duration_ms: event.duration_ms,
        },
        ActuatorKind::Rotate => ButtplugCommand::Rotate { speed: magnitude },
    })
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

    let lookup = percent_decode(lookup, device_id)?;
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

fn is_secure_websocket(server: &str) -> bool {
    server.starts_with("wss://")
}

struct Backoff {
    initial_ms: u64,
    current_ms: u64,
    max_ms: u64,
}

impl Backoff {
    fn new(max_ms: u64) -> Self {
        let initial_ms = 1_000u64.min(max_ms.max(1));
        Self {
            initial_ms,
            current_ms: initial_ms,
            max_ms: max_ms.max(initial_ms),
        }
    }

    fn reset(&mut self) {
        self.current_ms = self.initial_ms;
    }

    fn next(&mut self) -> Duration {
        let delay = Duration::from_millis(self.current_ms);
        self.current_ms = (self.current_ms * 2).min(self.max_ms);
        delay
    }
}

fn choose_actuator(
    actuators: &[Actuator],
    selected: Option<&HashSet<Actuator>>,
) -> Option<Actuator> {
    let mut actuators = actuators.to_vec();
    actuators.sort_by_key(|actuator| (actuator.kind, actuator.index));

    if let Some(selected) = selected
        && let Some(actuator) = actuators
            .iter()
            .find(|actuator| !selected.contains(actuator))
    {
        return Some(*actuator);
    }

    actuators.first().copied()
}

fn percent_decode(input: &str, device_id: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hi = *bytes.get(index + 1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid percent-encoding in device lookup '{input}' (device id '{device_id}')"
                    )
                })?;
                let lo = *bytes.get(index + 2).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid percent-encoding in device lookup '{input}' (device id '{device_id}')"
                    )
                })?;
                let hi = hex_value(hi, input, device_id)?;
                let lo = hex_value(lo, input, device_id)?;
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

fn hex_value(byte: u8, input: &str, device_id: &str) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!(
            "invalid hex digit '{}' in device lookup '{input}' (device id '{device_id}')",
            byte as char
        ),
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
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, mpsc as std_mpsc};
    use std::time::Duration;

    fn no_server_config() -> ButtplugConfig {
        ButtplugConfig {
            server: "ws://127.0.0.1:0".to_string(),
            connection_timeout_ms: 50,
            max_backoff_ms: 100,
            scan_interval_ms: 60_000,
        }
    }

    fn install_running_worker(
        backend: &ButtplugBackend,
        tx: mpsc::Sender<GestureJob>,
        worker: std::thread::JoinHandle<()>,
    ) {
        let (shutdown_tx, _shutdown_rx) = oneshot::channel();
        *backend.state.lock().unwrap() = WorkerState::Running {
            tx,
            shutdown_tx,
            worker,
        };
    }

    #[test]
    fn startup_and_teardown_are_safe_before_connection() {
        let backend = ButtplugBackend::new(no_server_config());
        assert!(backend.startup().is_ok());
        assert!(backend.teardown().is_ok());
    }

    #[test]
    fn teardown_returns_an_error_when_shutdown_times_out() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        let (tx, _rx) = mpsc::channel(1);
        let worker = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(700));
        });
        install_running_worker(&backend, tx, worker);

        let err = backend.teardown().unwrap_err().to_string();
        assert!(err.contains("shutdown"));
        assert!(err.contains("timed out"));
    }

    #[test]
    fn repeated_teardown_keeps_failing_until_worker_stops() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        let (tx, _rx) = mpsc::channel(1);
        let worker = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(700));
        });
        install_running_worker(&backend, tx, worker);

        let err = backend.teardown().unwrap_err().to_string();
        assert!(err.contains("shutdown"));
        assert!(err.contains("timed out"));

        let err = backend.teardown().unwrap_err().to_string();
        assert!(err.contains("shutdown still in progress"));

        std::thread::sleep(Duration::from_millis(300));
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
        let err = parse_device_id("broken%2").unwrap_err().to_string();
        assert!(err.contains("device lookup 'broken%2'"));
        assert!(err.contains("device id 'broken%2'"));
    }

    #[test]
    fn parse_device_id_rejects_bad_percent_encoding_with_invalid_hex_digit() {
        let err = parse_device_id("broken%2Z").unwrap_err().to_string();
        assert!(err.contains("device lookup 'broken%2Z'"));
        assert!(err.contains("invalid hex digit 'Z'"));
    }

    #[test]
    fn resolve_device_ids_prefers_unused_actuators_by_priority() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(
            DeviceLookup::ByIndex(7),
            0,
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
            0,
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
        backend.seed_device(DeviceLookup::ByIndex(3), 0, vec![]);

        let ids = vec!["Missing".to_string(), "3".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(resolved, ids);
    }

    #[test]
    fn resolve_device_ids_leaves_unknown_explicit_actuator_unresolved() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(
            DeviceLookup::ByIndex(7),
            0,
            vec![ActuatorKind::Vibrate, ActuatorKind::Linear],
        );

        let ids = vec!["7/9".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(resolved, ids);
    }

    #[test]
    fn resolve_device_ids_leaves_explicit_actuator_on_zero_actuator_device_unresolved() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.seed_device(DeviceLookup::ByIndex(3), 0, vec![]);

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
            0,
            vec![ActuatorKind::Vibrate, ActuatorKind::Linear],
        );

        let ids = vec!["7".to_string(), "Lovense%20Nora".to_string()];
        let resolved = backend.resolve_device_ids(&ids).unwrap();

        assert_eq!(
            resolved,
            vec!["7/0".to_string(), "Lovense%20Nora/0".to_string()]
        );
    }

    #[test]
    fn startup_is_atomic_under_concurrent_calls() {
        let backend = Arc::new(ButtplugBackend::new(no_server_config()));
        let (release_tx, release_rx) = std_mpsc::channel();
        let builder_calls = Arc::new(AtomicUsize::new(0));

        let first_backend = Arc::clone(&backend);
        let first_calls = Arc::clone(&builder_calls);
        let first = std::thread::spawn(move || {
            first_backend.startup_with_runtime(move || {
                first_calls.fetch_add(1, Ordering::SeqCst);
                release_rx
                    .recv()
                    .expect("release signal should arrive before startup exits");
                Ok(tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?)
            })
        });

        while builder_calls.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }

        let second_backend = Arc::clone(&backend);
        let second = std::thread::spawn(move || {
            second_backend.startup_with_runtime(|| {
                Err(anyhow::anyhow!("second startup should not build a runtime"))
            })
        });

        release_tx.send(()).unwrap();

        assert!(first.join().unwrap().is_ok());
        assert!(second.join().unwrap().is_ok());
        assert_eq!(builder_calls.load(Ordering::SeqCst), 1);

        backend.teardown().unwrap();
    }

    #[test]
    fn translate_rotate_event_uses_magnitude_as_speed() {
        let event = Event::new(80, 0.75, 1);
        assert_eq!(
            translate_event(ActuatorKind::Rotate, &event),
            Some(ButtplugCommand::Rotate { speed: 0.75 })
        );

        let event = Event::new(80, -0.75, 0);
        assert_eq!(
            translate_event(ActuatorKind::Rotate, &event),
            Some(ButtplugCommand::Rotate { speed: 0.75 })
        );
    }

    #[test]
    fn translate_linear_event_into_linear_command() {
        let event = Event::new(80, 0.75, 0);
        assert_eq!(
            translate_event(ActuatorKind::Linear, &event),
            Some(ButtplugCommand::Linear {
                position: 0.75,
                duration_ms: 80,
            })
        );
    }

    #[test]
    fn translate_vibrate_event_into_vibrate_command() {
        let event = Event::new(80, 0.50, 0);
        assert_eq!(
            translate_event(ActuatorKind::Vibrate, &event),
            Some(ButtplugCommand::Vibrate { magnitude: 0.50 })
        );
    }

    #[test]
    fn startup_processes_queued_events_and_teardown_stops_processing() {
        let backend = ButtplugBackend::new(no_server_config());
        backend.seed_device(DeviceLookup::ByIndex(0), 0, vec![ActuatorKind::Vibrate]);

        backend.startup().unwrap();
        assert_eq!(backend.telemetry().processed_jobs, 0);

        for _ in 0..16 {
            backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));
        }

        for _ in 0..100 {
            if backend.telemetry().processed_jobs == 16 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(backend.telemetry().processed_jobs, 16);

        backend.teardown().unwrap();
        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(backend.telemetry().processed_jobs, 16);
    }

    #[test]
    fn send_event_records_dropped_conditions() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.send_event("broken%2".to_string(), &Event::new(50, 1.0, 0));
        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));

        let telemetry = backend.telemetry();
        assert_eq!(telemetry.dropped_invalid_device, 1);
        assert_eq!(telemetry.dropped_unstarted, 1);
        assert_eq!(telemetry.dropped_full, 0);
    }

    #[test]
    fn send_event_records_queue_full() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        let (tx, _rx) = mpsc::channel(1);
        backend.install_test_sender(tx);

        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));
        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));

        let telemetry = backend.telemetry();
        assert_eq!(telemetry.dropped_full, 1);
        assert_eq!(telemetry.dropped_unstarted, 0);
        assert_eq!(telemetry.dropped_invalid_device, 0);
        backend.teardown().unwrap();
    }

    #[test]
    fn telemetry_snapshot_reports_runtime_drop_counts() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        backend.send_event("broken%2".to_string(), &Event::new(50, 1.0, 0));
        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));

        let telemetry = backend.telemetry_snapshot();
        assert_eq!(telemetry.dropped_invalid_device, 1);
        assert_eq!(telemetry.dropped_unstarted, 1);
        assert_eq!(telemetry.dropped_full, 0);
    }

    #[test]
    fn startup_failure_does_not_leave_backend_half_initialized() {
        let backend = ButtplugBackend::new(no_server_config());
        backend.seed_device(DeviceLookup::ByIndex(0), 0, vec![ActuatorKind::Vibrate]);

        let err = backend
            .startup_with_runtime(|| Err(anyhow::anyhow!("runtime build failed")))
            .unwrap_err();
        assert!(err.to_string().contains("runtime build failed"));
        assert!(matches!(
            *backend.state.lock().unwrap(),
            WorkerState::Stopped
        ));

        backend.startup().unwrap();
        backend.send_event("0".to_string(), &Event::new(50, 1.0, 0));

        for _ in 0..100 {
            if backend.processed_jobs.load(Ordering::SeqCst) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(backend.processed_jobs.load(Ordering::SeqCst), 1);
        backend.teardown().unwrap();
    }

    #[test]
    fn classify_output_types_prefers_vibrate() {
        assert_eq!(
            classify_output_types(&[OutputType::Vibrate, OutputType::Rotate]),
            Some(ActuatorKind::Vibrate)
        );
    }

    #[test]
    fn classify_output_types_falls_back_to_linear_then_rotate() {
        assert_eq!(
            classify_output_types(&[OutputType::HwPositionWithDuration]),
            Some(ActuatorKind::Linear)
        );
        assert_eq!(
            classify_output_types(&[OutputType::Rotate]),
            Some(ActuatorKind::Rotate)
        );
        assert_eq!(
            classify_output_types(&[OutputType::HwPositionWithDuration, OutputType::Rotate]),
            Some(ActuatorKind::Linear)
        );
    }

    #[test]
    fn classify_output_types_excludes_unsupported_outputs() {
        assert_eq!(
            classify_output_types(&[OutputType::Led, OutputType::Constrict]),
            None
        );
        assert_eq!(classify_output_types(&[]), None);
    }

    #[test]
    fn is_secure_websocket_detects_wss_scheme() {
        assert!(is_secure_websocket("wss://example.com:12345"));
        assert!(!is_secure_websocket("ws://example.com:12345"));
        assert!(!is_secure_websocket("ws://localhost:12345"));
    }

    #[test]
    fn backoff_doubles_up_to_max_then_holds() {
        let mut backoff = Backoff::new(8_000);
        assert_eq!(backoff.next(), Duration::from_millis(1_000));
        assert_eq!(backoff.next(), Duration::from_millis(2_000));
        assert_eq!(backoff.next(), Duration::from_millis(4_000));
        assert_eq!(backoff.next(), Duration::from_millis(8_000));
        assert_eq!(backoff.next(), Duration::from_millis(8_000));
    }

    #[test]
    fn backoff_reset_returns_to_initial_delay() {
        let mut backoff = Backoff::new(30_000);
        backoff.next();
        backoff.next();
        backoff.reset();
        assert_eq!(backoff.next(), Duration::from_millis(1_000));
    }

    #[test]
    fn backoff_max_below_initial_delay_is_clamped_to_max() {
        let mut backoff = Backoff::new(500);
        assert_eq!(backoff.next(), Duration::from_millis(500));
        assert_eq!(backoff.next(), Duration::from_millis(500));
    }

    #[test]
    fn telemetry_snapshot_starts_at_zero_for_new_counters() {
        let backend = ButtplugBackend::new(ButtplugConfig::default());
        let telemetry = backend.telemetry_snapshot();
        assert_eq!(telemetry.dropped_disconnected, 0);
        assert_eq!(telemetry.dropped_unknown_device, 0);
    }
}

#[cfg(test)]
#[path = "buttplug_mock_server.rs"]
mod buttplug_mock_server;
