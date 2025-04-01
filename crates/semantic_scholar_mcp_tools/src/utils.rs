use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use futures::lock::Mutex;
use futures_timer::Delay;
use http_client::{HttpClient, Request, RequestBuilderExt, ResponseAsyncBodyExt};
use serde_json::Value;

pub struct RateLimiter {
    last_call_time: Mutex<HashMap<String, Instant>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            last_call_time: Mutex::new(HashMap::new()),
        }
    }

    pub async fn acquire(&self, endpoint: &str) -> Result<()> {
        let mut last_call_map = self.last_call_time.lock().await;

        let rate_limit = Duration::from_secs(1);

        if let Some(last_call) = last_call_map.get(endpoint) {
            let elapsed = last_call.elapsed();
            if elapsed < rate_limit {
                let sleep_time = rate_limit - elapsed;
                Delay::new(sleep_time).await;
            }
        }

        last_call_map.insert(endpoint.to_string(), Instant::now());
        Ok(())
    }
}

pub async fn make_request(
    http_client: &Arc<dyn HttpClient>,
    rate_limiter: &Arc<RateLimiter>,
    endpoint: &str,
    params: Option<&Value>,
    base_url: Option<&str>,
) -> Result<Value> {
    rate_limiter.acquire(endpoint).await?;

    let base_url = base_url.unwrap_or("https://api.semanticscholar.org/graph/v1");
    let url = if let Some(params) = params {
        let query_string = build_query_string(params)?;
        format!("{}{}?{}", base_url, endpoint, query_string)
    } else {
        format!("{}{}", base_url, endpoint)
    };

    let api_key = std::env::var("SEMANTIC_SCHOLAR_API_KEY").ok();

    let max_retries = 5;
    let mut retry_delay = Duration::from_millis(100);

    let mut attempts = 0;
    loop {
        attempts += 1;

        let mut request_builder = Request::builder().method("GET").uri(url.as_str());

        if let Some(key) = &api_key {
            request_builder = request_builder.header("x-api-key", key);
        }

        let request = request_builder.header("Accept", "application/json").end()?;
        let response = http_client.send(request).await;

        match response {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    let body: Value = response
                        .json()
                        .await
                        .map_err(|e| anyhow!("Failed to parse JSON response: {}", e))?;
                    return Ok(body);
                } else {
                    let error_body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "Unknown error".to_string());

                    if status == 429 || status == 503 || status == 502 {
                        // Rate limiting or server errors - we can retry these
                        if attempts <= max_retries {
                            Delay::new(retry_delay).await;
                            // Exponential backoff
                            retry_delay = retry_delay * 2;
                            continue;
                        } else {
                            return Err(anyhow!(
                                "Rate limit exceeded after {} retries. Consider using an API key for higher limits.",
                                max_retries
                            ));
                        }
                    } else if status == 404 {
                        return Err(anyhow!("Resource not found: {}", error_body));
                    } else {
                        return Err(anyhow!("HTTP error {}: {}", status, error_body));
                    }
                }
            }
            Err(e) => {
                // Network errors might be transient, so we retry
                if attempts <= max_retries {
                    Delay::new(retry_delay).await;
                    // Exponential backoff
                    retry_delay = retry_delay * 2;
                    continue;
                } else {
                    return Err(anyhow!(
                        "Request failed after {} attempts: {}",
                        max_retries,
                        e
                    ));
                }
            }
        }
    }
}

fn build_query_string(params: &Value) -> Result<String> {
    let mut query_parts = Vec::new();

    if let Some(obj) = params.as_object() {
        for (key, value) in obj {
            match value {
                Value::String(s) => {
                    query_parts.push(format!("{}={}", key, urlencoding::encode(s)));
                }
                Value::Number(n) => {
                    query_parts.push(format!("{}={}", key, n));
                }
                Value::Bool(b) => {
                    query_parts.push(format!("{}={}", key, b));
                }
                Value::Array(arr) => {
                    let joined = arr
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(",");
                    query_parts.push(format!("{}={}", key, urlencoding::encode(&joined)));
                }
                _ => {}
            }
        }
    }

    Ok(query_parts.join("&"))
}
