use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct CloudEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub source: String,
    pub sourcetype: String,
    pub subject: Option<String>,
    pub data: serde_json::Value,
}
