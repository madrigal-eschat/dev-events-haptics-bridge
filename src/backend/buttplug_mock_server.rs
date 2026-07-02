//! Minimal Buttplug JSON-over-WebSocket mock server, test-only.
//!
//! Speaks just enough of the protocol (RequestServerInfo, RequestDeviceList,
//! StartScanning, OutputCmd) to drive `ButtplugBackend`'s connection manager
//! end-to-end without a real Intiface server. Responses are built from real
//! `buttplug_core` message structs and serialized with serde_json, rather
//! than hand-written JSON, so the wire format always matches what the
//! client's schema validator expects.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use buttplug_core::message::{
    ButtplugClientMessageV4, ButtplugMessage, ButtplugMessageSpecVersion, ButtplugServerMessageV4,
    DeviceFeature, DeviceFeatureOutput, DeviceFeatureOutputValueProperties, DeviceListV4,
    DeviceMessageInfoV4, OkV0, OutputCommand, ServerInfoV4,
};
use buttplug_core::util::range::RangeInclusive;
use buttplug_core::util::small_vec_enum_map::SmallVecEnumMap;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

pub struct MockButtplugServer {
    pub addr: SocketAddr,
    pub output_cmds: mpsc::UnboundedReceiver<OutputCommand>,
    task: JoinHandle<()>,
}

impl MockButtplugServer {
    pub fn stop(self) {
        self.task.abort();
    }
}

fn fake_vibrator_device_list() -> DeviceListV4 {
    let output = SmallVecEnumMap::from(vec![DeviceFeatureOutput::Vibrate(
        DeviceFeatureOutputValueProperties::new(RangeInclusive::new(0, 20)),
    )]);
    let mut features: BTreeMap<u32, DeviceFeature> = BTreeMap::new();
    features.insert(
        0,
        DeviceFeature::new(0, "Vibrator", &output, &SmallVecEnumMap::from(Vec::new())),
    );
    let info = DeviceMessageInfoV4::new(0, "Test Vibrator", &None, 0, &features);
    DeviceListV4::new(vec![info])
}

pub async fn start() -> MockButtplugServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    start_with_listener(listener).await
}

pub async fn start_on(addr: SocketAddr) -> MockButtplugServer {
    let listener = TcpListener::bind(addr)
        .await
        .expect("bind mock server on fixed addr");
    start_with_listener(listener).await
}

async fn start_with_listener(listener: TcpListener) -> MockButtplugServer {
    let addr = listener.local_addr().expect("mock server local addr");
    let (output_tx, output_rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let Ok(ws) = tokio_tungstenite::accept_async(stream).await else {
            return;
        };
        let (mut write, mut read) = ws.split();

        while let Some(Ok(Message::Text(text))) = read.next().await {
            let Ok(requests) = serde_json::from_str::<Vec<ButtplugClientMessageV4>>(&text) else {
                continue;
            };

            let mut responses: Vec<ButtplugServerMessageV4> = Vec::new();
            for request in requests {
                let id = request.id();
                match request {
                    ButtplugClientMessageV4::RequestServerInfo(_) => {
                        responses.push(ButtplugServerMessageV4::ServerInfo(ServerInfoV4::new(
                            "Mock Buttplug Server",
                            ButtplugMessageSpecVersion::Version4,
                            0,
                            0,
                        )));
                    }
                    ButtplugClientMessageV4::RequestDeviceList(_) => {
                        let mut list = fake_vibrator_device_list();
                        list.set_id(id);
                        responses.push(ButtplugServerMessageV4::DeviceList(list));
                    }
                    ButtplugClientMessageV4::OutputCmd(cmd) => {
                        let _ = output_tx.send(cmd.command());
                        responses.push(ButtplugServerMessageV4::Ok(OkV0::new(id)));
                    }
                    _ => {
                        responses.push(ButtplugServerMessageV4::Ok(OkV0::new(id)));
                    }
                }
            }

            let Ok(json) = serde_json::to_string(&responses) else {
                continue;
            };
            if write.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    });

    MockButtplugServer {
        addr,
        output_cmds: output_rx,
        task,
    }
}
