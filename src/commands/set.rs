#![allow(unused_imports)]
use crate::brick::{BricksetSet, fetch_set};
use rust_i18n::t;
use serenity::all::{Context, CreateEmbed, Framework};

use sqlx::SqlitePool;

pub enum SetInteractionResponse {
    DirectMatch(String, Box<CreateEmbed>),
    SearchResults(Vec<serenity::all::CreateSelectMenuOption>),
}

pub async fn set_interaction(
    http: &dyn crate::http::HttpProvider,
    db: &SqlitePool,
    query: &str,
    locale: &str,
    services: &[String],
    limit: u64,
) -> anyhow::Result<SetInteractionResponse> {
    let clean_query = query.trim();

    let first_word = clean_query.split_whitespace().next().unwrap_or("");
    if first_word.chars().any(|c| c.is_ascii_digit()) {
        if let Ok(set) = fetch_set(http, first_word, limit).await {
            let articles = crate::db::search_feed_items(db, &set.number, &set.name, &set.theme)
                .await
                .unwrap_or_default();
            let (content, embed) = build_set_message(&set, locale, services, &articles);
            if let Some(e) = embed {
                return Ok(SetInteractionResponse::DirectMatch(content, Box::new(e)));
            }
        }
    }

    match crate::brick::search_sets(http, clean_query, limit).await {
        Ok(results) => {
            if results.is_empty() {
                Err(anyhow::anyhow!("No sets found matching query"))
            } else if results.len() == 1 {
                let s_id = format!("{}-{}", results[0].number, results[0].numberVariant);
                if let Ok(set) = fetch_set(http, &s_id, limit).await {
                    let articles =
                        crate::db::search_feed_items(db, &set.number, &set.name, &set.theme)
                            .await
                            .unwrap_or_default();
                    let (content, embed) = build_set_message(&set, locale, services, &articles);
                    if let Some(e) = embed {
                        return Ok(SetInteractionResponse::DirectMatch(content, Box::new(e)));
                    }
                }
                Err(anyhow::anyhow!("Failed to fetch set details"))
            } else {
                let options: Vec<serenity::all::CreateSelectMenuOption> = results
                    .into_iter()
                    .take(25)
                    .map(|r| {
                        let set_id = format!("{}-{}", r.number, r.numberVariant);
                        let label = format!("{} - {}", set_id, r.name);
                        let truncated_label = if label.len() > 100 {
                            format!("{}...", &label[..97])
                        } else {
                            label
                        };
                        serenity::all::CreateSelectMenuOption::new(truncated_label, set_id)
                    })
                    .collect();
                Ok(SetInteractionResponse::SearchResults(options))
            }
        }
        Err(e) => {
            tracing::error!("Failed to fetch/search set {}: {}", clean_query, e);
            Err(anyhow::anyhow!("Failed to fetch set"))
        }
    }
}

fn build_set_message(
    set: &BricksetSet,
    locale: &str,
    services: &[String],
    articles: &[crate::db::FeedItem],
) -> (String, Option<CreateEmbed>) {
    let set_id = format!("{}-{}", set.number, set.numberVariant);

    let mut builder = CreateEmbed::new()
        .title(t!(
            "command.set.title",
            locale = locale,
            name = &set.name,
            id = &set_id
        ))
        .field(
            t!("command.set.year", locale = locale),
            set.year.to_string(),
            true,
        )
        .field(
            t!("command.set.theme", locale = locale),
            set.theme.clone(),
            true,
        )
        .field(
            t!("command.set.pieces", locale = locale),
            set.pieces.map_or_else(
                || t!("common.na", locale = locale).to_string(),
                |p| p.to_string(),
            ),
            true,
        );

    if let Some(subtheme) = &set.subtheme {
        builder = builder.field(t!("command.set.subtheme", locale = locale), subtheme, true);
    }

    if let Some(rating) = set.rating {
        builder = builder.field(
            t!("command.set.rating", locale = locale),
            format!("{rating:.1}/5.0"),
            true,
        );
    }

    if let Some(img) = set.image.as_ref().and_then(|i| i.imageURL.clone()) {
        builder = builder.image(img);
    }

    if let Some(url) = &set.bricksetURL {
        builder = builder.url(url);
    }

    let mut links_text = Vec::new();

    if services.contains(&"bricklink".to_string()) {
        links_text.push(format!(
            "**{}**: {}",
            t!("command.set.bricklink", locale = locale),
            crate::links::bricklink::set_url(&set_id)
        ));
    }
    if services.contains(&"brickset".to_string()) {
        links_text.push(format!(
            "**Brickset**: {}",
            crate::links::brickset::set_url(&set_id)
        ));
    }
    if services.contains(&"lego".to_string()) {
        links_text.push(format!(
            "**{}**: {}",
            t!("command.set.legocom", locale = locale),
            crate::links::lego::search_url(&set.number)
        ));
    }
    if services.contains(&"rebrickable".to_string()) {
        links_text.push(format!(
            "**Rebrickable**: {}",
            crate::links::rebrickable::set_url(&set_id)
        ));
    }

    if services.contains(&"articles".to_string()) && !articles.is_empty() {
        links_text.push(format!(
            "\n**{}**:",
            t!(
                "command.set.related_articles",
                locale = locale,
                default = "Related Articles"
            )
        ));
        for article in articles {
            links_text.push(format!(
                "- [{}]({}) ({})",
                article.item_title, article.id, article.source_title
            ));
        }
    }

    let content = if links_text.is_empty() {
        String::new()
    } else {
        links_text.join(" \n")
    };

    let final_embed = Some(builder);

    (content, final_embed)
}

