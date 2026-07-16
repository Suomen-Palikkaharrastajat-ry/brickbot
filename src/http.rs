#![allow(clippy::cast_possible_truncation)]
use bytes::Bytes;
use moka::future::Cache;
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{error, info, warn};

#[cfg(test)]
use mockall::{automock, predicate::*};

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct CacheKey {
    pub method: String,
    pub normalized_url: String,
    pub auth_scope: Option<String>,
}

static CACHE: OnceLock<Cache<CacheKey, Bytes>> = OnceLock::new();

pub fn init_cache(ttl_secs: u64, max_memory: u64) {
    let _ = CACHE.set(
        Cache::builder()
            .weigher(|_key, value: &Bytes| value.len() as u32)
            .max_capacity(max_memory)
            .time_to_live(Duration::from_secs(ttl_secs))
            .build(),
    );
}

fn get_cache() -> Cache<CacheKey, Bytes> {
    CACHE
        .get_or_init(|| {
            let max_memory = std::env::var("CACHE_MAX_MEMORY_BYTES")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(50 * 1024 * 1024); // Default 50MB

            let ttl_secs = std::env::var("CACHE_TTL_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(600); // 10 minutes TTL by default

            Cache::builder()
                .weigher(|_key, value: &Bytes| value.len() as u32)
                .max_capacity(max_memory)
                .time_to_live(Duration::from_secs(ttl_secs))
                .build()
        })
        .clone()
}

use async_trait::async_trait;

#[cfg_attr(test, automock)]
#[async_trait]
pub trait HttpProvider: Send + Sync {
    async fn get_bounded_bytes<'a>(
        &self,
        url: &'a str,
        limit: u64,
        skip_cache: bool,
    ) -> anyhow::Result<Bytes>;
    async fn get_bounded_text<'a>(
        &self,
        url: &'a str,
        limit: u64,
        skip_cache: bool,
    ) -> anyhow::Result<String>;
    async fn get_text_basic_auth<'a, 'b>(
        &self,
        url: &'a str,
        user: &'a str,
        pass: Option<&'b str>,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn get_json_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        skip_cache: bool,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn post_json_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn post_multipart_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        file_name: &'a str,
        file_bytes: Vec<u8>,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn patch_json_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn patch_multipart_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        file_name: &'a str,
        file_bytes: Vec<u8>,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn post_form<'a>(
        &self,
        url: &'a str,
        form: Vec<(String, String)>,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn post_form_basic_auth<'a, 'b>(
        &self,
        url: &'a str,
        user: &'a str,
        pass: Option<&'b str>,
        form: Vec<(String, String)>,
        limit: u64,
    ) -> anyhow::Result<String>;
    async fn patch_form_basic_auth<'a, 'b>(
        &self,
        url: &'a str,
        user: &'a str,
        pass: Option<&'b str>,
        form: Vec<(String, String)>,
        limit: u64,
    ) -> anyhow::Result<String>;
}

#[derive(Clone, Default)]
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    async fn execute_request(
        &self,
        req: RequestBuilder,
        log_desc: &str,
    ) -> anyhow::Result<reqwest::Response> {
        info!("Sending HTTP Request: {}", log_desc);

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    info!("HTTP Request Successful [{}]: {}", status, log_desc);
                    Ok(resp)
                } else {
                    warn!("HTTP Request Failed with status {}: {}", status, log_desc);
                    let err_text = resp.text().await.unwrap_or_default();
                    Err(anyhow::anyhow!("HTTP error {status}: {err_text}"))
                }
            }
            Err(e) => {
                error!("HTTP Request Error: {}: {}", e, log_desc);
                Err(e.into())
            }
        }
    }
}

fn redact_url(url: &str) -> String {
    reqwest::Url::parse(url).map_or_else(
        |_| url.to_string(),
        |mut parsed| {
            let query_pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
            parsed.query_pairs_mut().clear();
            for (k, v) in query_pairs {
                let k_lower = k.to_lowercase();
                if k_lower.contains("token")
                    || k_lower.contains("key")
                    || k_lower.contains("pass")
                    || k_lower.contains("secret")
                    || k_lower.contains("hash")
                {
                    parsed.query_pairs_mut().append_pair(&k, "***");
                } else {
                    parsed.query_pairs_mut().append_pair(&k, &v);
                }
            }
            parsed.to_string()
        },
    )
}

