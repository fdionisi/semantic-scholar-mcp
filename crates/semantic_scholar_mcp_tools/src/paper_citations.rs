use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cache::{Cache, Query};
use context_server::{Tool, ToolContent, ToolExecutor};
use embed::Embed;
use http_client::HttpClient;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::utils::{RateLimiter, make_request};

pub struct PaperCitationsTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl PaperCitationsTool {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        rate_limiter: Arc<RateLimiter>,
        cache: Arc<dyn Cache>,
        embed: Arc<dyn Embed>,
    ) -> Self {
        Self {
            http_client,
            rate_limiter,
            cache,
            embed,
        }
    }

    pub(crate) fn format_citations(&self, response: &Value) -> Result<String> {
        if response.get("error").is_some() {
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Ok(format!("Error: {}", message));
        }

        let offset = response.get("offset").and_then(|o| o.as_u64()).unwrap_or(0);
        let next_offset = response.get("next").and_then(|n| n.as_u64());

        if let Some(data) = response.get("data").and_then(|d| d.as_array()) {
            if data.is_empty() {
                return Ok(String::from("No citations found for this paper."));
            }

            let mut result = format!(
                "Found {} citing papers (offset: {}):\n\n",
                data.len(),
                offset
            );

            for (i, paper) in data.iter().enumerate() {
                let title = paper
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Unknown title");
                let paper_id = paper
                    .get("paperId")
                    .and_then(|p| p.as_str())
                    .unwrap_or("Unknown ID");

                result.push_str(&format!(
                    "{}. {} (ID: {})\n",
                    i + 1 + offset as usize,
                    title,
                    paper_id
                ));

                if let Some(is_influential) = paper.get("isInfluential").and_then(|i| i.as_bool()) {
                    if is_influential {
                        result.push_str("   [INFLUENTIAL CITATION]\n");
                    }
                }

                if let Some(authors) = paper.get("authors").and_then(|a| a.as_array()) {
                    let author_names: Vec<&str> = authors
                        .iter()
                        .filter_map(|author| author.get("name").and_then(|n| n.as_str()))
                        .collect();

                    if !author_names.is_empty() {
                        result.push_str(&format!("   Authors: {}\n", author_names.join(", ")));
                    }
                }

                if let Some(year) = paper.get("year").and_then(|y| y.as_u64()) {
                    result.push_str(&format!("   Year: {}\n", year));
                }

                if let Some(venue) = paper.get("venue").and_then(|v| v.as_str()) {
                    if !venue.is_empty() {
                        result.push_str(&format!("   Venue: {}\n", venue));
                    }
                }

                if let Some(citation_count) = paper.get("citationCount").and_then(|c| c.as_u64()) {
                    result.push_str(&format!("   Citations: {}\n", citation_count));
                }

                if let Some(contexts) = paper.get("contexts").and_then(|c| c.as_array()) {
                    if !contexts.is_empty() {
                        result.push_str("   Citation contexts:\n");

                        for (idx, context) in contexts.iter().take(3).enumerate() {
                            if let Some(text) = context.as_str() {
                                result.push_str(&format!("     {}. \"{}\"\n", idx + 1, text));
                            }
                        }

                        if contexts.len() > 3 {
                            result.push_str(&format!(
                                "     ... and {} more contexts\n",
                                contexts.len() - 3
                            ));
                        }
                    }
                }

                if let Some(intents) = paper.get("intents").and_then(|i| i.as_array()) {
                    let intent_types: Vec<&str> = intents
                        .iter()
                        .filter_map(|intent| intent.as_str())
                        .collect();

                    if !intent_types.is_empty() {
                        result.push_str(&format!(
                            "   Citation intents: {}\n",
                            intent_types.join(", ")
                        ));
                    }
                }

                if let Some(url) = paper.get("url").and_then(|u| u.as_str()) {
                    result.push_str(&format!("   URL: {}\n", url));
                }

                if i < data.len() - 1 {
                    result.push_str("\n");
                }
            }

            if let Some(next) = next_offset {
                result.push_str(&format!("\nFor more citations, use offset={}", next));
            }

            Ok(result)
        } else {
            Ok(String::from(
                "No citations found or unexpected API response format.",
            ))
        }
    }
}

#[async_trait]
impl ToolExecutor for PaperCitationsTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing PaperCitationsTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let paper_id = args
            .get("paper_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid paper_id parameter"))?;

        if paper_id.trim().is_empty() {
            return Err(anyhow!("Paper ID cannot be empty"));
        }

        let query_text = format!("paper_citations:{}", paper_id);

        // Generate embedding for the query
        let embedding = self.embed.embed(&query_text).await?;

        // Try to find similar queries in cache
        let similar_queries = self.cache.search_similarity(&embedding)?;

        if !similar_queries.is_empty() {
            let (cached_query, similarity) = &similar_queries[0];
            // If we have a very similar query, use the cached result
            if *similarity > 0.95 {
                log::debug!("Using cached result with similarity {}", similarity);
                let formatted_result = self.format_citations(&cached_query.results)?;
                return Ok(vec![ToolContent::Text {
                    text: formatted_result,
                }]);
            }
        }

        let fields = args.get("fields").cloned();

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100);

        if limit > 1000 {
            return Err(anyhow!("Limit cannot exceed 1000"));
        }

        let mut params_map = serde_json::Map::new();
        params_map.insert("offset".to_string(), json!(offset));
        params_map.insert("limit".to_string(), json!(limit));

        if let Some(f) = fields {
            params_map.insert("fields".to_string(), f);
        }

        let params = Value::Object(params_map);

        let result = make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/paper/{}/citations", paper_id),
            Some(&params),
            None,
        )
        .await?;

        // Store the query and result in cache
        let cache_entry = Query {
            text: query_text,
            embedding,
            results: result.clone(),
        };

        if let Err(e) = self.cache.store(cache_entry) {
            log::warn!("Failed to store query in cache: {}", e);
        }

        let formatted_result = self.format_citations(&result)?;

        Ok(vec![ToolContent::Text {
            text: formatted_result,
        }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "paper_citations".into(),
            description: Some("Get papers that cite a specific paper in Semantic Scholar".into()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "paper_id": {
                        "type": "string",
                        "description": "Paper identifier in one of the following formats: Semantic Scholar ID, DOI:doi, ARXIV:id, MAG:id, ACL:id, PMID:id, PMCID:id, URL:url"
                    },
                    "fields": {
                        "type": "array",
                        "description": "List of fields to return for each citing paper. Default: paperId and title",
                        "items": {
                            "type": "string",
                            "enum": [
                                "title", "abstract", "year", "venue", "authors", "url", "paperId",
                                "citationCount", "influentialCitationCount", "contexts", "intents", "isInfluential"
                            ]
                        }
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Number of citations to skip for pagination. Default: 0"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of citations to return. Default: 100, Maximum: 1000"
                    }
                },
                "required": ["paper_id"]
            }),
        }
    }
}