pub fn build_set_command(locale: &str) -> serenity::all::CreateCommand {
    use serenity::all::{CommandOptionType, CreateCommand, CreateCommandOption};

    let cmd_name = rust_i18n::t!("command.set.name", locale = locale).to_string();
    let cmd_desc = rust_i18n::t!("command.set.desc", locale = locale).to_string();
    let set_arg_name = rust_i18n::t!("command.set.set_arg_name", locale = locale).to_string();
    let set_desc = rust_i18n::t!("command.set.set_desc", locale = locale).to_string();

    let set_option = CreateCommandOption::new(CommandOptionType::String, &set_arg_name, &set_desc)
        .required(true);

    CreateCommand::new(&cmd_name)
        .description(&cmd_desc)
        .add_option(set_option)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_set_message_en() {
        let set = BricksetSet {
            number: "42083".to_string(),
            numberVariant: 1,
            name: "Bugatti Chiron".to_string(),
            year: 2018,
            theme: "Technic".to_string(),
            subtheme: Some("Ultimate Car Concept".to_string()),
            pieces: Some(3599),
            image: Some(crate::brick::BricksetImage {
                thumbnailURL: None,
                imageURL: Some("http://example.com/image.png".to_string()),
            }),
            bricksetURL: Some("https://brickset.com/sets/42083-1".to_string()),
            rating: Some(4.8),
        };

        let (content, embed) = build_set_message(
            &set,
            "en-US",
            &[
                "bricklink".to_string(),
                "lego".to_string(),
                "articles".to_string(),
            ],
            &[],
        );
        assert!(
            content.contains("https://www.bricklink.com/v2/catalog/catalogitem.page?S=42083-1")
        );
        assert!(content.contains("https://www.lego.com/fi-fi/search?q=42083"));

        let embed_json = serde_json::to_value(embed.unwrap()).unwrap();
        assert_eq!(embed_json["title"], "Set: Bugatti Chiron (42083-1)");
        assert_eq!(embed_json["image"]["url"], "http://example.com/image.png");
        assert_eq!(embed_json["url"], "https://brickset.com/sets/42083-1");
    }

    #[test]
    fn test_build_set_message_fi() {
        let set = BricksetSet {
            number: "10281".to_string(),
            numberVariant: 1,
            name: "Bonsai Tree".to_string(),
            year: 2021,
            theme: "Botanical Collection".to_string(),
            subtheme: None,
            pieces: None,
            image: None,
            bricksetURL: None,
            rating: None,
        };

        let (content, embed) = build_set_message(
            &set,
            "fi-FI",
            &[
                "bricklink".to_string(),
                "lego".to_string(),
                "articles".to_string(),
            ],
            &[],
        );
        assert!(content.contains("BrickLink"));
        assert!(content.contains("LEGO.com"));

        let embed_json = serde_json::to_value(embed.unwrap()).unwrap();
        assert_eq!(embed_json["title"], "Setti: Bonsai Tree (10281-1)");
        let mut has_pieces = false;
        for field in embed_json["fields"].as_array().unwrap() {
            if field["name"] == "Osia" && field["value"] == "N/A" {
                has_pieces = true;
            }
        }
        assert!(has_pieces);
    }

    #[tokio::test]
    async fn test_set_interaction_direct_match() {
        let mut mock_http = crate::http::MockHttpProvider::new();
        mock_http.expect_post_form()
            .times(1)
            .returning(|_, _, _| {
                Ok(r#"{"status": "success", "sets": [{"number": "42083", "numberVariant": 1, "name": "Bugatti", "year": 2018, "theme": "Technic"}]}"#.to_string())
            });

        std::env::set_var("BRICKSET_API_KEY", "test");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();

        let result = set_interaction(&mock_http, &pool, "42083", "en-US", &[], 1024 * 1024).await;
        assert!(result.is_ok());
        match result.unwrap() {
            SetInteractionResponse::DirectMatch(_, embed) => {
                let embed_json = serde_json::to_value(*embed).unwrap();
                assert_eq!(embed_json["title"], "Set: Bugatti (42083-1)");
            }
            _ => panic!("Expected DirectMatch"),
        }
    }

    #[tokio::test]
    async fn test_set_interaction_search_results() {
        let mut mock_http = crate::http::MockHttpProvider::new();
        mock_http.expect_post_form()
            .times(1)
            .returning(|_, _, _| {
                Ok(r#"{"status": "success", "sets": [{"number": "42083", "numberVariant": 1, "name": "Bugatti", "year": 2018, "theme": "Technic"}, {"number": "42083", "numberVariant": 2, "name": "Bugatti V2", "year": 2019, "theme": "Technic"}]}"#.to_string())
            });

        std::env::set_var("BRICKSET_API_KEY", "test");
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();

        let result = set_interaction(&mock_http, &pool, "Bugatti", "en-US", &[], 1024 * 1024).await;
        assert!(result.is_ok());
        match result.unwrap() {
            SetInteractionResponse::SearchResults(options) => {
                assert_eq!(options.len(), 2);
                let json_0 = serde_json::to_value(&options[0]).unwrap();
                let json_1 = serde_json::to_value(&options[1]).unwrap();
                assert_eq!(json_0["value"], "42083-1");
                assert_eq!(json_1["value"], "42083-2");
            }
            _ => panic!("Expected SearchResults"),
        }
    }
}
