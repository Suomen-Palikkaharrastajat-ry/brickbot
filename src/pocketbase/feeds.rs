use crate::config::PocketBaseConfig;
use crate::http::HttpProvider;
use crate::rss::FeedEntry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct RemoteFeed {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub site_url: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub feed_type: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub poll_interval_mins: Option<u64>,
    #[serde(default)]
    pub exclude_keywords: Vec<String>,
    #[serde(default)]
    pub exclude_urls: Vec<String>,
}

#[allow(dead_code)]
pub type RemoteFeedConfig = RemoteFeed;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub struct PocketBaseFeedItem {
    pub id: String,
    #[serde(default)]
    pub feed: Option<String>,
    #[serde(default)]
    pub guid: Option<String>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub is_hidden: bool,
    #[serde(default)]
    pub hidden_reason: Option<String>,
    #[serde(default)]
    pub hidden_at: Option<String>,
    #[serde(default)]
    pub hidden_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertFeedItemResult {
    pub id: String,
    pub is_hidden: bool,
}

pub type FeedItemRow = crate::db::FeedItem;
pub type FeedItemRawRow = (String, String, String, String);

#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
struct PocketBaseListResponse<T> {
    #[serde(default)]
    pub items: Vec<T>,
    #[serde(default)]
    #[allow(dead_code)]
    pub total_items: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub total_pages: Option<u64>,
}

fn detect_media_type(id: &str) -> &'static str {
    if id.contains("youtube.com") || id.contains("youtu.be") {
        "video"
    } else if id.contains("flickr.com") || id.contains("kuvat.fi") {
        "image"
    } else {
        "article"
    }
}

/// Fetches all active/enabled feeds from `PocketBase`.
pub async fn fetch_active_feeds(
    http: &dyn HttpProvider,
    pb_cfg: &PocketBaseConfig,
    limit: u64,
) -> anyhow::Result<Vec<RemoteFeed>> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let url = format!(
        "{}/api/collections/{}/records?filter=(enabled=true)&perPage=500",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feeds_collection()
    );

    let body = http
        .get_json_with_auth(&url, auth.as_deref(), true, limit)
        .await?;
    let resp: PocketBaseListResponse<RemoteFeed> = serde_json::from_str(&body)?;
    Ok(resp.items)
}

/// Upserts a feed item into `PocketBase`, respecting existing moderation (`is_hidden = true`).
pub async fn upsert_feed_item(
    http: &dyn HttpProvider,
    pb_cfg: &PocketBaseConfig,
    limit: u64,
    feed_id: &str,
    item: &FeedEntry,
) -> anyhow::Result<UpsertFeedItemResult> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let check_url = format!(
        "{}/api/collections/{}/records?filter=(feed='{}'%26%26guid='{}')",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feed_items_collection(),
        urlencoding::encode(feed_id),
        urlencoding::encode(&item.id)
    );

    let body = http
        .get_json_with_auth(&check_url, auth.as_deref(), true, limit)
        .await?;
    let check_resp: serde_json::Value = serde_json::from_str(&body)?;

    if let Some(existing) = check_resp
        .get("items")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
    {
        let existing_id = existing["id"].as_str().unwrap_or_default().to_string();
        let is_hidden = existing["is_hidden"].as_bool().unwrap_or(false);

        if is_hidden {
            return Ok(UpsertFeedItemResult {
                id: existing_id,
                is_hidden: true,
            });
        }

        let patch_url = format!(
            "{}/api/collections/{}/records/{}",
            pb_cfg.url.trim_end_matches('/'),
            pb_cfg.feed_items_collection(),
            existing_id
        );
        let patch_payload = serde_json::json!({
            "title": item.title,
            "description": item.description,
            "thumbnail": item.thumbnail,
        });
        let _ = http
            .patch_json_with_auth(&patch_url, auth.as_deref(), &patch_payload, limit)
            .await;

        return Ok(UpsertFeedItemResult {
            id: existing_id,
            is_hidden: false,
        });
    }

    let create_url = format!(
        "{}/api/collections/{}/records",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feed_items_collection()
    );
    let media_type = detect_media_type(&item.id);
    let snippet = crate::rss::one_line(&item.description)
        .chars()
        .take(500)
        .collect::<String>();
    let published_at = item
        .published
        .map_or_else(|| chrono::Utc::now().to_rfc3339(), |dt| dt.to_rfc3339());

    let create_payload = serde_json::json!({
        "feed": feed_id,
        "guid": item.id,
        "link": item.id,
        "title": item.title,
        "description": item.description,
        "snippet": snippet,
        "published_at": published_at,
        "thumbnail": item.thumbnail,
        "media_type": media_type,
        "is_hidden": false
    });

    let resp_str = http
        .post_json_with_auth(&create_url, auth.as_deref(), &create_payload, limit)
        .await?;
    let resp_val: serde_json::Value = serde_json::from_str(&resp_str)?;
    let id = resp_val["id"].as_str().unwrap_or_default().to_string();

    Ok(UpsertFeedItemResult {
        id,
        is_hidden: false,
    })
}

