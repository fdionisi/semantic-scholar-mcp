use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cache::{Cache, Query};
use context_server::{Tool, ToolContent, ToolExecutor};
use embed::Embed;
use http_client::HttpClient;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::{RateLimiter, utils::make_request};

pub struct AuthorPapersTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl AuthorPapersTool {
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

    pub(crate) fn format_author_papers(&self, response: &Value) -> Result<String> {
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
                return Ok(String::from("No papers found for this author."));
            }

            let mut result = format!(
                "Found {} papers by this author (offset: {}):\n\n",
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

                if let Some(authors) = paper.get("authors").and_then(|a| a.as_array()) {
                    let author_names: Vec<&str> = authors
                        .iter()
                        .filter_map(|author| author.get("name").and_then(|n| n.as_str()))
                        .collect();

                    if !author_names.is_empty() {
                        result.push_str(&format!("   Authors: {}\n", author_names.join(", ")));
                    }
                }

                if let Some(abstract_text) = paper.get("abstract").and_then(|a| a.as_str()) {
                    if !abstract_text.is_empty() {
                        let summary = if abstract_text.len() > 200 {
                            format!("{}...", &abstract_text[0..200])
                        } else {
                            abstract_text.to_string()
                        };
                        result.push_str(&format!("   Abstract: {}\n", summary));
                    }
                }

                if let Some(url) = paper.get("url").and_then(|u| u.as_str()) {
                    result.push_str(&format!("   URL: {}\n", url));
                }

                if let Some(is_open_access) = paper.get("isOpenAccess").and_then(|o| o.as_bool()) {
                    if is_open_access {
                        if let Some(pdf) = paper.get("openAccessPdf") {
                            if let Some(pdf_url) = pdf.get("url").and_then(|u| u.as_str()) {
                                result.push_str(&format!("   Open Access PDF: {}\n", pdf_url));
                            }
                        }
                    }
                }

                if i < data.len() - 1 {
                    result.push_str("\n");
                }
            }

            if let Some(next) = next_offset {
                result.push_str(&format!("\nFor more papers, use offset={}", next));
            }

            Ok(result)
        } else {
            Ok(String::from(
                "No papers found or unexpected API response format.",
            ))
        }
    }
}

#[async_trait]
impl ToolExecutor for AuthorPapersTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing AuthorPapersTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let author_id = args
            .get("author_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid author_id parameter"))?;

        if author_id.trim().is_empty() {
            return Err(anyhow!("Author ID cannot be empty"));
        }

        let fields = args.get("fields").cloned();

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100);

        if limit > 1000 {
            return Err(anyhow!("Limit cannot exceed 1000"));
        }

        // Create a query string to use for embedding and cache lookup
        let query_string = format!(
            "author_papers:{} offset:{} limit:{} fields:{:?}",
            author_id, offset, limit, fields
        );

        // Generate embedding for the query
        let embedding = self.embed.embed(&query_string).await?;

        // Try to find similar queries in cache
        let similar_queries = self.cache.search_similarity(&embedding)?;

        let result = if !similar_queries.is_empty() && similar_queries[0].1 > 0.95 {
            // Use cached result if similarity is very high
            log::debug!("Using cached result for author papers query");
            similar_queries[0].0.results.clone()
        } else {
            // Otherwise make the API request
            let mut params_map = serde_json::Map::new();
            params_map.insert("offset".to_string(), json!(offset));
            params_map.insert("limit".to_string(), json!(limit));

            if let Some(f) = fields {
                params_map.insert("fields".to_string(), f);
            }

            let params = Value::Object(params_map);

            let api_result = make_request(
                &self.http_client,
                &self.rate_limiter,
                &format!("/author/{}/papers", author_id),
                Some(&params),
                None,
            )
            .await?;

            // Store the result in cache
            let query = Query {
                text: query_string,
                embedding,
                results: api_result.clone(),
            };

            if let Err(e) = self.cache.store(query) {
                log::warn!("Failed to store query in cache: {}", e);
            }

            api_result
        };

        let formatted_result = self.format_author_papers(&result)?;

        Ok(vec![ToolContent::Text {
            text: formatted_result,
        }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "author_papers".into(),
            description: Some(
                "Get papers written by a specific author in Semantic Scholar with pagination support"
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "author_id": {
                        "type": "string",
                        "description": "Semantic Scholar author ID"
                    },
                    "fields": {
                        "type": "array",
                        "description": "List of fields to return for each paper. Default: title and year",
                        "items": {
                            "type": "string",
                            "enum": [
                                "title", "abstract", "year", "venue", "authors", "url", "paperId",
                                "citationCount", "influentialCitationCount", "isOpenAccess",
                                "openAccessPdf", "fieldsOfStudy", "s2FieldsOfStudy",
                                "publicationTypes", "publicationDate", "journal", "externalIds"
                            ]
                        }
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Number of papers to skip for pagination. Default: 0"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of papers to return. Default: 100, Maximum: 1000"
                    }
                },
                "required": ["author_id"]
            }),
        }
    }
}
