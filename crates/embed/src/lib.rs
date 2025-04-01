use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait Embed: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}
