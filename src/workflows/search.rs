use crate::workflows::AppContext;
use rust_i18n::t;
use serenity::all::{
    ActionRowComponent, ComponentInteraction, ComponentInteractionDataKind, Context,
    CreateActionRow, CreateInputText, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateModal, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, InputTextStyle,
    ModalInteraction,
};

pub async fn handle_update_services_set(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
    locale: &str,
) -> anyhow::Result<()> {
    let set_num = arg.unwrap_or_default();
    if let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind {
        let services = values.clone();

        let user_id = interaction.user.id.get().to_string();
        let services_str = services.join(",");
        let _ =
            crate::db::update_user_preferred_services(&app_ctx.db, &user_id, &services_str).await;

        if let Ok((content, embed)) = crate::commands::set::set_interaction(
            ctx,
            app_ctx.http.as_ref(),
            &app_ctx.db,
            set_num,
            locale,
            &services,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            let mut data = CreateInteractionResponseMessage::new().content(content);
            if let Some(embed) = embed {
                data = data.add_embed(embed);
            }

            let options = vec![
                CreateSelectMenuOption::new("BrickLink", "bricklink")
                    .default_selection(services.contains(&"bricklink".to_string())),
                CreateSelectMenuOption::new("Brickset", "brickset")
                    .default_selection(services.contains(&"brickset".to_string())),
                CreateSelectMenuOption::new("LEGO.com", "lego")
                    .default_selection(services.contains(&"lego".to_string())),
                CreateSelectMenuOption::new("Rebrickable", "rebrickable")
                    .default_selection(services.contains(&"rebrickable".to_string())),
                CreateSelectMenuOption::new(
                    t!("command.set.articles", locale = locale),
                    "articles",
                )
                .default_selection(services.contains(&"articles".to_string())),
            ];
            let select = CreateSelectMenu::new(
                format!("update_services_set:{set_num}"),
                CreateSelectMenuKind::String { options },
            )
            .min_values(0)
            .max_values(5)
            .placeholder(t!("modal.services.placeholder", locale = locale));

            data = data.components(vec![CreateActionRow::SelectMenu(select)]);

            interaction
                .create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(data))
                .await?;
        } else {
            tracing::error!("Failed to update set interaction");
        }
    }
    Ok(())
}

pub async fn handle_workflow_set_search(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
    locale: &str,
) -> anyhow::Result<()> {
    if let Some(val) = arg {
        let parts: Vec<&str> = val.split(':').collect();
        if !parts.is_empty() && parts[0] != "none" {
            let set_num = parts[0];
            let channel_id = i64::try_from(interaction.channel_id.get()).unwrap_or_default();
            let _ = crate::db::clear_ambient_cooldown(&app_ctx.db, channel_id, "LegoSet").await;

            let user_id = interaction.user.id.get().to_string();
            let default_services = if let Ok(Some(services_str)) =
                crate::db::get_user_preferred_services(&app_ctx.db, &user_id).await
            {
                if services_str.is_empty() {
                    vec![]
                } else {
                    services_str.split(',').map(ToString::to_string).collect()
                }
            } else {
                vec![
                    "bricklink".to_string(),
                    "lego".to_string(),
                    "articles".to_string(),
                ]
            };

            if let Ok((content, embed)) = crate::commands::set::set_interaction(
                ctx,
                app_ctx.http.as_ref(),
                &app_ctx.db,
                set_num,
                locale,
                &default_services,
                app_ctx.config.resource_limits.max_http_body_bytes,
            )
            .await
            {
                let mut data = CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(false);

                if let Some(e) = embed {
                    data = data.add_embed(e);
                }

                let options = vec![
                    CreateSelectMenuOption::new("BrickLink", "bricklink")
                        .default_selection(default_services.contains(&"bricklink".to_string())),
                    CreateSelectMenuOption::new("Brickset", "brickset")
                        .default_selection(default_services.contains(&"brickset".to_string())),
                    CreateSelectMenuOption::new("LEGO.com", "lego")
                        .default_selection(default_services.contains(&"lego".to_string())),
                    CreateSelectMenuOption::new("Rebrickable", "rebrickable")
                        .default_selection(default_services.contains(&"rebrickable".to_string())),
                    CreateSelectMenuOption::new(
                        t!("command.set.articles", locale = locale),
                        "articles",
                    )
                    .default_selection(default_services.contains(&"articles".to_string())),
                ];
                let select = CreateSelectMenu::new(
                    format!("update_services_set:{set_num}"),
                    CreateSelectMenuKind::String { options },
                )
                .min_values(0)
                .max_values(5)
                .placeholder(t!("modal.services.placeholder", locale = locale));

                data = data.components(vec![CreateActionRow::SelectMenu(select)]);

                interaction
                    .create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(data))
                    .await?;
                return Ok(());
            }
        }
    }

    let mut input = CreateInputText::new(
        InputTextStyle::Short,
        t!("modal.set_search.input_label", locale = locale),
        "set_number_input",
    )
    .placeholder(t!("modal.set_search.input_placeholder", locale = locale))
    .required(true);

    let mut modal_id = "modal_set_search".to_string();
    if let Some(val) = arg {
        let parts: Vec<&str> = val.split(':').collect();
        if !parts.is_empty() && parts[0] != "none" {
            input = input.value(parts[0]);
        }
        modal_id = format!("modal_set_search:{val}");
    }

    let modal = CreateModal::new(modal_id, t!("modal.set_search.title", locale = locale))
        .components(vec![CreateActionRow::InputText(input)]);
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;
    Ok(())
}

