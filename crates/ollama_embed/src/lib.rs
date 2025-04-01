use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use embed::Embed;
use http_client::{HttpClient, http::Uri};
use ollama::{EmbedInput, EmbedRequest, Ollama, OllamaBuilder};

pub struct OllamaEmbed(Ollama);

pub struct OllamaEmbedBuilder(OllamaBuilder);

impl OllamaEmbed {
    pub fn builder() -> OllamaEmbedBuilder {
        OllamaEmbedBuilder(Ollama::builder())
    }
}

impl OllamaEmbedBuilder {
    pub fn with_http_client(&mut self, http_client: Arc<dyn HttpClient>) -> &mut Self {
        self.0.with_http_client(http_client);
        self
    }

    pub fn with_uri<U: Into<Uri>>(&mut self, uri: U) -> &mut Self {
        self.0.with_uri(uri);
        self
    }

    pub fn build(&self) -> OllamaEmbed {
        OllamaEmbed(self.0.build())
    }
}

#[async_trait]
impl Embed for OllamaEmbed {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.0
            .embed(EmbedRequest {
                model: "nomic-embed-text:latest".into(),
                input: EmbedInput::Single(text.into()),
                truncate: Some(false),
                options: None,
                keep_alive: None,
            })
            .await
            .map(|result| result.embeddings[0].to_owned())
    }
}
