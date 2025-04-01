use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cache::{Cache, Query};
use context_server::{Tool, ToolContent, ToolExecutor};
use embed::Embed;
use http_client::HttpClient;
use serde_json::{Value, json};

use crate::utils::{RateLimiter, make_request};

pub struct PaperSearchTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl PaperSearchTool {
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

    fn format_search_results(&self, response: &Value) -> Result<String> {
        if response.get("error").is_some() {
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Ok(format!("Error: {}", message));
        }

        if let Some(data) = response.get("data").and_then(|d| d.as_array()) {
            if data.is_empty() {
                return Ok(String::from("No papers found matching your criteria."));
            }

            let total = response.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
            let offset = response.get("offset").and_then(|o| o.as_u64()).unwrap_or(0);

            let mut result = format!(
                "Found {} total papers matching your query. Showing results {}-{}:\n\n",
                total,
                offset + 1,
                offset + data.len() as u64
            );

            for (i, paper) in data.iter().enumerate() {
                let title = paper
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("Unknown title");

                result.push_str(&format!("{}. {}\n", i + 1, title));

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

                if let Some(abstract_text) = paper.get("abstract").and_then(|a| a.as_str()) {
                    if !abstract_text.is_empty() {
                        let summary = if abstract_text.len() > 300 {
                            format!("{}...", &abstract_text[0..300])
                        } else {
                            abstract_text.to_string()
                        };
                        result.push_str(&format!("   Abstract: {}\n", summary));
                    }
                }

                if let Some(url) = paper.get("url").and_then(|u| u.as_str()) {
                    result.push_str(&format!("   URL: {}\n", url));
                }

                if let Some(paper_id) = paper.get("paperId").and_then(|p| p.as_str()) {
                    result.push_str(&format!("   Paper ID: {}\n", paper_id));
                }

                if i < data.len() - 1 {
                    result.push_str("\n");
                }
            }

            if let Some(next) = response.get("next").and_then(|n| n.as_u64()) {
                result.push_str(&format!("\nFor more results, use offset={}", next));
            }

            Ok(result)
        } else {
            Ok(String::from(
                "No results found or unexpected API response format.",
            ))
        }
    }
}

#[async_trait]
impl ToolExecutor for PaperSearchTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing PaperSearchTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid query parameter"))?;

        if query.trim().is_empty() {
            return Err(anyhow!("Query string cannot be empty"));
        }

        let fields = args.get("fields").cloned().unwrap_or_else(|| {
            json!([
                "title",
                "abstract",
                "year",
                "citationCount",
                "authors",
                "url"
            ])
        });

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);

        if limit > 100 {
            return Err(anyhow!("Limit cannot exceed 100"));
        }

        let params = json!({
            "query": query,
            "fields": fields,
            "offset": offset,
            "limit": limit,
            "publication_types": args.get("publication_types"),
            "open_access_pdf": args.get("open_access_pdf"),
            "min_citation_count": args.get("min_citation_count"),
            "year": args.get("year"),
            "venue": args.get("venue"),
            "fields_of_study": args.get("fields_of_study")
        });

        // Generate an embedding for the query
        let embedding = self.embed.embed(&query).await?;

        // Check if we have a cached result for a similar query
        let similar_queries = self.cache.search_similarity(&embedding)?;

        // Check for any cached queries with high similarity and matching action/params
        for (cached_query, similarity) in similar_queries.iter() {
            if similarity > &0.95 && cached_query.action == "paper_search" {
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
            "/paper/search",
            Some(&params),
            None,
        )
        .await?;

        let formatted_result = self.format_search_results(&result)?;

        let query = Query {
            action: "paper_search".into(),
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
            name: "paper_search".into(),
            description: Some(
                "Search for papers on Semantic Scholar using relevance-based ranking".into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "A text query to search for. The query will be matched against paper titles, abstracts, venue names, and author names."
                    },
                    "fields": {
                        "type": "array",
                        "description": "List of fields to return for each paper. Default: title, abstract, year, citationCount, authors, url",
                        "items": {
                            "type": "string",
                            "enum": [
                                "title", "abstract", "year", "citationCount", "authors", "url",
                                "citations", "references", "venue", "influentialCitationCount",
                                "corpusId", "externalIds", "fieldsOfStudy", "isOpenAccess",
                                "openAccessPdf", "paperId", "publicationDate", "publicationTypes",
                                "publicationVenue", "s2FieldsOfStudy", "tldr"
                            ]
                        }
                    },
                    "publication_types": {
                        "type": "array",
                        "description": "Filter by publication types",
                        "items": {
                            "type": "string",
                            "enum": [
                                "Review", "JournalArticle", "CaseReport", "ClinicalTrial",
                                "Conference", "Dataset", "Editorial", "LettersAndComments",
                                "MetaAnalysis", "News", "Study", "Book", "BookSection"
                            ]
                        }
                    },
                    "open_access_pdf": {
                        "type": "boolean",
                        "description": "If true, only include papers with a public PDF"
                    },
                    "min_citation_count": {
                        "type": "integer",
                        "description": "Minimum number of citations required"
                    },
                    "year": {
                        "type": "string",
                        "description": "Filter by publication year. Formats: '2019', '2016-2020', '2010-', '-2015'"
                    },
                    "venue": {
                        "type": "array",
                        "description": "Filter by publication venues",
                        "items": {
                            "type": "string"
                        }
                    },
                    "fields_of_study": {
                        "type": "array",
                        "description": "Filter by fields of study",
                        "items": {
                            "type": "string"
                        }
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Number of results to skip for pagination"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results to return (max: 100)"
                    }
                },
                "required": ["query"]
            }),
        }
    }
}
