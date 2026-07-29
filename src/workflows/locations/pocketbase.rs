use crate::config::PocketBaseConfig;
use crate::workflows::locations::models::{Location, PbLocationResponse};

/// Fetches every published record from the `PocketBase` `locations` collection.
///
/// Mirrors `events_sync::fetch_pocketbase_events`: hand-encoded filter, `perPage=100`
/// pagination, and cache-bypassing reads so moderators always see current data.
pub async fn fetch_pocketbase_locations(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &PocketBaseConfig,
    limit: u64,
) -> anyhow::Result<Vec<Location>> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let collection = pb_cfg.locations_collection();
    // Pre-encoded `(state='published')`.
    let filter = "(state=%27published%27)";

    let mut current_page = 1;
    let mut all_locations = Vec::new();

    loop {
        let url = format!(
            "{}/api/collections/{}/records?page={}&perPage=100&filter={}",
            pb_cfg.url.trim_end_matches('/'),
            collection,
            current_page,
            filter
        );

        let res_text = http
            .get_json_with_auth(&url, auth.as_deref(), true, limit)
            .await?;
        let pb_resp: PbLocationResponse = serde_json::from_str(&res_text)?;

        for item in pb_resp.items {
            all_locations.push(item.into_location(&pb_cfg.url, collection));
        }

        if current_page >= pb_resp.total_pages {
            break;
        }
        current_page += 1;
    }

    Ok(all_locations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::MockHttpProvider;

    fn pb_cfg() -> PocketBaseConfig {
        PocketBaseConfig {
            url: "https://pb.example.com".to_string(),
            collection: "events".to_string(),
            locations_collection: None,
        }
    }

    #[tokio::test]
    async fn test_fetch_pocketbase_locations_paginates() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_get_json_with_auth()
            .times(2)
            .returning(|url, _, skip_cache, _| {
                assert!(url.contains("/api/collections/locations/records"));
                assert!(url.contains("perPage=100"));
                assert!(skip_cache, "PocketBase reads must bypass the cache");
                if url.contains("page=1") {
                    Ok(r#"{"items":[{"id":"aaaaaaaaaaaaaaa","title":"Kauppa","point":{"lat":60.1,"lon":24.9},"updated":"2026-07-01 10:00:00.000Z"}],"page":1,"totalPages":2}"#.to_string())
                } else {
                    Ok(r#"{"items":[{"id":"bbbbbbbbbbbbbbb","title":"Museo","updated":"2026-07-02 10:00:00.000Z"}],"page":2,"totalPages":2}"#.to_string())
                }
            });

        let locations = fetch_pocketbase_locations(&mock_http, &pb_cfg(), 1024 * 1024)
            .await
            .unwrap();

        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0].title, "Kauppa");
        assert!(locations[0].point.is_some());
        assert_eq!(locations[1].title, "Museo");
        assert!(locations[1].point.is_none());
    }

    #[tokio::test]
    async fn test_fetch_pocketbase_locations_uses_configured_collection() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_get_json_with_auth()
            .times(1)
            .returning(|url, _, _, _| {
                assert!(url.contains("/api/collections/map_places/records"));
                Ok(r#"{"items":[],"page":1,"totalPages":1}"#.to_string())
            });

        let mut cfg = pb_cfg();
        cfg.locations_collection = Some("map_places".to_string());

        let locations = fetch_pocketbase_locations(&mock_http, &cfg, 1024 * 1024)
            .await
            .unwrap();
        assert!(locations.is_empty());
    }
}
