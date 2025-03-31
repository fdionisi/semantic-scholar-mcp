mod utils;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use context_server::{Tool, ToolContent, ToolExecutor};
use http_client::HttpClient;
use serde_json::{Value, json};
use std::sync::Arc;

pub use crate::utils::RateLimiter;

pub struct PaperSearchTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl PaperSearchTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
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

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            "/paper/search",
            Some(&params),
            None,
        )
        .await?;

        let formatted_result = self.format_search_results(&result)?;

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

pub struct PaperDetailsTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl PaperDetailsTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
        }
    }

    fn format_paper_details(&self, response: &Value) -> Result<String> {
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

        let fields = args.get("fields").cloned();

        let params = match fields {
            Some(fields_value) => json!({"fields": fields_value}),
            None => json!({}),
        };

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/paper/{}", paper_id),
            Some(&params),
            None,
        )
        .await?;

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

pub struct PaperCitationsTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl PaperCitationsTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
        }
    }

    fn format_citations(&self, response: &Value) -> Result<String> {
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

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/paper/{}/citations", paper_id),
            Some(&params),
            None,
        )
        .await?;

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

pub struct PaperReferencesTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl PaperReferencesTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
        }
    }

    fn format_references(&self, response: &Value) -> Result<String> {
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
                return Ok(String::from("No references found for this paper."));
            }

            let mut result = format!(
                "Found {} referenced papers (offset: {}):\n\n",
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
                        result.push_str("   [INFLUENTIAL REFERENCE]\n");
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
                result.push_str(&format!("\nFor more references, use offset={}", next));
            }

            Ok(result)
        } else {
            Ok(String::from(
                "No references found or unexpected API response format.",
            ))
        }
    }
}

#[async_trait]
impl ToolExecutor for PaperReferencesTool {
    async fn execute(&self, arguments: Option<Value>) -> Result<Vec<ToolContent>> {
        log::debug!("Executing PaperReferencesTool");
        let args = arguments.ok_or_else(|| anyhow!("Missing arguments"))?;

        let paper_id = args
            .get("paper_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Missing or invalid paper_id parameter"))?;

        if paper_id.trim().is_empty() {
            return Err(anyhow!("Paper ID cannot be empty"));
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

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/paper/{}/references", paper_id),
            Some(&params),
            None,
        )
        .await?;

        let formatted_result = self.format_references(&result)?;

        Ok(vec![ToolContent::Text {
            text: formatted_result,
        }])
    }

    fn to_tool(&self) -> Tool {
        Tool {
            name: "paper_references".into(),
            description: Some("Get papers cited by a specific paper in Semantic Scholar".into()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "paper_id": {
                        "type": "string",
                        "description": "Paper identifier in one of the following formats: Semantic Scholar ID, DOI:doi, ARXIV:id, MAG:id, ACL:id, PMID:id, PMCID:id, URL:url"
                    },
                    "fields": {
                        "type": "array",
                        "description": "List of fields to return for each referenced paper. Default: paperId and title",
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
                        "description": "Number of references to skip for pagination. Default: 0"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of references to return. Default: 100, Maximum: 1000"
                    }
                },
                "required": ["paper_id"]
            }),
        }
    }
}
pub struct AuthorSearchTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl AuthorSearchTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
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

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            "/author/search",
            Some(&params),
            None,
        )
        .await?;

        let formatted_result = self.format_author_search(&result)?;

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

pub struct AuthorDetailsTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl AuthorDetailsTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
        }
    }

    fn format_author_details(&self, response: &Value) -> Result<String> {
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

        let fields = args.get("fields").cloned();

        let params = match fields {
            Some(fields_value) => json!({"fields": fields_value}),
            None => json!({}),
        };

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/author/{}", author_id),
            Some(&params),
            None,
        )
        .await?;

        let formatted_result = self.format_author_details(&result)?;

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

pub struct AuthorPapersTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl AuthorPapersTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
        }
    }

    fn format_author_papers(&self, response: &Value) -> Result<String> {
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

        let mut params_map = serde_json::Map::new();
        params_map.insert("offset".to_string(), json!(offset));
        params_map.insert("limit".to_string(), json!(limit));

        if let Some(f) = fields {
            params_map.insert("fields".to_string(), f);
        }

        let params = Value::Object(params_map);

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/author/{}/papers", author_id),
            Some(&params),
            None,
        )
        .await?;

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

pub struct PaperRecommendationSingleTool {
    http_client: Arc<dyn HttpClient>,
    rate_limiter: Arc<utils::RateLimiter>,
}

impl PaperRecommendationSingleTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
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

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            &format!("/recommendations/v1/papers/forpaper/{}", paper_id),
            Some(&params),
            Some("https://api.semanticscholar.org"),
        )
        .await?;

        let formatted_result = self.format_recommendations(&result)?;

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
    rate_limiter: Arc<utils::RateLimiter>,
}

impl PaperRecommendationMultiTool {
    pub fn new(http_client: Arc<dyn HttpClient>, rate_limiter: Arc<utils::RateLimiter>) -> Self {
        Self {
            http_client,
            rate_limiter,
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

        let request_body = json!({
            "positivePaperIds": positive_ids,
            "negativePaperIds": negative_paper_ids,
            "fields": fields,
            "limit": limit
        });

        let result = utils::make_request(
            &self.http_client,
            &self.rate_limiter,
            "/recommendations/v1/papers",
            Some(&request_body),
            Some("https://api.semanticscholar.org"),
        )
        .await?;

        let formatted_result = self.format_recommendations(&result)?;

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
