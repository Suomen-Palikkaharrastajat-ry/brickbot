use crate::config::ZulipConfig;

use super::SendMessageResponse;

pub async fn post_topic_to_stream(
    http: &dyn crate::http::HttpProvider,
    zulip_cfg: &ZulipConfig,
    stream: &str,
    topic: &str,
    content: &str,
    limit: u64,
) -> anyhow::Result<()> {
    let url = format!("{}/api/v1/messages", zulip_cfg.url.trim_end_matches('/'));

    let form = vec![
        ("type".to_string(), "stream".to_string()),
        ("to".to_string(), stream.to_string()),
        ("topic".to_string(), topic.to_string()),
        ("content".to_string(), content.to_string()),
    ];

    let api_key = std::env::var("ZULIP_API_KEY").unwrap_or_default();
    http.post_form_basic_auth(&url, &zulip_cfg.bot_email, Some(&api_key), form, limit)
        .await?;

    Ok(())
}

pub async fn resolve_zulip_topic(
    http: &dyn crate::http::HttpProvider,
    zulip_cfg: &ZulipConfig,
    current_topic: &str,
    stream: &str,
    custom_msg: &str,
    limit: u64,
) -> anyhow::Result<()> {
    if current_topic.starts_with("✔ ") {
        return Ok(());
    }

    let send_url = format!("{}/api/v1/messages", zulip_cfg.url.trim_end_matches('/'));

    let form = vec![
        ("type".to_string(), "stream".to_string()),
        ("to".to_string(), stream.to_string()),
        ("topic".to_string(), current_topic.to_string()),
        ("content".to_string(), custom_msg.to_string()),
    ];

    let api_key = std::env::var("ZULIP_API_KEY").unwrap_or_default();
    let res_text = http
        .post_form_basic_auth(&send_url, &zulip_cfg.bot_email, Some(&api_key), form, limit)
        .await?;

    let resp_data: SendMessageResponse = serde_json::from_str(&res_text)?;
    let message_id = resp_data.id;

    // 2. Patch the bot's own message to rename the entire topic
    let patch_url = format!(
        "{}/api/v1/messages/{}",
        zulip_cfg.url.trim_end_matches('/'),
        message_id
    );
    let new_topic = format!("✔ {current_topic}");

    let patch_form = vec![
        ("topic".to_string(), new_topic),
        ("propagate_mode".to_string(), "change_all".to_string()),
    ];

    http.patch_form_basic_auth(
        &patch_url,
        &zulip_cfg.bot_email,
        Some(&api_key),
        patch_form,
        limit,
    )
    .await?;

    Ok(())
}

pub async fn unresolve_zulip_topic(
    http: &dyn crate::http::HttpProvider,
    zulip_cfg: &ZulipConfig,
    base_topic: &str,
    limit: u64,
) -> anyhow::Result<()> {
    let resolved_topic = format!("✔ {base_topic}");
    let narrow = serde_json::json!([
        {"operator": "topic", "operand": resolved_topic}
    ])
    .to_string();

    let api_key = std::env::var("ZULIP_API_KEY").unwrap_or_default();
    if let Ok(url) = reqwest::Url::parse_with_params(
        &format!("{}/api/v1/messages", zulip_cfg.url.trim_end_matches('/')),
        &[
            ("narrow", &narrow),
            ("anchor", &"oldest".to_string()),
            ("num_before", &"0".to_string()),
            ("num_after", &"1".to_string()),
        ],
    ) {
        let messages_url = url.to_string();
        if let Ok(messages_resp) = http
            .get_text_basic_auth(&messages_url, &zulip_cfg.bot_email, Some(&api_key), limit)
            .await
        {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&messages_resp) {
                if let Some(messages) = json.get("messages").and_then(|m| m.as_array()) {
                    if let Some(first_msg) = messages.first() {
                        if let Some(first_msg_id) =
                            first_msg.get("id").and_then(serde_json::Value::as_u64)
                        {
                            let patch_url = format!(
                                "{}/api/v1/messages/{}",
                                zulip_cfg.url.trim_end_matches('/'),
                                first_msg_id
                            );
                            let patch_form = vec![
                                ("topic".to_string(), base_topic.to_string()),
                                ("propagate_mode".to_string(), "change_all".to_string()),
                            ];
                            let _ = http
                                .patch_form_basic_auth(
                                    &patch_url,
                                    &zulip_cfg.bot_email,
                                    Some(&api_key),
                                    patch_form,
                                    limit,
                                )
                                .await;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::MockHttpProvider;

    #[tokio::test]
    async fn test_post_topic_to_stream_success() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_post_form_basic_auth()
            .with(
                mockall::predicate::eq("https://zulip.example.com/api/v1/messages"),
                mockall::predicate::eq("bot@example.com"),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok(r#"{"result":"success"}"#.to_string()));

        let zulip_cfg = ZulipConfig {
            url: "https://zulip.example.com".to_string(),
            bot_email: "bot@example.com".to_string(),
            moderation_stream: "mod".to_string(),
            moderators: vec![],
        };

        std::env::set_var("ZULIP_API_KEY", "secret");
        let res = post_topic_to_stream(
            &mock_http,
            &zulip_cfg,
            "mod",
            "test_topic",
            "hello world",
            1024 * 1024,
        )
        .await;
        std::env::remove_var("ZULIP_API_KEY");

        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_resolve_topic_success() {
        let mut mock_http = MockHttpProvider::new();
        mock_http
            .expect_post_form_basic_auth()
            .with(
                mockall::predicate::eq("https://zulip.example.com/api/v1/messages"),
                mockall::predicate::eq("bot@example.com"),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok(r#"{"result":"success","id":42}"#.to_string()));

        mock_http
            .expect_patch_form_basic_auth()
            .with(
                mockall::predicate::eq("https://zulip.example.com/api/v1/messages/42"),
                mockall::predicate::eq("bot@example.com"),
                mockall::predicate::always(),
                mockall::predicate::always(),
                mockall::predicate::always(),
            )
            .times(1)
            .returning(|_, _, _, _, _| Ok(r#"{"result":"success"}"#.to_string()));

        let zulip_cfg = ZulipConfig {
            url: "https://zulip.example.com".to_string(),
            bot_email: "bot@example.com".to_string(),
            moderation_stream: "mod".to_string(),
            moderators: vec![],
        };

        std::env::set_var("ZULIP_API_KEY", "secret");
        let res = resolve_zulip_topic(
            &mock_http,
            &zulip_cfg,
            "test_topic",
            "mod",
            "Marking topic as resolved...",
            1024 * 1024,
        )
        .await;
        std::env::remove_var("ZULIP_API_KEY");

        assert!(res.is_ok());
    }
}
