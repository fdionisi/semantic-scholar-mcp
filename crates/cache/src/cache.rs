use anyhow::Result;
use chrono::NaiveDateTime;
use serde_json::Value;

#[derive(serde::Deserialize, serde::Serialize)]
pub struct CacheEntry<T> {
    pub value: T,
    pub created_at: NaiveDateTime,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Query {
    pub action: String,
    pub text: String,
    pub params: Option<Value>,
    pub embedding: Vec<f32>,
    pub results: Value,
}

pub trait Cache: Send + Sync {
    fn store(&self, query: Query) -> Result<()>;
    fn search_similarity(&self, query: &[f32]) -> Result<Vec<(Query, f32)>>;
}
