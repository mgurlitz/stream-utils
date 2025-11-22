use flate2::read::GzDecoder;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use std::io::Read;
use std::time::{Duration, Instant};

pub type HttpClient = Client<
    hyper_tls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Empty<Bytes>,
>;

pub fn build_client(insecure: bool) -> HttpClient {
    let mut http = hyper_util::client::legacy::connect::HttpConnector::new();
    http.enforce_http(false);

    let https = if insecure {
        let tls = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .expect("Failed to build TLS connector");
        hyper_tls::HttpsConnector::from((http, tls.into()))
    } else {
        hyper_tls::HttpsConnector::from((http, native_tls::TlsConnector::new().unwrap().into()))
    };

    Client::builder(TokioExecutor::new()).build(https)
}

pub async fn fetch_url(
    client: &HttpClient,
    url: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let uri: hyper::Uri = url.parse()?;
    let req = Request::builder()
        .uri(&uri)
        .header("User-Agent", "m3u8-dl/1.0")
        .header("Accept-Encoding", "gzip, identity")
        .body(Empty::<Bytes>::new())?;

    let resp = client.request(req).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("HTTP {status} for {url}").into());
    }

    // Check if response is gzip encoded
    let is_gzip = resp
        .headers()
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_lowercase().contains("gzip"))
        .unwrap_or(false);

    let body = resp.collect().await?.to_bytes();

    if is_gzip {
        let mut decoder = GzDecoder::new(&body[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    } else {
        Ok(body.to_vec())
    }
}

/// Fetch with retries, respecting a total timeout budget across all attempts.
/// Individual attempts don't have their own timeout - we just keep trying until
/// either success, max retries, or the total timeout is exhausted.
pub async fn fetch_with_retry(
    client: &HttpClient,
    url: &str,
    total_timeout: Duration,
    max_retries: u32,
    retry_delay_ms: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let start = Instant::now();
    let mut last_err = None;

    for attempt in 0..=max_retries {
        // Check if we've exceeded the total timeout
        if start.elapsed() >= total_timeout {
            break;
        }

        // Calculate remaining time for this attempt
        let remaining = total_timeout.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            break;
        }

        // Try the fetch with the remaining timeout
        match tokio::time::timeout(remaining, fetch_url(client, url)).await {
            Ok(Ok(data)) => return Ok(data),
            Ok(Err(e)) => last_err = Some(e),
            Err(_) => last_err = Some("Request timed out".into()),
        }

        // Don't sleep after the last attempt or if we're out of time
        if attempt < max_retries && start.elapsed() < total_timeout {
            let sleep_time = Duration::from_millis(retry_delay_ms)
                .min(total_timeout.saturating_sub(start.elapsed()));
            if !sleep_time.is_zero() {
                tokio::time::sleep(sleep_time).await;
            }
        }
    }

    Err(last_err.unwrap_or_else(|| format!("Fetch failed after {total_timeout:?}").into()))
}
