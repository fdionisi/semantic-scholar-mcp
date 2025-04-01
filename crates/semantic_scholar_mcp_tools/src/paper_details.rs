use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cache::{Cache, Query};
use context_server::{Tool, ToolContent, ToolExecutor};
use embed::Embed;
use http_client::HttpClient;
use serde_json::{Value, json};

use crate::utils::{RateLimiter, make_request};

pub struct PaperDetailsTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<RateLimiter>,
    cache: Arc<dyn Cache>,
    embed: Arc<dyn Embed>,
}

impl PaperDetailsTool {
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

    pub(crate) fn format_paper_details(&self, response: &Value) -> Result<String> {
        if response.get("error").is_some() {
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Ok(format!("Error: {}", message));
        }

        let title = response
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("Unknown title");
        let paper_id = response
            .get("paperId")
            .and_then(|p| p.as_str())
            .unwrap_or("Unknown ID");

        let mut result = format!("Paper Details: {}\n", title);
        result.push_str(&format!("Paper ID: {}\n\n", paper_id));

        if let Some(authors) = response.get("authors").and_then(|a| a.as_array()) {
            result.push_str("Authors:\n");
            for author in authors {
                let name = author
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("Unknown");
                let author_id = author
                    .get("authorId")
                    .and_then(|id| id.as_str())
                    .unwrap_or("Unknown");
                result.push_str(&format!("- {} (ID: {})\n", name, author_id));
            }
            result.push_str("\n");
        }

        if let Some(year) = response.get("year").and_then(|y| y.as_u64()) {
            result.push_str(&format!("Year: {}\n", year));
        }

        if let Some(venue) = response.get("venue").and_then(|v| v.as_str()) {
            if !venue.is_empty() {
                result.push_str(&format!("Venue: {}\n", venue));
            }
        }

        if let Some(publication_date) = response.get("publicationDate").and_then(|d| d.as_str()) {
            result.push_str(&format!("Publication Date: {}\n", publication_date));
        }

        if let Some(citation_count) = response.get("citationCount").and_then(|c| c.as_u64()) {
            result.push_str(&format!("Citation Count: {}\n", citation_count));
        }

        if let Some(influential_citation_count) = response
            .get("influentialCitationCount")
            .and_then(|c| c.as_u64())
        {
            result.push_str(&format!(
                "Influential Citation Count: {}\n",
                influential_citation_count
            ));
        }

        if let Some(fields_of_study) = response.get("fieldsOfStudy").and_then(|f| f.as_array()) {
            let fields: Vec<&str> = fields_of_study
                .iter()
                .filter_map(|field| field.as_str())
                .collect();

            if !fields.is_empty() {
                result.push_str(&format!("Fields of Study: {}\n", fields.join(", ")));
            }
        }

        if let Some(is_open_access) = response.get("isOpenAccess").and_then(|o| o.as_bool()) {
            result.push_str(&format!(
                "Open Access: {}\n",
                if is_open_access { "Yes" } else { "No" }
            ));

            if is_open_access {
                if let Some(pdf) = response.get("openAccessPdf") {
                    if let Some(url) = pdf.get("url").and_then(|u| u.as_str()) {
                        result.push_str(&format!("Open Access PDF: {}\n", url));
                    }
                }
            }
        }

        if let Some(abstract_text) = response.get("abstract").and_then(|a| a.as_str()) {
            if !abstract_text.is_empty() {
                result.push_str(&format!("\nAbstract:\n{}\n", abstract_text));
            }
        }

        if let Some(tldr) = response.get("tldr") {
            if let Some(text) = tldr.get("text").and_then(|t| t.as_str()) {
                result.push_str(&format!("\nTL;DR:\n{}\n", text));
            }
        }

        if let Some(url) = response.get("url").and_then(|u| u.as_str()) {
            result.push_str(&format!("\nSemantic Scholar URL: {}\n", url));
        }

        if let Some(external_ids) = response.get("externalIds") {
            result.push_str("\nExternal IDs:\n");

            if let Some(doi) = external_ids.get("DOI").and_then(|d| d.as_str()) {
                result.push_str(&format!("DOI: {}\n", doi));
            }

            if let Some(arxiv) = external_ids.get("ArXiv").and_then(|a| a.as_str()) {
                result.push_str(&format!("ArXiv: {}\n", arxiv));
            }

            if let Some(pmid) = external_ids.get("PubMed").and_then(|p| p.as_str()) {
                result.push_str(&format!("PubMed: {}\n", pmid));
            }

            if let Some(acl) = external_ids.get("ACL").and_then(|a| a.as_str()) {
                result.push_str(&format!("ACL: {}\n", acl));
            }
        }

        if let Some(citations) = response.get("citations").and_then(|c| c.as_array()) {
            result.push_str(&format!("\nCitations: {} papers\n", citations.len()));
            result.push_str("(Use the paper_citations tool with this paper ID to see details)\n");
        }

        if let Some(references) = response.get("references").and_then(|r| r.as_array()) {
            result.push_str(&format!("\nReferences: {} papers\n", references.len()));
            result.push_str("(Use the paper_references tool with this paper ID to see details)\n");
        }

        Ok(result)
    }
}

#[async_trait]
impl ToolExecutor for PaperDetailsTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing PaperDetailsTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let paper_id = args
            .get("paper_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid paper_id parameter"))?;

        if paper_id.trim().is_empty() {
            return Err(anyhow!("Paper ID cannot be empty"));
        }

        // Create a query string to use for embedding and cache lookup
        let query_text = format!("paper_details:{}", paper_id);

        // Generate an embedding for the query
        let embedding = self.embed.embed(&query_text).await?;

        // Check if we have this query in the cache
        let cache_results = self.cache.search_similarity(&embedding)?;

        let result = if !cache_results.is_empty() && cache_results[0].1 > 0.95 {
            // Use the cached result if similarity is high enough
            log::debug!("Using cached result for paper details");
            cache_results[0].0.results.clone()
        } else {
            // Otherwise make the API request
            let fields = args.get("fields").cloned();

            let params = match fields {
                Some(fields_value) => json!({"fields": fields_value}),
                None => json!({}),
            };

            let api_result = make_request(
                &self.http_client,
                &self.rate_limiter,
                &format!("/paper/{}", paper_id),
                Some(&params),
                None,
            )
            .await?;

            // Store the result in the cache
            self.cache.store(Query {
                text: query_text,
                embedding,
                results: api_result.clone(),
            })?;

            api_result
        };

        let formatted_result = self.format_paper_details(&result)?;

        Ok(vec![ToolContent::Text {
            text: formatted_result,
        }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "paper_details".into(),
            description: Some(
                "Get detailed information about a specific paper from Semantic Scholar".into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "paper_id": {
                        "type": "string",
                        "description": "Paper identifier in one of the following formats: Semantic Scholar ID, DOI:doi, ARXIV:id, MAG:id, ACL:id, PMID:id, PMCID:id, URL:url"
                    },
                    "fields": {
                        "type": "array",
                        "description": "List of fields to return. Default: title and abstract",
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
                    }
                },
                "required": ["paper_id"]
            }),
        }
    }
}
