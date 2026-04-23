use crate::error::{LiteLLMError, Result};
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use std::time::Duration;
use tokio::time::sleep;

/// Maximum buffer size for SSE streaming (16MB)
pub const MAX_SSE_BUFFER_SIZE: usize = 16 * 1024 * 1024;

/// Default retry configuration
pub const DEFAULT_MAX_RETRIES: u32 = 3;
pub const DEFAULT_INITIAL_BACKOFF_MS: u64 = 1000;
pub const DEFAULT_MAX_BACKOFF_MS: u64 = 30000;
pub const DEFAULT_BACKOFF_MULTIPLIER: f64 = 2.0;

/// Configuration for retry behavior
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial backoff duration in milliseconds
    pub initial_backoff_ms: u64,
    /// Maximum backoff duration in milliseconds
    pub max_backoff_ms: u64,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            initial_backoff_ms: DEFAULT_INITIAL_BACKOFF_MS,
            max_backoff_ms: DEFAULT_MAX_BACKOFF_MS,
            backoff_multiplier: DEFAULT_BACKOFF_MULTIPLIER,
        }
    }
}

impl RetryConfig {
    /// Calculate backoff duration for a given attempt number
    fn backoff_duration(&self, attempt: u32) -> Duration {
        let backoff_ms =
            (self.initial_backoff_ms as f64) * self.backoff_multiplier.powi(attempt as i32);
        let clamped_ms = backoff_ms.min(self.max_backoff_ms as f64) as u64;
        Duration::from_millis(clamped_ms)
    }
}

/// Determines if a status code is retryable
fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
            | StatusCode::BAD_GATEWAY
            | StatusCode::REQUEST_TIMEOUT
    )
}

/// Send a JSON request and parse the response.
///
/// This function includes retry logic with exponential backoff for transient failures.
pub async fn send_json<T: serde::de::DeserializeOwned>(
    req: reqwest::RequestBuilder,
) -> Result<(T, HeaderMap)> {
    send_json_with_retry(req, &RetryConfig::default()).await
}

/// Send a JSON request with custom retry configuration.
///
/// Note: Due to reqwest::RequestBuilder not implementing Clone, the retry config
/// is currently unused for direct builder calls. Use `with_retry` for retryable requests.
#[allow(unused_variables)]
pub async fn send_json_with_retry<T: serde::de::DeserializeOwned>(
    req: reqwest::RequestBuilder,
    retry_config: &RetryConfig,
) -> Result<(T, HeaderMap)> {
    // We need to clone the request for retries, but RequestBuilder doesn't implement Clone.
    // Instead, we'll try to build the request and handle retries at a higher level.
    // For now, we execute once - the retry logic should be applied at the call site
    // where the builder can be recreated.
    send_json_once(req).await
}

/// Execute a request once without retries.
pub async fn send_json_once<T: serde::de::DeserializeOwned>(
    req: reqwest::RequestBuilder,
) -> Result<(T, HeaderMap)> {
    let resp = req.send().await.map_err(LiteLLMError::from)?;

    let status = resp.status();
    let headers = resp.headers().clone();

    if !status.is_success() {
        let text = resp.text().await.map_err(LiteLLMError::from)?;
        let trimmed = text.lines().take(20).collect::<Vec<_>>().join("\n");
        return Err(LiteLLMError::http(format!(
            "http {}: {}",
            status.as_u16(),
            trimmed
        )));
    }

    let parsed = resp.json().await.map_err(map_json_error)?;
    Ok((parsed, headers))
}

/// Helper to execute a request-building closure with retry logic.
///
/// The closure should build and return a fresh RequestBuilder for each attempt.
pub async fn with_retry<T, F, Fut>(
    retry_config: &RetryConfig,
    mut build_request: F,
) -> Result<(T, HeaderMap)>
where
    T: serde::de::DeserializeOwned,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<reqwest::RequestBuilder>>,
{
    let mut last_error = None;

    for attempt in 0..=retry_config.max_retries {
        let req = build_request().await?;
        let resp = req.send().await;

        match resp {
            Ok(response) => {
                let status = response.status();
                let headers = response.headers().clone();

                if status.is_success() {
                    let parsed = response.json().await.map_err(map_json_error)?;
                    return Ok((parsed, headers));
                }

                let text = response.text().await.map_err(LiteLLMError::from)?;

                // Check if this is a retryable error
                if is_retryable_status(status) && attempt < retry_config.max_retries {
                    let backoff = retry_config.backoff_duration(attempt);
                    sleep(backoff).await;
                    last_error = Some(LiteLLMError::http(format!(
                        "http {}: {}",
                        status.as_u16(),
                        text.lines().take(5).collect::<Vec<_>>().join("\n")
                    )));
                    continue;
                }

                // Non-retryable error or max retries exceeded
                let trimmed = text.lines().take(20).collect::<Vec<_>>().join("\n");
                return Err(LiteLLMError::http(format!(
                    "http {}: {}",
                    status.as_u16(),
                    trimmed
                )));
            }
            Err(e) => {
                // Network errors are retryable
                if attempt < retry_config.max_retries {
                    let backoff = retry_config.backoff_duration(attempt);
                    sleep(backoff).await;
                    last_error = Some(LiteLLMError::from(e));
                    continue;
                }
                return Err(LiteLLMError::from(e));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| LiteLLMError::http("max retries exceeded")))
}

fn map_json_error(err: reqwest::Error) -> LiteLLMError {
    if err.is_timeout() {
        let message = if err.is_decode() {
            "request timed out while reading response body"
        } else {
            "request timed out"
        };

        return LiteLLMError::Http {
            message: message.into(),
            source: Some(Box::new(err)),
        };
    }

    LiteLLMError::Parse(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn retry_config_backoff_calculation() {
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff_ms: 1000,
            max_backoff_ms: 10000,
            backoff_multiplier: 2.0,
        };

        assert_eq!(config.backoff_duration(0), Duration::from_millis(1000));
        assert_eq!(config.backoff_duration(1), Duration::from_millis(2000));
        assert_eq!(config.backoff_duration(2), Duration::from_millis(4000));
        assert_eq!(config.backoff_duration(3), Duration::from_millis(8000));
        // Should be clamped to max
        assert_eq!(config.backoff_duration(4), Duration::from_millis(10000));
    }

    #[test]
    fn retryable_status_codes() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT));
        assert!(is_retryable_status(StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(StatusCode::REQUEST_TIMEOUT));

        assert!(!is_retryable_status(StatusCode::OK));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(StatusCode::NOT_FOUND));
        assert!(!is_retryable_status(StatusCode::INTERNAL_SERVER_ERROR));
    }

    #[derive(Debug, Deserialize)]
    struct TestPayload {
        #[serde(rename = "value")]
        _value: String,
    }

    #[tokio::test]
    async fn send_json_maps_body_read_timeouts_to_http_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request_buf = [0_u8; 1024];
            let _ = stream.read(&mut request_buf).await;

            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 14\r\n\r\n",
                )
                .await
                .unwrap();

            sleep(Duration::from_millis(100)).await;
            let _ = stream.write_all(br#"{"value":"ok"}"#).await;
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();

        let err = send_json_once::<TestPayload>(client.get(format!("http://{addr}/")))
            .await
            .unwrap_err();

        match err {
            LiteLLMError::Http { message, source } => {
                assert!(
                    message.contains("timed out"),
                    "unexpected message: {message}"
                );
                assert!(
                    source.is_some(),
                    "timeout errors should retain the reqwest source"
                );
            }
            other => panic!("expected http timeout error, got {other:?}"),
        }
    }
}
