use std::{env, path::PathBuf, sync::Arc};

use anyhow::{Result, anyhow};
use context_server::{ContextServer, ContextServerRpcRequest, ContextServerRpcResponse};
use context_server_utils::{
    prompt_registry::PromptRegistry, resource_registry::ResourceRegistry,
    tool_registry::ToolRegistry,
};
use directories::ProjectDirs;
use http_client::HttpClient;
use http_client_reqwest::HttpClientReqwest;
use local_cache::LocalCache;
use ollama_embed::OllamaEmbed;
use semantic_scholar_mcp_tools::{
    AuthorDetailsTool, AuthorPapersTool, AuthorSearchTool, PaperCitationsTool, PaperDetailsTool,
    PaperRecommendationMultiTool, PaperRecommendationSingleTool, PaperReferencesTool,
    PaperSearchTool, RateLimiter,
};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

struct ContextServerState {
    rpc: ContextServer,
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("code", "fdionisi", "semantic-scholar-mcp")
        .ok_or_else(|| anyhow!("unable to find project directory"))
}

fn database_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("cache.db"))
}

impl ContextServerState {
    fn new(http_client: Arc<dyn HttpClient>) -> Result<Self> {
        let resource_registry = Arc::new(ResourceRegistry::default());

        let tool_registry = Arc::new(ToolRegistry::default());

        let rate_limiter = Arc::new(RateLimiter::new());
        let local_cache = Arc::new(LocalCache::new(database_dir()?, None)?);
        let ollama_embed = Arc::new(
            OllamaEmbed::builder()
                .with_http_client(http_client.clone())
                .build(),
        );
        tool_registry.register(Arc::new(AuthorDetailsTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(AuthorPapersTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(AuthorSearchTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(PaperSearchTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(PaperDetailsTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(PaperCitationsTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(PaperReferencesTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(PaperRecommendationSingleTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));
        tool_registry.register(Arc::new(PaperRecommendationMultiTool::new(
            http_client.clone(),
            rate_limiter.clone(),
            local_cache.clone(),
            ollama_embed.clone(),
        )));

        let prompt_registry = Arc::new(PromptRegistry::default());

        Ok(Self {
            rpc: ContextServer::builder()
                .with_server_info((env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")))
                .with_resources(resource_registry)
                .with_tools(tool_registry)
                .with_prompts(prompt_registry)
                .build()?,
        })
    }

    async fn process_request(
        &self,
        request: ContextServerRpcRequest,
    ) -> Result<Option<ContextServerRpcResponse>> {
        self.rpc.handle_incoming_message(request).await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let http_client = Arc::new(HttpClientReqwest::default());

    if env::var("SEMANTIC_SCHOLAR_API_KEY").is_err() {
        eprintln!("SEMANTIC_SCHOLAR_API_KEY environment variable is not defined");
    }

    let state = ContextServerState::new(http_client)?;

    let mut stdin = BufReader::new(io::stdin()).lines();
    let mut stdout = io::stdout();

    while let Some(line) = stdin.next_line().await? {
        let request: ContextServerRpcRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                eprintln!("Error parsing request: {}", e);
                continue;
            }
        };

        if let Some(response) = state.process_request(request).await? {
            let response_json = serde_json::to_string(&response)?;
            stdout.write_all(response_json.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    Ok(())
}
