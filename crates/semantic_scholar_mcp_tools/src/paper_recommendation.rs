use anyhow::Result;
use anyhow::anyhow;
use async_trait::async_trait;
use cache::Cache;
use cache::Query;
use context_server::Tool;
use context_server::ToolContent;
use context_server::ToolExecutor;
use embed::Embed;
use http_client::HttpClient;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;

use crate::utils::RateLimiter;
use crate::utils::make_request;

pub struct PaperRecommendationSingleTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl PaperRecommendationSingleTool {
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

    fn format_recommendations(&self, response: &Value) -> Result<String> {
        if response.get("error").is_some() {
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Ok(format!("Error: {}", message));
        }

        if let Some(recommended_papers) =
            response.get("recommendedPapers").and_then(|r| r.as_array())
        {
            if recommended_papers.is_empty() {
                return Ok(String::from("No recommendations found for this paper."));
            }

            let mut result = format!("Found {} recommended papers:\n\n", recommended_papers.len());

            for (i, paper) in recommended_papers.iter().enumerate() {
                let title = paper
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Unknown title");
                let paper_id = paper
                    .get("paperId")
                    .and_then(|p| p.as_str())
                    .unwrap_or("Unknown ID");

                result.push_str(&format!("{}. {} (ID: {})\n", i + 1, title, paper_id));

                if let Some(year) = paper.get("year").and_then(|y| y.as_u64()) {
                    result.push_str(&format!("   Year: {}\n", year));
                }

                if let Some(venue) = paper.get("venue").and_then(|v| v.as_str()) {
                    if !venue.is_empty() {
                        result.push_str(&format!("   Venue: {}\n", venue));
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

                if let Some(citation_count) = paper.get("citationCount").and_then(|c| c.as_u64()) {
                    result.push_str(&format!("   Citations: {}\n", citation_count));
                }

                if let Some(abstract_text) = paper.get("abstract").and_then(|a| a.as_str()) {
                    if !abstract_text.is_empty() {
                        let summary = abstract_text.to_string();
                        result.push_str(&format!("   Abstract: {}\n", summary));
                    }
                }

                if let Some(url) = paper.get("url").and_then(|u| u.as_str()) {
                    result.push_str(&format!("   URL: {}\n", url));
                }

                if i < recommended_papers.len() - 1 {
                    result.push_str("\n");
                }
            }

            result.push_str("\nNote: To get more detailed information about each paper, use the 'fields' parameter.");

            Ok(result)
        } else {
            Ok(String::from(
                "No recommendations found or unexpected API response format.",
            ))
        }
    }
}

#[async_trait]
impl ToolExecutor for PaperRecommendationSingleTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing PaperRecommendationSingleTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let paper_id = args
            .get("paper_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid paper_id parameter"))?;

        if paper_id.trim().is_empty() {
            return Err(anyhow!("Paper ID cannot be empty"));
        }

        let fields = args
            .get("fields")
            .and_then(|f| f.as_str())
            .unwrap_or("title,year,authors");

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100);

        let from_pool = args
            .get("from_pool")
            .and_then(|v| v.as_str())
            .unwrap_or("recent");

        if limit > 500 {
            return Err(anyhow!("Limit cannot exceed 500"));
        }

        if from_pool != "recent" && from_pool != "all-cs" {
            return Err(anyhow!(
                "Invalid paper pool specified. Must be 'recent' or 'all-cs'"
            ));
        }

        let mut params_map = serde_json::Map::new();
        params_map.insert("limit".to_string(), json!(limit));
        params_map.insert("fields".to_string(), json!(fields));
        params_map.insert("from".to_string(), json!(from_pool));

        let params = Value::Object(params_map);

        // Generate an embedding for the query
        let embedding = self.embed.embed(&paper_id).await?;

        // Check if we have a cached result for a similar query
        let similar_queries = self.cache.search_similarity(&embedding)?;

        // Check for any cached queries with high similarity and matching action/params
        for (cached_query, similarity) in similar_queries.iter() {
            if similarity > &0.95 && cached_query.action == "paper_recommendations_single" {
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
            &format!("/recommendations/v1/papers/forpaper/{}", paper_id),
            Some(&params),
            Some("https://api.semanticscholar.org"),
        )
        .await?;

        let formatted_result = self.format_recommendations(&result)?;

        let query = Query {
            action: "paper_recommendations_single".into(),
            text: paper_id.into(),
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
            name: "paper_recommendations_single".into(),
            description: Some(
                "Get paper recommendations based on a single seed paper in Semantic Scholar".into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "paper_id": {
                        "type": "string",
                        "description": "Paper identifier in one of the following formats: Semantic Scholar ID, DOI:doi, ARXIV:id, MAG:id, ACL:id, PMID:id, PMCID:id, URL:url"
                    },
                    "fields": {
                        "type": "string",
                        "description": "Comma-separated list of fields to return for each paper. Default: title,year,authors",
                        "examples": ["title,year,authors", "title,abstract,authors,url", "title,year,venue,citationCount"]
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of recommendations to return. Default: 100, Maximum: 500"
                    },
                    "from_pool": {
                        "type": "string",
                        "description": "Which pool of papers to recommend from. Default: recent",
                        "enum": ["recent", "all-cs"]
                    }
                },
                "required": ["paper_id"]
            }),
        }
    }
}

pub struct PaperRecommendationMultiTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl PaperRecommendationMultiTool {
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

