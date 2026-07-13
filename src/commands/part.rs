#![allow(unused_imports)]
use crate::brick::fetch_part;
use chrono::Datelike;
use rust_i18n::t;
use serenity::all::{Context, CreateEmbed, Framework};

pub async fn part_interaction(
    _ctx: &Context,
    http: &dyn crate::http::HttpProvider,
    part_num: &str,
    locale: &str,
    limit: u64,
) -> anyhow::Result<(String, Option<CreateEmbed>)> {
    let part_num = part_num.split_whitespace().next().unwrap_or("").to_string();

    match fetch_part(http, &part_num, limit).await {
        Ok(part) => {
            let (content, builder) = build_part_message(&part, &part_num, locale);
            Ok((content, Some(builder)))
        }
        Err(e) => {
            tracing::error!("Failed to fetch part {}: {}", part_num, e);
            Err(anyhow::anyhow!("Failed to fetch part"))
        }
    }
}

fn build_part_message(
    part: &crate::brick::RebrickablePart,
    part_num: &str,
    locale: &str,
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

    // Bricklink integration
    let bl_id = part
        .external_ids
        .get("BrickLink")
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or_else(|| part.part_num.clone());
    let bl_url = crate::links::bricklink::part_url(&bl_id);

    let content = t!(
        "command.part.content",
        locale = locale,
        num = part_num,
        url = &bl_url
    )
    .to_string();

    (content, builder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_build_part_message_en() {
        let mut ext_ids = HashMap::new();
        ext_ids.insert("BrickLink".to_string(), vec!["3001".to_string()]);

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

        let (content, embed) = build_part_message(&part, "3001", "en-US");
        assert!(content.contains("https://www.bricklink.com/v2/catalog/catalogitem.page?P=3001"));

        let embed_json = serde_json::to_value(&embed).unwrap();
        assert_eq!(embed_json["title"], "Part: Brick 2x4 (3001)");
        assert_eq!(
            embed_json["thumbnail"]["url"],
            "http://example.com/3001.png"
        );
        assert_eq!(embed_json["url"], "https://rebrickable.com/parts/3001");
    }
}
