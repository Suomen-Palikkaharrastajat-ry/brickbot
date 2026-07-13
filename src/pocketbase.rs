use crate::config::PocketBaseConfig;
use serde_json::Value;

pub async fn push_event_data(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &PocketBaseConfig,
    payload: &Value,
    limit: u64,
) -> anyhow::Result<String> {
    let url = format!(
        "{}/api/collections/{}/records",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.collection
    );

    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let mut clean_payload = payload.clone();
    let image_url = clean_payload
        .as_object_mut()
        .and_then(|obj| obj.remove("image_url"));

    let resp = if let Some(url_val) = image_url {
        if let Some(url_str) = url_val.as_str() {
            match http.get_bounded_bytes(url_str, limit, false).await {
                Ok(bytes) => {
                    http.post_multipart_with_auth(
                        &url,
                        auth.as_deref(),
                        &clean_payload,
                        "cover.png",
                        bytes.to_vec(),
                        limit,
                    )
                    .await?
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch image from {}, falling back to JSON post: {}",
                        url_str,
                        e
                    );
                    http.post_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                        .await?
                }
            }
        } else {
            http.post_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                .await?
        }
    } else {
        http.post_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
            .await?
    };

    let json: Value = serde_json::from_str(&resp)?;
    let id = json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::MockHttpProvider;

    #[tokio::test]
    async fn test_push_event_data_success() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_post_json_with_auth()
            .with(
                mockall::predicate::eq("https://pb.example.com/api/collections/events/records"),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(r#"{"id":"9ket96ibdt566u6"}"#.to_string()));

        let pb_cfg = PocketBaseConfig {
            url: "https://pb.example.com".to_string(),
            collection: "events".to_string(),
        };

        std::env::set_var("POCKETBASE_IMPERSONATE_AUTH_TOKEN", "secret_token");
        let payload = serde_json::json!({"title": "Test"});
        let res = push_event_data(&mock_http, &pb_cfg, &payload, 1024 * 1024).await;
        std::env::remove_var("POCKETBASE_IMPERSONATE_AUTH_TOKEN");
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "9ket96ibdt566u6");
    }

    #[tokio::test]
    async fn test_push_event_data_error() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_post_json_with_auth()
            .times(1)
            .returning(|_, _, _, _| Err(anyhow::anyhow!("Network error")));

        let pb_cfg = PocketBaseConfig {
            url: "https://pb.example.com".to_string(),
            collection: "events".to_string(),
        };

        std::env::remove_var("POCKETBASE_IMPERSONATE_AUTH_TOKEN");
        let payload = serde_json::json!({"title": "Test"});
        let res = push_event_data(&mock_http, &pb_cfg, &payload, 1024 * 1024).await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "Network error");
    }
}
