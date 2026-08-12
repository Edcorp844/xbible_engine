use serde::{Deserialize, Serialize};
use uniffi::Record;

#[derive(Debug, Serialize, Deserialize, Clone, Record)]
pub struct Event {
    pub id: u64,
    pub title: String,
    pub image: Option<String>,
    pub slug: String,
    pub start: i64,
    pub end: i64,
    pub row: u32,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approx: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_fixed: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Record)]
pub struct Period {
    pub id: String,
    pub image: String,
    pub description: String,
    pub start_year: i64,
    pub end_year: i64,
    pub interval: u32,
    pub title: String,
    pub section_title: String,
    pub sub_title: String,
    pub color: String,
    pub events: Vec<Event>,
}
