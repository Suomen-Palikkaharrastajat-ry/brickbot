#![allow(unused_imports)]
use crate::brick::fetch_part;
use chrono::Datelike;
use rust_i18n::t;
use serenity::all::{Context, CreateEmbed, Framework};

pub enum PartInteractionResponse {
    DirectMatch(String, Box<CreateEmbed>),
    SearchResults(Vec<serenity::all::CreateSelectMenuOption>),
}

pub async fn part_interaction(
    _ctx: &Context,
    http: &dyn crate::http::HttpProvider,
    query: &str,
    locale: &str,
    services: &[String],
    limit: u64,
) -> anyhow::Result<PartInteractionResponse> {
    let clean_query = query.trim();

    // Attempt direct lookup first using the first token
    let first_word = clean_query.split_whitespace().next().unwrap_or("");
    if let Ok(part) = fetch_part(http, first_word, limit).await {
        let (content, builder) = build_part_message(&part, first_word, locale, services);
        return Ok(PartInteractionResponse::DirectMatch(
            content,
            Box::new(builder),
        ));
    }

    // Fallback to search
    match crate::brick::search_parts(http, clean_query, limit).await {
        Ok(results) => {
            if results.is_empty() {
                Err(anyhow::anyhow!("No parts found matching query"))
            } else if results.len() == 1 {
                let p_num = &results[0].part_num;
                fetch_part(http, p_num, limit).await.map_or_else(
                    |_| Err(anyhow::anyhow!("Failed to fetch part details")),
                    |part| {
                        let (content, builder) = build_part_message(&part, p_num, locale, services);
                        Ok(PartInteractionResponse::DirectMatch(
                            content,
                            Box::new(builder),
                        ))
                    },
                )
            } else {
                let options: Vec<serenity::all::CreateSelectMenuOption> = results
                    .into_iter()
                    .take(25)
                    .map(|r| {
                        let label = format!("{} - {}", r.part_num, r.name);
                        let truncated_label = if label.len() > 100 {
                            format!("{}...", &label[..97])
                        } else {
                            label
                        };
                        serenity::all::CreateSelectMenuOption::new(truncated_label, r.part_num)
                    })
                    .collect();
                Ok(PartInteractionResponse::SearchResults(options))
            }
        }
        Err(e) => {
            tracing::error!("Failed to fetch/search part {}: {}", clean_query, e);
            Err(anyhow::anyhow!("Failed to fetch part"))
        }
    }
}

fn build_part_message(
    part: &crate::brick::RebrickablePart,
    _part_num: &str,
    locale: &str,
    services: &[String],
) -> (String, CreateEmbed) {
    let in_production = if part.year_to >= chrono::Utc::now().naive_utc().year() {
        t!("common.yes", locale = locale).to_string()
    } else {
        t!("common.no", locale = locale).to_string()
    };

    let molds = if part.molds.is_empty() {
        t!("common.none", locale = locale).to_string()
    } else {
        part.molds.join(", ")
    };
    let alternates = if part.alternates.is_empty() {
        t!("common.none", locale = locale).to_string()
    } else {
        part.alternates.join(", ")
    };
    let print_of = part
        .print_of
        .clone()
        .unwrap_or_else(|| t!("common.na", locale = locale).to_string());

    let mut builder = CreateEmbed::new()
        .title(t!(
            "command.part.title",
            locale = locale,
            name = &part.name,
            id = &part.part_num
        ))
        .url(&part.part_url)
        .field(
            t!("command.part.years", locale = locale),
            format!("{}-{}", part.year_from, part.year_to),
            true,
        )
        .field(
            t!("command.part.in_production", locale = locale),
            in_production,
            true,
        )
        .field(t!("command.part.print_of", locale = locale), print_of, true)
        .field(t!("command.part.molds", locale = locale), molds, true)
        .field(
            t!("command.part.alternates", locale = locale),
            alternates,
            true,
        );

    if let Some(img) = &part.part_img_url {
        builder = builder.thumbnail(img.clone());
    }

    let mut links_text = Vec::new();

    if services.contains(&"bricklink".to_string()) {
        let bl_id = part
            .external_ids
            .get("BrickLink")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_else(|| part.part_num.clone());
        links_text.push(format!(
            "**BrickLink**: {}",
            crate::links::bricklink::part_url(&bl_id)
        ));
    }

    let in_production = part.year_to >= chrono::Utc::now().naive_utc().year();
    if services.contains(&"lego".to_string()) && in_production {
        let lego_id = part
            .external_ids
            .get("Lego")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_else(|| part.part_num.clone());
        links_text.push(format!(
            "**LEGO Pick a Brick**: {}",
            crate::links::lego::pick_a_brick_url(&lego_id)
        ));
    }

    if services.contains(&"rebrickable".to_string()) {
        links_text.push(format!(
            "**Rebrickable**: {}",
            crate::links::rebrickable::part_url(&part.part_num)
        ));
    }

    let content = if links_text.is_empty() {
        String::new()
    } else {
        links_text.join(" \n")
    };

    (content, builder)
}

pub fn build_part_command(locale: &str) -> serenity::all::CreateCommand {
    use serenity::all::{CommandOptionType, CreateCommand, CreateCommandOption};

    let cmd_name = rust_i18n::t!("command.part.name", locale = locale).to_string();
    let cmd_desc = rust_i18n::t!("command.part.desc", locale = locale).to_string();
    let part_arg_name = rust_i18n::t!("command.part.part_arg_name", locale = locale).to_string();
    let part_desc = rust_i18n::t!("command.part.part_desc", locale = locale).to_string();

    let part_option =
        CreateCommandOption::new(CommandOptionType::String, &part_arg_name, &part_desc)
            .required(true);

    CreateCommand::new(&cmd_name)
        .description(&cmd_desc)
        .add_option(part_option)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_build_part_message_en() {
        let mut ext_ids = HashMap::new();
        ext_ids.insert("BrickLink".to_string(), vec!["3001".to_string()]);
        ext_ids.insert("Lego".to_string(), vec!["98765".to_string()]);

        let part = crate::brick::RebrickablePart {
            part_num: "3001".to_string(),
            name: "Brick 2x4".to_string(),
            part_url: "https://rebrickable.com/parts/3001".to_string(),
            part_img_url: Some("http://example.com/3001.png".to_string()),
            external_ids: ext_ids,
            print_of: None,
            year_from: 1958,
            year_to: 2026,
            molds: vec![],
            alternates: vec![],
        };

        let (content, embed) = build_part_message(
            &part,
            "3001",
            "en-US",
            &[
                "bricklink".to_string(),
                "rebrickable".to_string(),
                "lego".to_string(),
            ],
        );
        assert!(content.contains("https://www.bricklink.com/v2/catalog/catalogitem.page?P=3001"));
        assert!(content.contains("https://rebrickable.com/parts/3001"));
        assert!(
            content.contains("https://www.lego.com/fi-fi/pick-and-build/pick-a-brick?query=98765")
        );

        let embed_json = serde_json::to_value(embed).unwrap();
        assert_eq!(embed_json["title"], "Part: Brick 2x4 (3001)");
        assert_eq!(
            embed_json["thumbnail"]["url"],
            "http://example.com/3001.png"
        );
        assert_eq!(embed_json["url"], "https://rebrickable.com/parts/3001");
    }
}
