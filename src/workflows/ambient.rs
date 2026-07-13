use crate::ambient::Topic;
use crate::workflows::AppContext;
use rust_i18n::t;
use serenity::all::{
    ActionRowComponent, ButtonStyle, ComponentInteraction, Context, CreateActionRow, CreateButton,
    CreateInputText, CreateInteractionResponse, CreateInteractionResponseMessage, CreateModal,
    CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, InputTextStyle,
    ModalInteraction,
};

pub fn generate_suggestion_components(
    topic: Topic,
    locale: &str,
    extracted_id: Option<&str>,
    channel_id: u64,
    message_id: u64,
) -> Vec<CreateActionRow> {
    let topic_str = format!("{topic:?}");
    let suffix = format!(":{channel_id}:{message_id}");
    match topic {
        Topic::LegoSet => {
            let mut search_id = format!("workflow_set_search:none{suffix}");
            if let Some(id) = extracted_id {
                search_id = format!("workflow_set_search:{id}{suffix}");
            }
            vec![CreateActionRow::Buttons(vec![
                CreateButton::new(search_id)
                    .label(t!("workflow.set_search.label", locale = locale))
                    .style(ButtonStyle::Primary),
                CreateButton::new(format!("workflow_ignore:{topic_str}{suffix}"))
                    .label(t!("workflow.ignore.label", locale = locale))
                    .style(ButtonStyle::Secondary),
                CreateButton::new(format!("workflow_ignore_always{suffix}"))
                    .label(t!("workflow.ignore_always.label", locale = locale))
                    .style(ButtonStyle::Danger),
            ])]
        }
        Topic::LegoPart => {
            let mut search_id = format!("workflow_part_search:none{suffix}");
            if let Some(id) = extracted_id {
                search_id = format!("workflow_part_search:{id}{suffix}");
            }
            vec![CreateActionRow::Buttons(vec![
                CreateButton::new(search_id)
                    .label(t!("workflow.part_search.label", locale = locale))
                    .style(ButtonStyle::Primary),
                CreateButton::new(format!("workflow_ignore:{topic_str}{suffix}"))
                    .label(t!("workflow.ignore.label", locale = locale))
                    .style(ButtonStyle::Secondary),
                CreateButton::new(format!("workflow_ignore_always{suffix}"))
                    .label(t!("workflow.ignore_always.label", locale = locale))
                    .style(ButtonStyle::Danger),
            ])]
        }
    }
}

pub fn generate_suggestion_text(
    topic: Topic,
    locale: &str,
    extracted_id: Option<&str>,
    item_name: Option<&str>,
    article_count: usize,
) -> String {
    match topic {
        Topic::LegoSet => match (extracted_id, item_name) {
            (Some(id), Some(name)) => {
                if article_count > 0 {
                    t!(
                        "workflow.suggestion.set_with_id_name_articles",
                        locale = locale,
                        id = id,
                        name = name,
                        count = article_count
                    )
                    .to_string()
                } else {
                    t!(
                        "workflow.suggestion.set_with_id_name",
                        locale = locale,
                        id = id,
                        name = name
                    )
                    .to_string()
                }
            }
            (Some(id), None) => {
                t!("workflow.suggestion.set_with_id", locale = locale, id = id).to_string()
            }
            (None, _) => t!("workflow.suggestion.set", locale = locale).to_string(),
        },
        Topic::LegoPart => match (extracted_id, item_name) {
            (Some(id), Some(name)) => t!(
                "workflow.suggestion.part_with_id_name",
                locale = locale,
                id = id,
                name = name
            )
            .to_string(),
            (Some(id), None) => {
                t!("workflow.suggestion.part_with_id", locale = locale, id = id).to_string()
            }
            (None, _) => t!("workflow.suggestion.part", locale = locale).to_string(),
        },
    }
}

pub async fn handle_workflow_ignore(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
    locale: &str,
) -> anyhow::Result<()> {
    let _ = interaction.message.delete(&ctx.http).await;

    if let Some(topic_str) = arg {
        let channel_id = i64::try_from(interaction.channel_id.get()).unwrap_or_default();
        let _ = crate::db::defer_ambient_cooldown(&app_ctx.db, channel_id, topic_str).await;
    }

    let data = CreateInteractionResponseMessage::new()
        .content(t!("workflow.ignore.response", locale = locale))
        .ephemeral(true);
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(data))
        .await?;
    Ok(())
}

pub async fn handle_workflow_ignore_always(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    locale: &str,
) -> anyhow::Result<()> {
    let _ = interaction.message.delete(&ctx.http).await;
    let user_id = interaction.user.id.get().to_string();

    let _ = crate::db::set_user_ambient_preference(&app_ctx.db, &user_id, true).await;

    let data = CreateInteractionResponseMessage::new()
        .content(t!("workflow.ignore_always.response", locale = locale))
        .ephemeral(true);
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(data))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ambient::Topic;

    #[test]
    fn test_generate_suggestion_text() {
        let topic = Topic::LegoSet;
        let text_en = generate_suggestion_text(topic, "en-US", None, None, 0);
        // Because of rust_i18n macro we check for specific known strings or formatting
        // The mock or fallback behavior should at least produce a string.
        assert!(!text_en.is_empty());

        let text_fi = generate_suggestion_text(
            Topic::LegoPart,
            "fi-FI",
            Some("3001"),
            Some("Brick 2 x 4"),
            0,
        );
        assert!(!text_fi.is_empty());
    }

    #[test]
    fn test_generate_suggestion_components() {
        // Test LegoSet topic with an ID
        let components =
            generate_suggestion_components(Topic::LegoSet, "en-US", Some("42083"), 0, 0);
        // Should have 1 ActionRow
        assert_eq!(components.len(), 1);

        // Test LegoSet without an ID (should give "Search Sets")
        let components_no_id = generate_suggestion_components(Topic::LegoSet, "en-US", None, 0, 0);
        assert_eq!(components_no_id.len(), 1);
    }
}