async fn read_bounded_body(mut resp: reqwest::Response, limit: u64) -> anyhow::Result<Bytes> {
    if let Some(content_length) = resp.content_length() {
        if content_length > limit {
            return Err(anyhow::anyhow!(
                "BodyTooLarge: Content-Length {content_length} exceeds limit {limit}"
            ));
        }
    }

    let mut body = bytes::BytesMut::new();
    while let Some(chunk) = resp.chunk().await? {
        if (body.len() + chunk.len()) as u64 > limit {
            return Err(anyhow::anyhow!(
                "BodyTooLarge: Body size exceeds limit {limit}"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

#[async_trait]
impl HttpProvider for HttpClient {
    async fn get_bounded_bytes<'a>(
        &self,
        url: &'a str,
        limit: u64,
        skip_cache: bool,
    ) -> anyhow::Result<Bytes> {
        let cache = get_cache();
        let safe_url = redact_url(url);
        let cache_key = CacheKey {
            method: "GET".to_string(),
            normalized_url: safe_url.clone(),
            auth_scope: None,
        };

        if !skip_cache {
            if let Some(cached) = cache.get(&cache_key).await {
                tracing::debug!("Cache hit for {}", url);
                return Ok(cached);
            }
        }

        let req = self.client.get(url);
        let resp = self
            .execute_request(req, &format!("GET {safe_url}"))
            .await?;

        let bytes = read_bounded_body(resp, limit).await?;
        if !skip_cache {
            cache.insert(cache_key, bytes.clone()).await;
        }
        Ok(bytes)
    }

    async fn get_bounded_text<'a>(
        &self,
        url: &'a str,
        limit: u64,
        skip_cache: bool,
    ) -> anyhow::Result<String> {
        let bytes = self.get_bounded_bytes(url, limit, skip_cache).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn get_text_basic_auth<'a, 'b>(
        &self,
        url: &'a str,
        user: &'a str,
        pass: Option<&'b str>,
        limit: u64,
    ) -> anyhow::Result<String> {
        let safe_url = redact_url(url);
        let req = self.client.get(url).basic_auth(user, pass);
        let resp = self
            .execute_request(req, &format!("GET BASIC {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn get_json_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        skip_cache: bool,
        limit: u64,
    ) -> anyhow::Result<String> {
        let cache = get_cache();
        let safe_url = redact_url(url);

        let auth_scope = auth_header.map(|h| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&h, &mut hasher);
            format!("{:x}", std::hash::Hasher::finish(&hasher))
        });

        let cache_key = CacheKey {
            method: "GET".to_string(),
            normalized_url: safe_url.clone(),
            auth_scope,
        };

        if !skip_cache {
            if let Some(cached) = cache.get(&cache_key).await {
                tracing::debug!("Cache hit for GET {} (with auth)", url);
                return Ok(std::str::from_utf8(&cached)?.to_string());
            }
        }

        let mut req = self.client.get(url);
        if let Some(auth) = auth_header {
            req = req.header("Authorization", auth);
        }
        let resp = self
            .execute_request(req, &format!("GET {safe_url} (with auth)"))
            .await?;

        let bytes = read_bounded_body(resp, limit).await?;

        if !skip_cache {
            cache.insert(cache_key, bytes.clone()).await;
        }
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn post_json_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        limit: u64,
    ) -> anyhow::Result<String> {
        let mut req = self.client.post(url).json(payload);
        if let Some(auth) = auth_header {
            req = req.header("Authorization", auth);
        }
        let safe_url = redact_url(url);
        let resp = self
            .execute_request(req, &format!("POST {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn post_multipart_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        file_name: &'a str,
        file_bytes: Vec<u8>,
        limit: u64,
    ) -> anyhow::Result<String> {
        let mut form = reqwest::multipart::Form::new();

        if let Some(obj) = payload.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    form = form.text(k.clone(), s.to_string());
                } else {
                    form = form.text(k.clone(), v.to_string());
                }
            }
        }

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name.to_string())
            .mime_str("application/octet-stream")?;

        form = form.part("image", part);

        let mut req = self.client.post(url).multipart(form);
        if let Some(auth) = auth_header {
            req = req.header("Authorization", auth);
        }
        let safe_url = redact_url(url);
        let resp = self
            .execute_request(req, &format!("POST MULTIPART {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn patch_json_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        limit: u64,
    ) -> anyhow::Result<String> {
        let mut req = self.client.patch(url).json(payload);
        if let Some(auth) = auth_header {
            req = req.header("Authorization", auth);
        }
        let safe_url = redact_url(url);
        let resp = self
            .execute_request(req, &format!("PATCH {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn patch_multipart_with_auth<'a, 'b>(
        &self,
        url: &'a str,
        auth_header: Option<&'b str>,
        payload: &'a Value,
        file_name: &'a str,
        file_bytes: Vec<u8>,
        limit: u64,
    ) -> anyhow::Result<String> {
        let mut form = reqwest::multipart::Form::new();

        if let Some(obj) = payload.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    form = form.text(k.clone(), s.to_string());
                } else {
                    form = form.text(k.clone(), v.to_string());
                }
            }
        }

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name.to_string())
            .mime_str("application/octet-stream")?;

        form = form.part("image", part);

        let mut req = self.client.patch(url).multipart(form);
        if let Some(auth) = auth_header {
            req = req.header("Authorization", auth);
        }
        let safe_url = redact_url(url);
        let resp = self
            .execute_request(req, &format!("PATCH MULTIPART {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn post_form<'a>(
        &self,
        url: &'a str,
        form: Vec<(String, String)>,
        limit: u64,
    ) -> anyhow::Result<String> {
        let cache = get_cache();
        let safe_url = redact_url(url);
        let mut form_str = String::new();
        for (k, v) in &form {
            form_str.push_str(k);
            form_str.push('=');
            form_str.push_str(v);
            form_str.push('&');
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&form_str, &mut hasher);
        let form_hash = std::hash::Hasher::finish(&hasher);

        let cache_key = CacheKey {
            method: "POST_FORM".to_string(),
            normalized_url: safe_url.clone(),
            auth_scope: Some(format!("{form_hash:x}")),
        };

        if let Some(cached) = cache.get(&cache_key).await {
            tracing::debug!("Cache hit for POST FORM {}", url);
            return Ok(std::str::from_utf8(&cached)?.to_string());
        }

        let req = self.client.post(url).form(&form);
        let resp = self
            .execute_request(req, &format!("POST FORM {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        cache.insert(cache_key, bytes.clone()).await;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn post_form_basic_auth<'a, 'b>(
        &self,
        url: &'a str,
        user: &'a str,
        pass: Option<&'b str>,
        form: Vec<(String, String)>,
        limit: u64,
    ) -> anyhow::Result<String> {
        let safe_url = redact_url(url);
        let req = self.client.post(url).basic_auth(user, pass).form(&form);
        let resp = self
            .execute_request(req, &format!("POST FORM BASIC {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }

    async fn patch_form_basic_auth<'a, 'b>(
        &self,
        url: &'a str,
        user: &'a str,
        pass: Option<&'b str>,
        form: Vec<(String, String)>,
        limit: u64,
    ) -> anyhow::Result<String> {
        let safe_url = redact_url(url);
        let req = self.client.patch(url).basic_auth(user, pass).form(&form);
        let resp = self
            .execute_request(req, &format!("PATCH FORM BASIC {safe_url}"))
            .await?;
        let bytes = read_bounded_body(resp, limit).await?;
        Ok(std::str::from_utf8(&bytes)?.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_http_provider() {
        let mut mock = MockHttpProvider::new();
        mock.expect_get_bounded_text()
            .returning(|_, _, _| Ok("mocked".to_string()));

        let result = mock
            .get_bounded_text("http://example.com", 1000, false)
            .await
            .unwrap();
        assert_eq!(result, "mocked");
    }
}