pub async fn handle_workflow_part_search(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
    locale: &str,
) -> anyhow::Result<()> {
    if let Some(val) = arg {
        let parts: Vec<&str> = val.split(':').collect();
        if !parts.is_empty() && parts[0] != "none" {
            let part_num = parts[0];
            let channel_id = i64::try_from(interaction.channel_id.get()).unwrap_or_default();
            let _ = crate::db::clear_ambient_cooldown(&app_ctx.db, channel_id, "LegoPart").await;

            if let Ok((content, embed)) = crate::commands::part::part_interaction(
                ctx,
                app_ctx.http.as_ref(),
                part_num,
                locale,
                app_ctx.config.resource_limits.max_http_body_bytes,
            )
            .await
            {
                let mut data = CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(vec![])
                    .ephemeral(false);

                if let Some(e) = embed {
                    data = data.add_embed(e);
                }

                interaction
                    .create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(data))
                    .await?;
                return Ok(());
            }
        }
    }

    let mut input = CreateInputText::new(
        InputTextStyle::Short,
        t!("modal.part_search.input_label", locale = locale),
        "part_number_input",
    )
    .placeholder(t!("modal.part_search.input_placeholder", locale = locale))
    .required(true);

    let mut modal_id = "modal_part_search".to_string();
    if let Some(val) = arg {
        let parts: Vec<&str> = val.split(':').collect();
        if !parts.is_empty() && parts[0] != "none" {
            input = input.value(parts[0]);
        }
        modal_id = format!("modal_part_search:{val}");
    }

    let modal = CreateModal::new(modal_id, t!("modal.part_search.title", locale = locale))
        .components(vec![CreateActionRow::InputText(input)]);
    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;
    Ok(())
}

pub async fn handle_modal_set_search(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ModalInteraction,
    locale: &str,
) -> anyhow::Result<()> {
    let channel_id = i64::try_from(interaction.channel_id.get()).unwrap_or_default();
    let _ = crate::db::clear_ambient_cooldown(&app_ctx.db, channel_id, "LegoSet").await;

    if let Some(ActionRowComponent::InputText(input)) = interaction
        .data
        .components
        .first()
        .and_then(|row| row.components.first())
    {
        let set_num = input.value.as_deref().unwrap_or_default();
        let user_id = interaction.user.id.get().to_string();
        let default_services = if let Ok(Some(services_str)) =
            crate::db::get_user_preferred_services(&app_ctx.db, &user_id).await
        {
            if services_str.is_empty() {
                vec![]
            } else {
                services_str.split(',').map(ToString::to_string).collect()
            }
        } else {
            vec![
                "bricklink".to_string(),
                "lego".to_string(),
                "articles".to_string(),
            ]
        };

        if let Ok((content, embed)) = crate::commands::set::set_interaction(
            ctx,
            app_ctx.http.as_ref(),
            &app_ctx.db,
            set_num,
            locale,
            &default_services,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            let mut data = CreateInteractionResponseMessage::new()
                .content(content.clone())
                .ephemeral(false);

            if let Some(ref e) = embed {
                data = data.add_embed(e.clone());
            }

            let options = vec![
                CreateSelectMenuOption::new("BrickLink", "bricklink")
                    .default_selection(default_services.contains(&"bricklink".to_string())),
                CreateSelectMenuOption::new("Brickset", "brickset")
                    .default_selection(default_services.contains(&"brickset".to_string())),
                CreateSelectMenuOption::new("LEGO.com", "lego")
                    .default_selection(default_services.contains(&"lego".to_string())),
                CreateSelectMenuOption::new("Rebrickable", "rebrickable")
                    .default_selection(default_services.contains(&"rebrickable".to_string())),
                CreateSelectMenuOption::new(
                    t!("command.set.articles", locale = locale),
                    "articles",
                )
                .default_selection(default_services.contains(&"articles".to_string())),
            ];
            let select = CreateSelectMenu::new(
                format!("update_services_set:{set_num}"),
                CreateSelectMenuKind::String { options },
            )
            .min_values(0)
            .max_values(5)
            .placeholder(t!("modal.services.placeholder", locale = locale));

            data = data.components(vec![CreateActionRow::SelectMenu(select)]);

            interaction
                .create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(data))
                .await?;
        } else {
            let data = CreateInteractionResponseMessage::new()
                .content(t!("workflow.set_search.error", locale = locale))
                .ephemeral(true);
            let _ = interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(data))
                .await;
        }
    }
    Ok(())
}

pub async fn handle_modal_part_search(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ModalInteraction,
    locale: &str,
) -> anyhow::Result<()> {
    let channel_id = i64::try_from(interaction.channel_id.get()).unwrap_or_default();
    let _ = crate::db::clear_ambient_cooldown(&app_ctx.db, channel_id, "LegoPart").await;

    if let Some(ActionRowComponent::InputText(input)) = interaction
        .data
        .components
        .first()
        .and_then(|row| row.components.first())
    {
        let part_num = input.value.as_deref().unwrap_or_default();
        if let Ok((content, embed)) = crate::commands::part::part_interaction(
            ctx,
            app_ctx.http.as_ref(),
            part_num,
            locale,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            let mut data = CreateInteractionResponseMessage::new()
                .content(content.clone())
                .ephemeral(false);

            if let Some(ref e) = embed {
                data = data.add_embed(e.clone());
            }

            interaction
                .create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(data))
                .await?;
        } else {
            let data = CreateInteractionResponseMessage::new()
                .content(t!("workflow.part_search.error", locale = locale))
                .ephemeral(true);
            let _ = interaction
                .create_response(&ctx.http, CreateInteractionResponse::Message(data))
                .await;
        }
    }
    Ok(())
}
