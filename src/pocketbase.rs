use crate::config::PocketBaseConfig;
use serde_json::Value;

/// Creates or updates a single record in `collection`.
///
/// A payload without an `id` is created with `POST`; one carrying an `id` updates that record
/// with `PATCH`. An `image_url` key is stripped and, when the image can be fetched, uploaded as
/// a multipart `cover.png` alongside the remaining fields.
pub async fn push_record_data(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &PocketBaseConfig,
    collection: &str,
    payload: &Value,
    limit: u64,
) -> anyhow::Result<String> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let mut clean_payload = payload.clone();
    let image_url = clean_payload
        .as_object_mut()
        .and_then(|obj| obj.remove("image_url"));

    let record_id = clean_payload
        .as_object_mut()
        .and_then(|obj| obj.remove("id"))
        .and_then(|v| v.as_str().map(std::string::ToString::to_string));

    let url = record_id.as_ref().map_or_else(
        || {
            format!(
                "{}/api/collections/{}/records",
                pb_cfg.url.trim_end_matches('/'),
                collection
            )
        },
        |id| {
            format!(
                "{}/api/collections/{}/records/{}",
                pb_cfg.url.trim_end_matches('/'),
                collection,
                id
            )
        },
    );

    let resp = if let Some(url_val) = image_url {
        if let Some(url_str) = url_val.as_str() {
            match http.get_bounded_bytes(url_str, limit, false).await {
                Ok(bytes) => {
                    if record_id.is_some() {
                        http.patch_multipart_with_auth(
                            &url,
                            auth.as_deref(),
                            &clean_payload,
                            "cover.png",
                            bytes.to_vec(),
                            limit,
                        )
                        .await?
                    } else {
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
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch image from {}, falling back to JSON post: {}",
                        url_str,
                        e
                    );
                    if record_id.is_some() {
                        http.patch_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                            .await?
                    } else {
                        http.post_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                            .await?
                    }
                }
            }
        } else if record_id.is_some() {
            http.patch_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                .await?
        } else {
            http.post_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                .await?
        }
    } else {
        if record_id.is_some() {
            http.patch_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                .await?
        } else {
            http.post_json_with_auth(&url, auth.as_deref(), &clean_payload, limit)
                .await?
        }
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

    fn test_pb_cfg() -> PocketBaseConfig {
        PocketBaseConfig {
            url: "https://pb.example.com".to_string(),
            collection: "events".to_string(),
            locations_collection: None,
        }
    }

    #[tokio::test]
    async fn test_push_record_data_success() {
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

        let pb_cfg = test_pb_cfg();

        std::env::set_var("POCKETBASE_IMPERSONATE_AUTH_TOKEN", "secret_token");
        let payload = serde_json::json!({"title": "Test"});
        let res = push_record_data(&mock_http, &pb_cfg, "events", &payload, 1024 * 1024).await;
        std::env::remove_var("POCKETBASE_IMPERSONATE_AUTH_TOKEN");
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "9ket96ibdt566u6");
    }

    #[tokio::test]
    async fn test_push_record_data_targets_locations_collection() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_post_json_with_auth()
            .with(
                mockall::predicate::eq("https://pb.example.com/api/collections/locations/records"),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(r#"{"id":"abcdefghij12345"}"#.to_string()));

        let pb_cfg = test_pb_cfg();

        let payload = serde_json::json!({"title": "Legokauppa"});
        let res = push_record_data(
            &mock_http,
            &pb_cfg,
            pb_cfg.locations_collection(),
            &payload,
            1024 * 1024,
        )
        .await;
        assert_eq!(res.unwrap(), "abcdefghij12345");
    }

    #[tokio::test]
    async fn test_push_record_data_patches_existing_location() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_patch_json_with_auth()
            .with(
                mockall::predicate::eq(
                    "https://pb.example.com/api/collections/locations/records/abcdefghij12345",
                ),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(r#"{"id":"abcdefghij12345"}"#.to_string()));

        let pb_cfg = test_pb_cfg();

        let payload = serde_json::json!({"id": "abcdefghij12345", "title": "Legokauppa"});
        let res = push_record_data(&mock_http, &pb_cfg, "locations", &payload, 1024 * 1024).await;
        assert_eq!(res.unwrap(), "abcdefghij12345");
    }

    #[tokio::test]
    async fn test_push_record_data_error() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_post_json_with_auth()
            .times(1)
            .returning(|_, _, _, _| Err(anyhow::anyhow!("Network error")));

        let pb_cfg = test_pb_cfg();

        std::env::remove_var("POCKETBASE_IMPERSONATE_AUTH_TOKEN");
        let payload = serde_json::json!({"title": "Test"});
        let res = push_record_data(&mock_http, &pb_cfg, "events", &payload, 1024 * 1024).await;
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().to_string(), "Network error");
    }
}
