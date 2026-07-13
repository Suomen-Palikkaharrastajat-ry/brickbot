use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModerationPayload {
    pub id: String,
    pub title: String,
    pub start_date: String,
    pub end_date: String,
    pub location: String,
    pub url: String,
    pub tags: Vec<String>,
    pub description: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}
