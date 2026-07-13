pub mod api;
pub mod handler;
pub mod listener;

use serde::Deserialize;
use serde_json::Value;
use serenity::all::Http as DiscordHttp;
use sqlx::SqlitePool;
use std::sync::Arc;

use crate::config::Config;

#[derive(serde::Deserialize)]
pub struct SendMessageResponse {
    pub id: i64,
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    pub http: Arc<dyn crate::http::HttpProvider>,
    pub discord: Arc<DiscordHttp>,
}

#[derive(Deserialize, Debug)]
pub struct ZulipRegisterResponse {
    pub result: String,
    pub msg: Option<String>,
    pub queue_id: Option<String>,
    pub last_event_id: Option<i64>,
}

#[derive(Deserialize, Debug)]
pub struct ZulipEventsResponse {
    pub events: Option<Vec<ZulipEvent>>,
}

#[derive(Deserialize, Debug)]
pub struct ZulipEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub message: Option<ZulipMessage>,
    pub id: i64,
}

#[derive(Deserialize, Debug)]
pub struct ZulipMessage {
    pub id: i64,
    pub sender_email: String,
    pub content: String,
    pub subject: String, // Topic name
    pub display_recipient: Value,
}

pub use api::*;
pub use handler::*;
pub use listener::*;
