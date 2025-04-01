use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cache::{Cache, Query};
use context_server::{Tool, ToolContent, ToolExecutor};
use embed::Embed;
use http_client::HttpClient;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::utils::{RateLimiter, make_request};

pub struct AuthorSearchTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl AuthorSearchTool {
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

    fn format_author_search(&self, response: &Value) -> Result<String> {
        if response.get("error").is_some() {
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Ok(format!("Error: {}", message));
        }

        let total = response.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
        let offset = response.get("offset").and_then(|o| o.as_u64()).unwrap_or(0);
        let next_offset = response.get("next").and_then(|n| n.as_u64());

        if let Some(data) = response.get("data").and_then(|d| d.as_array()) {
            if data.is_empty() {
                return Ok(String::from("No authors found matching your query."));
            }

            let mut result = format!(
                "Found {} total authors matching your query. Showing results {}-{}:\n\n",
                total,
                offset + 1,
                offset + data.len() as u64
            );

            for (i, author) in data.iter().enumerate() {
                let name = author
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("Unknown name");
                let author_id = author
                    .get("authorId")
                    .and_then(|id| id.as_str())
                    .unwrap_or("Unknown ID");

                result.push_str(&format!(
                    "{}. {} (ID: {})\n",
                    i + 1 + offset as usize,
                    name,
                    author_id
                ));

                if let Some(affiliations) = author.get("affiliations").and_then(|a| a.as_array()) {
                    let affiliation_list: Vec<&str> =
                        affiliations.iter().filter_map(|aff| aff.as_str()).collect();

                    if !affiliation_list.is_empty() {
                        result.push_str(&format!(
                            "   Affiliations: {}\n",
                            affiliation_list.join(", ")
                        ));
                    }
                }

                if let Some(aliases) = author.get("aliases").and_then(|a| a.as_array()) {
                    let alias_list: Vec<&str> =
                        aliases.iter().filter_map(|alias| alias.as_str()).collect();

                    if !alias_list.is_empty() {
                        result.push_str(&format!("   Also known as: {}\n", alias_list.join(", ")));
                    }
                }

                if let Some(paper_count) = author.get("paperCount").and_then(|p| p.as_u64()) {
                    result.push_str(&format!("   Papers: {}\n", paper_count));
                }

                if let Some(citation_count) = author.get("citationCount").and_then(|c| c.as_u64()) {
                    result.push_str(&format!("   Citations: {}\n", citation_count));
                }

                if let Some(h_index) = author.get("hIndex").and_then(|h| h.as_u64()) {
                    result.push_str(&format!("   h-index: {}\n", h_index));
                }

                if let Some(homepage) = author.get("homepage").and_then(|h| h.as_str()) {
                    if !homepage.is_empty() {
                        result.push_str(&format!("   Homepage: {}\n", homepage));
                    }
                }

                if let Some(url) = author.get("url").and_then(|u| u.as_str()) {
                    result.push_str(&format!("   Semantic Scholar URL: {}\n", url));
                }

                if let Some(papers) = author.get("papers").and_then(|p| p.as_array()) {
                    if !papers.is_empty() {
                        result.push_str(&format!(
                            "   Representative papers (showing up to 3 of {}):\n",
                            papers.len()
                        ));

                        for (pi, paper) in papers.iter().take(3).enumerate() {
                            let paper_title = paper
                                .get("title")
                                .and_then(|t| t.as_str())
                                .unwrap_or("Unknown title");
                            let paper_id = paper
                                .get("paperId")
                                .and_then(|id| id.as_str())
                                .unwrap_or("Unknown ID");

                            result.push_str(&format!(
                                "     {}. {} (ID: {})\n",
                                pi + 1,
                                paper_title,
                                paper_id
                            ));

                            if let Some(year) = paper.get("year").and_then(|y| y.as_u64()) {
                                result.push_str(&format!("        Year: {}\n", year));
                            }

                            if let Some(venue) = paper.get("venue").and_then(|v| v.as_str()) {
                                if !venue.is_empty() {
                                    result.push_str(&format!("        Venue: {}\n", venue));
                                }
                            }
                        }

                        if papers.len() > 3 {
                            result.push_str(&format!(
                                "     ... and {} more papers\n",
                                papers.len() - 3
                            ));
                        }
                    }
                }

                if i < data.len() - 1 {
                    result.push_str("\n");
                }
            }

            if let Some(next) = next_offset {
                result.push_str(&format!("\nFor more authors, use offset={}", next));
            }

            Ok(result)
        } else {
            Ok(String::from(
                "No authors found or unexpected API response format.",
            ))
        }
    }
}

#[async_trait]
impl ToolExecutor for AuthorSearchTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing AuthorSearchTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid query parameter"))?;

        if query.trim().is_empty() {
            return Err(anyhow!("Query string cannot be empty"));
        }

        let fields = args.get("fields").cloned();
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100);

        if limit > 1000 {
            return Err(anyhow!("Limit cannot exceed 1000"));
        }

        let mut params_map = serde_json::Map::new();
        params_map.insert("query".to_string(), json!(query));
        params_map.insert("offset".to_string(), json!(offset));
        params_map.insert("limit".to_string(), json!(limit));

        if let Some(f) = fields {
            params_map.insert("fields".to_string(), f);
        }

        let params = Value::Object(params_map);

        // Generate an embedding for the query
        let embedding = self.embed.embed(&query).await?;

        // Check if we have a cached result for a similar query
        let similar_queries = self.cache.search_similarity(&embedding)?;

        // Check for any cached queries with high similarity and matching action/params
        for (cached_query, similarity) in similar_queries.iter() {
            if similarity > &0.95 && cached_query.action == "author_search" {
                // Check if parameters match
                if cached_query.params == Some(params.clone()) {
                    log::debug!("Found cached result with similarity {}", similarity);
                    return Ok(vec![ToolContent::Text {
                        text: serde_json::from_value(cached_query.results.clone())?,
                    }]);
                }
            }
        }

        let result = make_request(
            &self.http_client,
            &self.rate_limiter,
            "/author/search",
            Some(&params),
            None,
        )
        .await?;

        let formatted_result = self.format_author_search(&result)?;

        let query = Query {
            action: "author_search".into(),
            text: query.into(),
            embedding,
            params: Some(params),
            results: json!(formatted_result),
        };

        if let Err(err) = self.cache.store(query) {
            log::warn!("Failed to store query in cache: {}", err);
        }

        Ok(vec![ToolContent::Text {
            text: formatted_result,
        }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "author_search".into(),
            description: Some("Search for authors by name on Semantic Scholar".into()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The name text to search for. The query will be matched against author names and their known aliases."
                    },
                    "fields": {
                        "type": "array",
                        "description": "List of fields to return for each author. Default: name and authorId",
                        "items": {
                            "type": "string",
                            "enum": [
                                "name", "aliases", "url", "authorId", "affiliations", "homepage",
                                "paperCount", "citationCount", "hIndex", "papers", "papers.year",
                                "papers.authors", "papers.abstract", "papers.venue", "papers.citations"
                            ]
                        }
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Number of authors to skip for pagination. Default: 0"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of authors to return. Default: 100, Maximum: 1000"
                    }
                },
                "required": ["query"]
            }),
        }
    }
}