/// Searches unhidden feed items from `PocketBase` and filters against LEGO set parameters.
pub async fn search_feed_items(
    http: &dyn HttpProvider,
    pb_cfg: &PocketBaseConfig,
    limit: u64,
    set_number: &str,
    set_name: &str,
    set_theme: &str,
) -> anyhow::Result<Vec<FeedItemRow>> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let url = format!(
        "{}/api/collections/{}/records?filter=(is_hidden=false)&perPage=100&expand=feed",
        pb_cfg.url.trim_end_matches('/'),
        pb_cfg.feed_items_collection()
    );

    let body = http
        .get_json_with_auth(&url, auth.as_deref(), false, limit)
        .await?;
    let resp: serde_json::Value = serde_json::from_str(&body)?;

    let mut rows: Vec<FeedItemRawRow> = Vec::new();
    if let Some(items) = resp.get("items").and_then(|v| v.as_array()) {
        for it in items {
            let id = it
                .get("link")
                .and_then(|v| v.as_str())
                .or_else(|| it.get("guid").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .to_string();
            let source_title = it
                .get("expand")
                .and_then(|v| v.get("feed"))
                .and_then(|f| f.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let item_title = it
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let item_desc = it
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            rows.push((id, source_title, item_title, item_desc));
        }
    }

    Ok(crate::db::feed::filter_feed_items(
        rows, set_number, set_name, set_theme,
    ))
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
            feeds_collection: None,
            feed_items_collection: None,
        }
    }

    #[tokio::test]
    async fn test_fetch_active_feeds_success() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_get_json_with_auth()
            .with(
                mockall::predicate::eq(
                    "https://pb.example.com/api/collections/feeds/records?filter=(enabled=true)&perPage=500",
                ),
                mockall::predicate::always(),
                mockall::predicate::eq(true),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| {
                Ok(r#"{
                    "items": [
                        {
                            "id": "feed1",
                            "url": "https://example.com/rss.xml",
                            "title": "Example Feed",
                            "enabled": true,
                            "exclude_keywords": ["haul", "sale"],
                            "exclude_urls": ["https://example.com/skip.html"]
                        },
                        {
                            "id": "feed2",
                            "url": "https://youtube.com/channel/abc",
                            "title": "Example Video",
                            "enabled": true,
                            "exclude_keywords": [],
                            "exclude_urls": []
                        }
                    ],
                    "total_items": 2
                }"#
                .to_string())
            });

        let pb = test_pb_cfg();
        let feeds = fetch_active_feeds(&mock_http, &pb, 1024 * 1024)
            .await
            .unwrap();

        assert_eq!(feeds.len(), 2);
        assert_eq!(feeds[0].id, "feed1");
        assert_eq!(feeds[0].url, "https://example.com/rss.xml");
        assert_eq!(feeds[0].title.as_deref(), Some("Example Feed"));
        assert!(feeds[0].enabled);
        assert_eq!(feeds[0].exclude_keywords, vec!["haul", "sale"]);
        assert_eq!(feeds[0].exclude_urls, vec!["https://example.com/skip.html"]);

        assert_eq!(feeds[1].id, "feed2");
        assert_eq!(feeds[1].url, "https://youtube.com/channel/abc");
    }

    #[tokio::test]
    async fn test_upsert_feed_item_creates_new() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_get_json_with_auth()
            .with(
                mockall::predicate::eq(
                    "https://pb.example.com/api/collections/feed_items/records?filter=(feed='feed1'%26%26guid='https%3A%2F%2Fexample.com%2Fpost-1')",
                ),
                mockall::predicate::always(),
                mockall::predicate::eq(true),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(r#"{"items": []}"#.to_string()));

        mock_http
            .expect_post_json_with_auth()
            .with(
                mockall::predicate::eq("https://pb.example.com/api/collections/feed_items/records"),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, payload, _| {
                assert_eq!(payload["feed"], "feed1");
                assert_eq!(payload["guid"], "https://example.com/post-1");
                assert_eq!(payload["title"], "New Post");
                assert_eq!(payload["media_type"], "article");
                assert_eq!(payload["is_hidden"], false);
                Ok(r#"{"id": "newitem12345"}"#.to_string())
            });

        let pb = test_pb_cfg();
        let entry = FeedEntry {
            id: "https://example.com/post-1".to_string(),
            source_title: "My Blog".to_string(),
            title: "New Post".to_string(),
            description: "Some post content".to_string(),
            published: None,
            thumbnail: None,
        };

        let res = upsert_feed_item(&mock_http, &pb, 1024 * 1024, "feed1", &entry)
            .await
            .unwrap();

        assert_eq!(res.id, "newitem12345");
        assert!(!res.is_hidden);
    }

    #[tokio::test]
    async fn test_upsert_feed_item_preserves_is_hidden_true() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_get_json_with_auth()
            .with(
                mockall::predicate::eq(
                    "https://pb.example.com/api/collections/feed_items/records?filter=(feed='feed1'%26%26guid='https%3A%2F%2Fexample.com%2Fmod-1')",
                ),
                mockall::predicate::always(),
                mockall::predicate::eq(true),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| {
                Ok(r#"{
                    "items": [
                        {
                            "id": "mod12345",
                            "feed": "feed1",
                            "guid": "https://example.com/mod-1",
                            "title": "Spam Article",
                            "is_hidden": true,
                            "hidden_reason": "spam"
                        }
                    ]
                }"#
                .to_string())
            });

        // Ensure neither POST nor PATCH is invoked!
        mock_http.expect_post_json_with_auth().times(0);
        mock_http.expect_patch_json_with_auth().times(0);

        let pb = test_pb_cfg();
        let entry = FeedEntry {
            id: "https://example.com/mod-1".to_string(),
            source_title: "Blog".to_string(),
            title: "Spam Article Updated".to_string(),
            description: "Spam description".to_string(),
            published: None,
            thumbnail: None,
        };

        let res = upsert_feed_item(&mock_http, &pb, 1024 * 1024, "feed1", &entry)
            .await
            .unwrap();

        assert_eq!(res.id, "mod12345");
        assert!(res.is_hidden);
    }

    #[tokio::test]
    async fn test_upsert_feed_item_updates_existing_visible() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_get_json_with_auth()
            .with(
                mockall::predicate::eq(
                    "https://pb.example.com/api/collections/feed_items/records?filter=(feed='feed1'%26%26guid='https%3A%2F%2Fexample.com%2Fexisting-1')",
                ),
                mockall::predicate::always(),
                mockall::predicate::eq(true),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| {
                Ok(r#"{
                    "items": [
                        {
                            "id": "exist12345",
                            "feed": "feed1",
                            "guid": "https://example.com/existing-1",
                            "title": "Old Title",
                            "is_hidden": false
                        }
                    ]
                }"#
                .to_string())
            });

        mock_http
            .expect_patch_json_with_auth()
            .with(
                mockall::predicate::eq(
                    "https://pb.example.com/api/collections/feed_items/records/exist12345",
                ),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(r#"{"id": "exist12345"}"#.to_string()));

        let pb = test_pb_cfg();
        let entry = FeedEntry {
            id: "https://example.com/existing-1".to_string(),
            source_title: "Blog".to_string(),
            title: "Updated Title".to_string(),
            description: "Updated description".to_string(),
            published: None,
            thumbnail: None,
        };

        let res = upsert_feed_item(&mock_http, &pb, 1024 * 1024, "feed1", &entry)
            .await
            .unwrap();

        assert_eq!(res.id, "exist12345");
        assert!(!res.is_hidden);
    }

    #[tokio::test]
    async fn test_upsert_feed_item_encodes_special_characters() {
        let mut mock_http = MockHttpProvider::new();
        // feed_id: "feed's&special" -> "feed%27s%26special"
        // guid: "https://example.com/item?id=42&title=L'espace" -> "https%3A%2F%2Fexample.com%2Fitem%3Fid%3D42%26title%3DL%27espace"
        let expected_filter = "https://pb.example.com/api/collections/feed_items/records?filter=(feed='feed%27s%26special'%26%26guid='https%3A%2F%2Fexample.com%2Fitem%3Fid%3D42%26title%3DL%27espace')";
        mock_http
            .expect_get_json_with_auth()
            .with(
                mockall::predicate::eq(expected_filter),
                mockall::predicate::always(),
                mockall::predicate::eq(true),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| Ok(r#"{"items": []}"#.to_string()));

        mock_http
            .expect_post_json_with_auth()
            .times(1)
            .returning(|_, _, _, _| Ok(r#"{"id": "special_item_1"}"#.to_string()));

        let pb = test_pb_cfg();
        let entry = FeedEntry {
            id: "https://example.com/item?id=42&title=L'espace".to_string(),
            source_title: "Special Feed".to_string(),
            title: "L'espace Special".to_string(),
            description: "Post with special chars".to_string(),
            published: None,
            thumbnail: None,
        };

        let res = upsert_feed_item(&mock_http, &pb, 1024 * 1024, "feed's&special", &entry)
            .await
            .unwrap();

        assert_eq!(res.id, "special_item_1");
        assert!(!res.is_hidden);
    }

    #[tokio::test]
    async fn test_search_feed_items_pocketbase() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_get_json_with_auth()
            .with(
                mockall::predicate::eq(
                    "https://pb.example.com/api/collections/feed_items/records?filter=(is_hidden=false)&perPage=100&expand=feed",
                ),
                mockall::predicate::always(),
                mockall::predicate::eq(false),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _| {
                Ok(r#"{
                    "items": [
                        {
                            "id": "item1",
                            "link": "https://example.com/bugatti-review",
                            "title": "LEGO Technic 42083 Bugatti Chiron Review",
                            "description": "Comprehensive review of the Bugatti set.",
                            "expand": {
                                "feed": {
                                    "title": "Technic Fans"
                                }
                            }
                        },
                        {
                            "id": "item2",
                            "link": "https://example.com/star-wars-haul",
                            "title": "Star Wars 75192 Millennium Falcon Haul",
                            "description": "UCS Falcon unboxing.",
                            "expand": {
                                "feed": {
                                    "title": "Brick Portal"
                                }
                            }
                        }
                    ]
                }"#
                .to_string())
            });

        let pb = test_pb_cfg();
        let results = search_feed_items(
            &mock_http,
            &pb,
            1024 * 1024,
            "42083",
            "Bugatti Chiron",
            "Technic",
        )
        .await
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "https://example.com/bugatti-review");
        assert_eq!(results[0].source_title, "Technic Fans");
        assert_eq!(
            results[0].item_title,
            "LEGO Technic 42083 Bugatti Chiron Review"
        );
    }
}
