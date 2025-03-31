# Semantic Scholar MCP (Model Context Protocol)

A lightweight service that connects Large Language Models to the Semantic Scholar API, enabling AI assistants to search for and analyse academic papers.

## Features

- Implements the Context Server RPC protocol
- Provides tools for searching papers, retrieving paper details, and exploring author information
- Returns formatted academic information including abstracts, citations, and author details
- Supports rate limiting to comply with Semantic Scholar API usage guidelines
- Handles pagination for search results and citations

## Requirements

- Rust toolchain
- `SEMANTIC_SCHOLAR_API_KEY` environment variable with your Semantic Scholar API key

## Tool Parameters

The MCP provides several tools, including:

### paper_search
- `query`: Search query string (required)
- `fields`: List of fields to return for each paper
- `offset`: Number of results to skip for pagination
- `limit`: Maximum number of results to return (max: 100)
- `publication_types`: Filter by publication types
- `open_access_pdf`: If true, only include papers with a public PDF
- `min_citation_count`: Minimum number of citations required
- `year`: Filter by publication year
- `venue`: Filter by publication venues
- `fields_of_study`: Filter by fields of study

### paper_details
- `paper_id`: Identifier for the specific paper (required)
- `fields`: List of fields to return

### author_search
- `query`: Author name to search for (required)
- `fields`: List of fields to return for each author
- `offset`: Number of results to skip for pagination
- `limit`: Maximum number of results to return (max: 1000)

### paper_citations
- `paper_id`: Identifier for the paper to get citations for (required)
- `fields`: List of fields to return for each citing paper
- `offset`: Number of citations to skip for pagination
- `limit`: Maximum number of citations to return (max: 1000)

### author_details
- `author_id`: Semantic Scholar author ID (required)
- `fields`: List of fields to return

### author_papers
- `author_id`: Semantic Scholar author ID (required)
- `fields`: List of fields to return for each paper
- `offset`: Number of papers to skip for pagination
- `limit`: Maximum number of papers to return (max: 1000)

### paper_references
- `paper_id`: Identifier for the paper to get references for (required)
- `fields`: List of fields to return for each referenced paper
- `offset`: Number of references to skip for pagination
- `limit`: Maximum number of references to return (max: 1000)

### paper_recommendations_single
- `paper_id`: Identifier for the seed paper (required)
- `fields`: List of fields to return for each recommended paper
- `limit`: Maximum number of recommendations to return (max: 500)
- `from_pool`: Which pool of papers to recommend from ('recent' or 'all-cs')

### paper_recommendations_multi
- `positive_paper_ids`: List of paper IDs to use as positive examples (required)
- `negative_paper_ids`: Optional list of paper IDs to use as negative examples
- `fields`: List of fields to return for each recommended paper
- `limit`: Maximum number of recommendations to return (max: 500)

## Usage

1. Set the `SEMANTIC_SCHOLAR_API_KEY` environment variable with your API key.
2. Run the Semantic Scholar MCP service.
3. Send JSON-RPC requests to the service with the desired tool and parameters.
4. Receive formatted responses with academic information.

## Rate Limiting

The service implements rate limiting to comply with Semantic Scholar API usage guidelines:
- 100ms delay between most API calls
- 1 second delay for batch operations, paper search, and recommendations

## Error Handling

The service provides informative error messages for various scenarios, including:
- Missing or invalid parameters
- API rate limit exceeded
- Resource not found
- HTTP errors

## License

MIT