    fn format_recommendations(&self, response: &Value) -> Result<String> {
        if response.get("error").is_some() {
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Ok(format!("Error: {}", message));
        }

        if let Some(recommended_papers) =
            response.get("recommendedPapers").and_then(|r| r.as_array())
        {
            if recommended_papers.is_empty() {
                return Ok(String::from("No recommendations found for these papers."));
            }

            let mut result = format!(
                "Found {} recommended papers based on your input papers:\n\n",
                recommended_papers.len()
            );

            for (i, paper) in recommended_papers.iter().enumerate() {
                let title = paper
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Unknown title");
                let paper_id = paper
                    .get("paperId")
                    .and_then(|p| p.as_str())
                    .unwrap_or("Unknown ID");

                result.push_str(&format!("{}. {} (ID: {})\n", i + 1, title, paper_id));

                if let Some(year) = paper.get("year").and_then(|y| y.as_u64()) {
                    result.push_str(&format!("   Year: {}\n", year));
                }

                if let Some(venue) = paper.get("venue").and_then(|v| v.as_str()) {
                    if !venue.is_empty() {
                        result.push_str(&format!("   Venue: {}\n", venue));
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

                if let Some(citation_count) = paper.get("citationCount").and_then(|c| c.as_u64()) {
                    result.push_str(&format!("   Citations: {}\n", citation_count));
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

                if i < recommended_papers.len() - 1 {
                    result.push_str("\n");
                }
            }

            result.push_str("\nNote: To get more detailed information about each paper, use the 'fields' parameter.");

            Ok(result)
        } else {
            Ok(String::from(
                "No recommendations found or unexpected API response format.",
            ))
        }
    }
}

#[async_trait]
impl ToolExecutor for PaperRecommendationMultiTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing PaperRecommendationMultiTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let positive_paper_ids = args
            .get("positive_paper_ids")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow!("Missing or invalid positive_paper_ids parameter"))?;

        if positive_paper_ids.is_empty() {
            return Err(anyhow!("Must provide at least one positive paper ID"));
        }

        let positive_ids: Vec<String> = positive_paper_ids
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        if positive_ids.is_empty() {
            return Err(anyhow!("All positive paper IDs must be strings"));
        }

        let negative_paper_ids =
            if let Some(neg_ids) = args.get("negative_paper_ids").and_then(|v| v.as_array()) {
                neg_ids
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            } else {
                Vec::new()
            };

        let fields = args
            .get("fields")
            .and_then(|f| f.as_str())
            .unwrap_or("title,year,authors");

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100);

        if limit > 500 {
            return Err(anyhow!("Limit cannot exceed 500"));
        }

        // Create a query string that uniquely identifies this request
        let query_text = format!(
            "paper_recommendations_multi:positive={:?}:negative={:?}:fields={}:limit={}",
            positive_ids, negative_paper_ids, fields, limit
        );

        // Generate an embedding for the query
        let embedding = self.embed.embed(&query_text).await?;

        // Create the request body for later use and caching
        let request_body = json!({
            "positivePaperIds": positive_ids,
            "negativePaperIds": negative_paper_ids,
            "fields": fields,
            "limit": limit
        });

        // Check if we have a cached result for a similar query
        let similar_queries = self.cache.search_similarity(&embedding)?;

        // Check for any cached queries with high similarity and matching action/params
        for (cached_query, similarity) in similar_queries.iter() {
            if similarity > &0.95 && cached_query.action == "paper_recommendations_multi" {
                // Check if parameters match
                if cached_query.params == Some(request_body.clone()) {
                    log::debug!("Found cached result with similarity {}", similarity);
                    let formatted_result = self.format_recommendations(&cached_query.results)?;
                    return Ok(vec![ToolContent::Text {
                        text: formatted_result,
                    }]);
                }
            }
        }

        // Otherwise, make the API request
        let result = make_request(
            &self.http_client,
            &self.rate_limiter,
            "/recommendations/v1/papers",
            Some(&request_body),
            Some("https://api.semanticscholar.org"),
        )
        .await?;

        let formatted_result = self.format_recommendations(&result)?;

        // Store the result in the cache
        let query = Query {
            action: "paper_recommendations_multi".into(),
            text: query_text,
            embedding,
            params: Some(request_body),
            results: result.clone(),
        };

        if let Err(e) = self.cache.store(query) {
            log::warn!("Failed to store query in cache: {}", e);
        }

        Ok(vec![ToolContent::Text {
            text: formatted_result,
        }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "paper_recommendations_multi".into(),
            description: Some(
                "Get paper recommendations based on multiple positive and optional negative examples in Semantic Scholar".into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "positive_paper_ids": {
                        "type": "array",
                        "description": "List of paper IDs to use as positive examples. Papers similar to these will be recommended.",
                        "items": {
                            "type": "string"
                        }
                    },
                    "negative_paper_ids": {
                        "type": "array",
                        "description": "Optional list of paper IDs to use as negative examples. Papers similar to these will be avoided in recommendations.",
                        "items": {
                            "type": "string"
                        }
                    },
                    "fields": {
                        "type": "string",
                        "description": "Comma-separated list of fields to return for each paper. Default: title,year,authors",
                        "examples": ["title,year,authors", "title,abstract,authors,url", "title,year,venue,citationCount"]
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of recommendations to return. Default: 100, Maximum: 500"
                    }
                },
                "required": ["positive_paper_ids"]
            }),
        }
    }
}
