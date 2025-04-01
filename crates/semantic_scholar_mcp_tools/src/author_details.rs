use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cache::{Cache, Query};
use context_server::{Tool, ToolContent, ToolExecutor};
use embed::Embed;
use http_client::HttpClient;
use serde_json::{Value, json};
use std::sync::Arc;

use crate::{RateLimiter, utils::make_request};

pub struct AuthorDetailsTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl AuthorDetailsTool {
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

    pub(crate) fn format_author_details(&self, response: &Value) -> Result<String> {
        if response.get("error").is_some() {
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Ok(format!("Error: {}", message));
        }

        let name = response
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Unknown name");
        let author_id = response
            .get("authorId")
            .and_then(|id| id.as_str())
            .unwrap_or("Unknown ID");

        let mut result = format!("Author: {}\n", name);
        result.push_str(&format!("Author ID: {}\n\n", author_id));

        if let Some(affiliations) = response.get("affiliations").and_then(|a| a.as_array()) {
            let affiliation_list: Vec<&str> =
                affiliations.iter().filter_map(|aff| aff.as_str()).collect();

            if !affiliation_list.is_empty() {
                result.push_str("Affiliations:\n");
                for aff in affiliation_list {
                    result.push_str(&format!("- {}\n", aff));
                }
                result.push_str("\n");
            }
        }

        if let Some(aliases) = response.get("aliases").and_then(|a| a.as_array()) {
            let alias_list: Vec<&str> = aliases.iter().filter_map(|alias| alias.as_str()).collect();

            if !alias_list.is_empty() {
                result.push_str("Also known as:\n");
                for alias in alias_list {
                    result.push_str(&format!("- {}\n", alias));
                }
                result.push_str("\n");
            }
        }

        result.push_str("Research Metrics:\n");

        if let Some(paper_count) = response.get("paperCount").and_then(|p| p.as_u64()) {
            result.push_str(&format!("- Papers: {}\n", paper_count));
        }

        if let Some(citation_count) = response.get("citationCount").and_then(|c| c.as_u64()) {
            result.push_str(&format!("- Citations: {}\n", citation_count));
        }

        if let Some(h_index) = response.get("hIndex").and_then(|h| h.as_u64()) {
            result.push_str(&format!("- h-index: {}\n", h_index));
        }

        result.push_str("\n");

        if let Some(homepage) = response.get("homepage").and_then(|h| h.as_str()) {
            if !homepage.is_empty() {
                result.push_str(&format!("Homepage: {}\n", homepage));
            }
        }

        if let Some(url) = response.get("url").and_then(|u| u.as_str()) {
            result.push_str(&format!("Semantic Scholar URL: {}\n\n", url));
        }

        if let Some(papers) = response.get("papers").and_then(|p| p.as_array()) {
            if !papers.is_empty() {
                result.push_str(&format!(
                    "Representative Papers (showing up to 10 of {}):\n\n",
                    papers.len()
                ));

                for (i, paper) in papers.iter().take(10).enumerate() {
                    let paper_title = paper
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("Unknown title");
                    let paper_id = paper
                        .get("paperId")
                        .and_then(|id| id.as_str())
                        .unwrap_or("Unknown ID");

                    result.push_str(&format!("{}. {} (ID: {})\n", i + 1, paper_title, paper_id));

                    if let Some(year) = paper.get("year").and_then(|y| y.as_u64()) {
                        result.push_str(&format!("   Year: {}\n", year));
                    }

                    if let Some(venue) = paper.get("venue").and_then(|v| v.as_str()) {
                        if !venue.is_empty() {
                            result.push_str(&format!("   Venue: {}\n", venue));
                        }
                    }

                    if let Some(citation_count) =
                        paper.get("citationCount").and_then(|c| c.as_u64())
                    {
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

                    if i < papers.len().min(10) - 1 {
                        result.push_str("\n");
                    }
                }

                if papers.len() > 10 {
                    result.push_str(&format!("\n... and {} more papers\n", papers.len() - 10));
                    result
                        .push_str("Use the author_papers tool to see all papers by this author.\n");
                }
            }
        } else {
            result.push_str("Paper information not included in the response. Use the 'fields' parameter to include 'papers'.\n");
            result.push_str(
                "Alternatively, use the author_papers tool to get all papers by this author.\n",
            );
        }

        Ok(result)
    }
}

#[async_trait]
impl ToolExecutor for AuthorDetailsTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing AuthorDetailsTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let author_id = args
            .get("author_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid author_id parameter"))?;

        if author_id.trim().is_empty() {
            return Err(anyhow!("Author ID cannot be empty"));
        }

        // Create a query string that uniquely identifies this request
        let query_text = format!("author_details:{}", author_id);

        // Generate an embedding for the query
        let embedding = self.embed.embed(&query_text).await?;

        // Check if we have a cached result for a similar query
        let similar_queries = self.cache.search_similarity(&embedding)?;

        // If we found similar queries in the cache, use the most similar one
        if let Some((cached_query, similarity)) = similar_queries.first() {
            if similarity > &0.95 {
                log::debug!("Found cached result with similarity {}", similarity);
                return Ok(vec![ToolContent::Text {
                    text: serde_json::from_value(cached_query.results.clone())?,
                }]);
            }
        }

        let fields = args.get("fields").cloned();

        let params = match fields {
            Some(fields_value) => json!({"fields": fields_value}),
            None => json!({}),
        };

        let result = make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/author/{}", author_id),
            Some(&params),
            None,
        )
        .await?;

        let formatted_result = self.format_author_details(&result)?;

        // Store the result in the cache
        let query = Query {
            text: query_text,
            embedding,
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
            name: "author_details".into(),
            description: Some(
                "Get detailed information about an author by their ID in Semantic Scholar".into(),
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
                        "description": "List of fields to return. Default: name and affiliations",
                        "items": {
                            "type": "string",
                            "enum": [
                                "name", "aliases", "url", "authorId", "affiliations", "homepage",
                                "paperCount", "citationCount", "hIndex", "papers", "papers.year",
                                "papers.authors", "papers.abstract", "papers.venue", "papers.citations"
                            ]
                        }
                    }
                },
                "required": ["author_id"]
            }),
        }
    }
}
