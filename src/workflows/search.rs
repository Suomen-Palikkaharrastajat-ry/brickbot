use crate::workflows::AppContext;
use rust_i18n::t;
use serenity::all::{
    ActionRowComponent, CommandInteraction, ComponentInteraction, ComponentInteractionDataKind,
    Context, CreateActionRow, CreateInputText, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateModal, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, InputTextStyle, ModalInteraction,
};

fn extract_set_search_select_row(
    interaction: &ComponentInteraction,
    new_selected_set_id: Option<&str>,
) -> (Option<String>, Option<CreateActionRow>) {
    let mut original_query = None;
    let mut search_select_row = None;

    for row in &interaction.message.components {
        if let Some(ActionRowComponent::SelectMenu(menu)) = row.components.first() {
            if let Some(custom_id) = &menu.custom_id {
                if custom_id.starts_with("set_search_select:") {
                    let query = custom_id.trim_start_matches("set_search_select:");
                    original_query = Some(query.to_string());

                    let mut new_options = Vec::new();
                    for opt in &menu.options {
                        let is_selected = new_selected_set_id
                            .map_or(opt.default, |selected| opt.value == selected);

                        let mut new_opt = CreateSelectMenuOption::new(&opt.label, &opt.value)
                            .default_selection(is_selected);
                        if let Some(desc) = &opt.description {
                            new_opt = new_opt.description(desc);
                        }
                        if let Some(emoji) = &opt.emoji {
                            new_opt = new_opt.emoji(emoji.clone());
                        }
                        new_options.push(new_opt);
                    }
                    let mut new_menu = CreateSelectMenu::new(
                        custom_id,
                        CreateSelectMenuKind::String {
                            options: new_options,
                        },
                    );
                    if let Some(placeholder) = &menu.placeholder {
                        new_menu = new_menu.placeholder(placeholder);
                    }
                    search_select_row = Some(CreateActionRow::SelectMenu(new_menu));
                }
            }
        }
    }

    (original_query, search_select_row)
}

#[allow(clippy::too_many_arguments)]
pub async fn build_set_response_data(
    _ctx: &Context,
    app_ctx: &AppContext,
    query: &str,
    original_query: Option<&str>,
    user_id: &str,
    locale: &str,
    preview_mode: bool,
    search_select_row: Option<CreateActionRow>,
) -> Option<CreateInteractionResponseMessage> {
    let default_services = if let Ok(Some(services_str)) =
        crate::db::get_user_preferred_services(&app_ctx.db, user_id).await
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

    crate::commands::set::set_interaction(
        app_ctx.http.as_ref(),
        &app_ctx.db,
        query,
        locale,
        &default_services,
        app_ctx.config.resource_limits.max_http_body_bytes,
    )
    .await
    .ok()
    .map(|response| match response {
        crate::commands::set::SetInteractionResponse::DirectMatch(content, embed, has_articles) => {
            let mut data = CreateInteractionResponseMessage::new()
                .content(content)
                .add_embed(*embed)
                .ephemeral(preview_mode);

            let mut options = vec![
                CreateSelectMenuOption::new("BrickLink", "bricklink")
                    .default_selection(default_services.contains(&"bricklink".to_string())),
                CreateSelectMenuOption::new("Brickset", "brickset")
                    .default_selection(default_services.contains(&"brickset".to_string())),
                CreateSelectMenuOption::new("LEGO.com", "lego")
                    .default_selection(default_services.contains(&"lego".to_string())),
                CreateSelectMenuOption::new("Rebrickable", "rebrickable")
                    .default_selection(default_services.contains(&"rebrickable".to_string())),
            ];

            if has_articles {
                options.push(
                    CreateSelectMenuOption::new(
                        t!("command.set.articles", locale = locale),
                        "articles",
                    )
                    .default_selection(default_services.contains(&"articles".to_string())),
                );
            }
            let select = CreateSelectMenu::new(
                format!("update_services_set:{query}"),
                CreateSelectMenuKind::String { options },
            )
            .min_values(0)
            .max_values(5)
            .placeholder(t!("modal.services.placeholder", locale = locale));

            let mut components = vec![];

            if preview_mode {
                if let Some(row) = search_select_row {
                    components.push(row);
                }
            }

            components.push(CreateActionRow::SelectMenu(select));

            if preview_mode {
                use serenity::all::{ButtonStyle, CreateButton};
                let show_btn = CreateButton::new(format!("set_search_show:{query}"))
                    .label(t!(
                        "command.set.show",
                        locale = locale,
                        default = "Post to Channel"
                    ))
                    .style(ButtonStyle::Primary);

                let mut btns = vec![show_btn];

                if let Some(oq) = original_query {
                    let search_btn = CreateButton::new(format!("set_search_again:{oq}"))
                        .label(t!(
                            "command.set.search_again",
                            locale = locale,
                            default = "Search Again"
                        ))
                        .style(ButtonStyle::Secondary);
                    btns.push(search_btn);
                }

                components.push(CreateActionRow::Buttons(btns));
            }

            data.components(components)
        }
        crate::commands::set::SetInteractionResponse::SearchResults(options) => {
            let actual_query = original_query.unwrap_or(query);
            let select_menu = serenity::all::CreateSelectMenu::new(
                format!("set_search_select:{actual_query}"),
                serenity::all::CreateSelectMenuKind::String { options },
            )
            .placeholder(t!(
                "command.set.search_placeholder",
                locale = locale,
                default = "Select a set..."
            ));

            let mut components = vec![CreateActionRow::SelectMenu(select_menu)];

            if preview_mode {
                use serenity::all::{ButtonStyle, CreateButton};
                let search_btn = CreateButton::new(format!("set_search_again:{actual_query}"))
                    .label(t!(
                        "command.set.search_again",
                        locale = locale,
                        default = "Search Again"
                    ))
                    .style(ButtonStyle::Secondary);
                components.push(CreateActionRow::Buttons(vec![search_btn]));
            }

            CreateInteractionResponseMessage::new()
                .content(t!(
                    "workflow.set_search.select",
                    locale = locale,
                    default = "Please select a set:"
                ))
                .components(components)
                .ephemeral(true)
        }
    })
}

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

        let preview_mode = interaction
            .message
            .flags
            .is_some_and(|f| f.contains(serenity::all::MessageFlags::EPHEMERAL));

        let (original_query, search_select_row) = extract_set_search_select_row(interaction, None);
        if let Some(mut data) = build_set_response_data(
            ctx,
            app_ctx,
            set_num,
            original_query.as_deref(),
            &user_id,
            locale,
            preview_mode,
            search_select_row,
        )
        .await
        {
            interaction
                .create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(data))
                .await?;
        } else {
            tracing::error!("Failed to update set interaction");
        }
    }
    Ok(())
}

fn extract_part_search_select_row(
    interaction: &ComponentInteraction,
    new_selected_part_num: Option<&str>,
) -> (Option<String>, Option<CreateActionRow>) {
    let mut original_query = None;
    let mut search_select_row = None;

    for row in &interaction.message.components {
        if let Some(ActionRowComponent::SelectMenu(menu)) = row.components.first() {
            if let Some(custom_id) = &menu.custom_id {
                if custom_id.starts_with("part_search_select:") {
                    let query = custom_id.trim_start_matches("part_search_select:");
                    original_query = Some(query.to_string());

                    let mut new_options = Vec::new();
                    for opt in &menu.options {
                        let is_selected = new_selected_part_num
                            .map_or(opt.default, |selected| opt.value == selected);

                        let mut new_opt = CreateSelectMenuOption::new(&opt.label, &opt.value)
                            .default_selection(is_selected);
                        if let Some(desc) = &opt.description {
                            new_opt = new_opt.description(desc);
                        }
                        if let Some(emoji) = &opt.emoji {
                            new_opt = new_opt.emoji(emoji.clone());
                        }
                        new_options.push(new_opt);
                    }
                    let mut new_menu = CreateSelectMenu::new(
                        custom_id,
                        CreateSelectMenuKind::String {
                            options: new_options,
                        },
                    );
                    if let Some(placeholder) = &menu.placeholder {
                        new_menu = new_menu.placeholder(placeholder);
                    }
                    search_select_row = Some(CreateActionRow::SelectMenu(new_menu));
                }
            }
        }
    }

    (original_query, search_select_row)
}

#[allow(clippy::too_many_arguments)]
pub async fn build_part_response_data(
    ctx: &Context,
    app_ctx: &AppContext,
    query: &str,
    original_query: Option<&str>,
    user_id: &str,
    locale: &str,
    preview_mode: bool,
    search_select_row: Option<CreateActionRow>,
) -> Option<CreateInteractionResponseMessage> {
    let default_services = if let Ok(Some(services_str)) =
        crate::db::get_user_preferred_services(&app_ctx.db, user_id).await
    {
        if services_str.is_empty() {
            vec![]
        } else {
            services_str.split(',').map(ToString::to_string).collect()
        }
    } else {
        vec!["bricklink".to_string(), "rebrickable".to_string()]
    };

    crate::commands::part::part_interaction(
        ctx,
        app_ctx.http.as_ref(),
        query,
        locale,
        &default_services,
        app_ctx.config.resource_limits.max_http_body_bytes,
    )
    .await
    .ok()
    .map(|response| match response {
        crate::commands::part::PartInteractionResponse::DirectMatch(
            content,
            embed,
            in_production,
        ) => {
            let mut data = CreateInteractionResponseMessage::new()
                .content(content)
                .add_embed(*embed)
                .ephemeral(preview_mode);

            let mut options = vec![
                CreateSelectMenuOption::new("BrickLink", "bricklink")
                    .default_selection(default_services.contains(&"bricklink".to_string())),
            ];
            if in_production {
                options.push(
                    CreateSelectMenuOption::new("LEGO Pick a Brick", "lego")
                        .default_selection(default_services.contains(&"lego".to_string())),
                );
            }
            options.push(
                CreateSelectMenuOption::new("Rebrickable", "rebrickable")
                    .default_selection(default_services.contains(&"rebrickable".to_string())),
            );

            let max_vals = if in_production { 3 } else { 2 };
            let select = CreateSelectMenu::new(
                format!("update_services_part:{query}"),
                CreateSelectMenuKind::String { options },
            )
            .min_values(0)
            .max_values(max_vals)
            .placeholder(t!("modal.services.placeholder", locale = locale));

            let mut components = vec![];

            if preview_mode {
                if let Some(row) = search_select_row {
                    components.push(row);
                }
            }

            components.push(CreateActionRow::SelectMenu(select));

            if preview_mode {
                use serenity::all::{ButtonStyle, CreateButton};
                let show_btn = CreateButton::new(format!("part_search_show:{query}"))
                    .label(t!(
                        "command.part.show",
                        locale = locale,
                        default = "Post to Channel"
                    ))
                    .style(ButtonStyle::Primary);

                let mut btns = vec![show_btn];

                if let Some(oq) = original_query {
                    let search_btn = CreateButton::new(format!("part_search_again:{oq}"))
                        .label(t!(
                            "command.part.search_again",
                            locale = locale,
                            default = "Search Again"
                        ))
                        .style(ButtonStyle::Secondary);
                    btns.push(search_btn);
                }

                components.push(CreateActionRow::Buttons(btns));
            }

            data = data.components(components);
            data
        }
        crate::commands::part::PartInteractionResponse::SearchResults(options) => {
            let actual_query = original_query.unwrap_or(query);
            let select_menu = serenity::all::CreateSelectMenu::new(
                format!("part_search_select:{actual_query}"),
                serenity::all::CreateSelectMenuKind::String { options },
            )
            .placeholder(t!("command.part.search_placeholder", locale = locale));

            let mut components = vec![CreateActionRow::SelectMenu(select_menu)];

            if preview_mode {
                use serenity::all::{ButtonStyle, CreateButton};
                let search_btn = CreateButton::new(format!("part_search_again:{actual_query}"))
                    .label(t!(
                        "command.part.search_again",
                        locale = locale,
                        default = "Search Again"
                    ))
                    .style(ButtonStyle::Secondary);
                components.push(CreateActionRow::Buttons(vec![search_btn]));
            }

            CreateInteractionResponseMessage::new()
                .content(t!(
                    "workflow.part_search.select",
                    locale = locale,
                    default = "Please select a part:"
                ))
                .components(components)
                .ephemeral(true)
        }
    })
}

pub async fn handle_update_services_part(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
    locale: &str,
) -> anyhow::Result<()> {
    let query = arg.unwrap_or_default();
    if let ComponentInteractionDataKind::StringSelect { values } = &interaction.data.kind {
        let services = values.clone();

        let user_id = interaction.user.id.get().to_string();
        let services_str = services.join(",");
        let _ =
            crate::db::update_user_preferred_services(&app_ctx.db, &user_id, &services_str).await;

        let preview_mode = interaction
            .message
            .flags
            .is_some_and(|f| f.contains(serenity::all::MessageFlags::EPHEMERAL));

        let (original_query, search_select_row) = extract_part_search_select_row(interaction, None);

        if let Some(data) = build_part_response_data(
            ctx,
            app_ctx,
            query,
            original_query.as_deref(),
            &user_id,
            locale,
            preview_mode,
            search_select_row,
        )
        .await
        {
            interaction
                .create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(data))
                .await?;
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
            if let Some(mut data) =
                build_set_response_data(ctx, app_ctx, set_num, None, &user_id, locale, false, None)
                    .await
            {
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

            let user_id = interaction.user.id.get().to_string();
            if let Some(data) =
                build_part_response_data(ctx, app_ctx, part_num, None, &user_id, locale, true, None)
                    .await
            {
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
        if let Some(mut data) =
            build_set_response_data(ctx, app_ctx, set_num, None, &user_id, locale, true, None).await
        {
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
        let query = input.value.as_deref().unwrap_or_default();
        let user_id = interaction.user.id.get().to_string();

        if let Some(data) =
            build_part_response_data(ctx, app_ctx, query, None, &user_id, locale, true, None).await
        {
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

pub async fn handle_set_command(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &CommandInteraction,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let set_num = interaction
        .data
        .options
        .iter()
        .find(|opt| {
            opt.name == "query"
                || opt.name == "set_number"
                || opt.name == "setin_numero"
                || opt.name == "haku"
        })
        .and_then(|opt| opt.value.as_str())
        .unwrap_or_default();

    let user_id = interaction.user.id.get().to_string();

    if let Some(data) =
        build_set_response_data(ctx, app_ctx, set_num, None, &user_id, &locale, true, None).await
    {
        interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(data))
            .await?;
    } else {
        let data = CreateInteractionResponseMessage::new()
            .content(t!("workflow.set_search.error", locale = locale))
            .ephemeral(true);
        let _ = interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(data))
            .await;
    }

    Ok(())
}

pub async fn handle_part_command(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &CommandInteraction,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let query = interaction
        .data
        .options
        .iter()
        .find(|opt| {
            opt.name == "query"
                || opt.name == "haku"
                || opt.name == "osan_numero"
                || opt.name == "part_number"
        })
        .and_then(|opt| opt.value.as_str())
        .unwrap_or_default();

    let user_id = interaction.user.id.get().to_string();

    if let Some(data) =
        build_part_response_data(ctx, app_ctx, query, None, &user_id, &locale, true, None).await
    {
        interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(data))
            .await?;
    } else {
        let data = CreateInteractionResponseMessage::new()
            .content(t!("command.part.search_no_results", locale = locale))
            .ephemeral(true);
        let _ = interaction
            .create_response(&ctx.http, CreateInteractionResponse::Message(data))
            .await;
    }

    Ok(())
}

pub async fn handle_part_search_select(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let part_num = if let serenity::all::ComponentInteractionDataKind::StringSelect { values } =
        &interaction.data.kind
    {
        values.first().map(String::as_str).unwrap_or_default()
    } else {
        return Err(anyhow::anyhow!("Invalid interaction data"));
    };

    let user_id = interaction.user.id.get().to_string();
    let preview_mode = true;
    let original_query = arg;

    let (_, search_select_row) = extract_part_search_select_row(interaction, Some(part_num));

    if let Some(data) = build_part_response_data(
        ctx,
        app_ctx,
        part_num,
        original_query,
        &user_id,
        &locale,
        preview_mode,
        search_select_row,
    )
    .await
    {
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

    Ok(())
}

pub async fn handle_part_search_show(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;
    let query = arg.unwrap_or_default();
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

    let mut data = None;
    if let Ok(crate::commands::part::PartInteractionResponse::DirectMatch(content_str, embed, _)) =
        crate::commands::part::part_interaction(
            ctx,
            app_ctx.http.as_ref(),
            query,
            &locale,
            &default_services,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
    {
        data = Some((content_str, embed));
    }

    if let Some((content_str, embed)) = data {
        // Add action row to indicate it was posted
        let update_content = t!(
            "command.part.posted",
            locale = locale,
            default = "Part info posted!"
        );
        let update_data = CreateInteractionResponseMessage::new()
            .content(update_content)
            .components(vec![])
            .embeds(vec![])
            .ephemeral(true);

        interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(update_data),
            )
            .await?;

        let msg = serenity::all::CreateMessage::new()
            .content(content_str)
            .embed(*embed);

        interaction.channel_id.send_message(&ctx.http, msg).await?;
    }

    Ok(())
}

pub async fn handle_part_search_again(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let mut input = CreateInputText::new(
        InputTextStyle::Short,
        t!("modal.part_search.input_label", locale = locale),
        "part_query",
    )
    .placeholder(t!("modal.part_search.input_placeholder", locale = locale))
    .required(true);

    if let Some(val) = arg {
        let parts: Vec<&str> = val.split(':').collect();
        if !parts.is_empty() && parts[0] != "none" {
            input = input.value(parts[0]);
        }
    }

    let modal = CreateModal::new(
        "modal_part_search",
        t!("modal.part_search.title", locale = locale),
    )
    .components(vec![CreateActionRow::InputText(input)]);

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;

    Ok(())
}

pub async fn handle_set_search_select(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let set_num = if let serenity::all::ComponentInteractionDataKind::StringSelect { values } =
        &interaction.data.kind
    {
        values.first().map(String::as_str).unwrap_or_default()
    } else {
        return Err(anyhow::anyhow!("Invalid interaction data"));
    };

    let user_id = interaction.user.id.get().to_string();
    let preview_mode = true;

    let original_query = arg;
    let (_, search_select_row) = extract_set_search_select_row(interaction, Some(set_num));
    if let Some(mut data) = build_set_response_data(
        ctx,
        app_ctx,
        set_num,
        original_query,
        &user_id,
        &locale,
        preview_mode,
        search_select_row,
    )
    .await
    {
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

    Ok(())
}

pub async fn handle_set_search_show(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;
    let query = arg.unwrap_or_default();
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

    if let Ok(crate::commands::set::SetInteractionResponse::DirectMatch(content_str, embed, _)) =
        crate::commands::set::set_interaction(
            app_ctx.http.as_ref(),
            &app_ctx.db,
            query,
            &locale,
            &default_services,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
    {
        let update_content = t!(
            "command.set.posted",
            locale = locale,
            default = "Set info posted!"
        );
        let update_data = CreateInteractionResponseMessage::new()
            .content(update_content)
            .components(vec![])
            .embeds(vec![])
            .ephemeral(true);

        interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(update_data),
            )
            .await?;

        let msg = serenity::all::CreateMessage::new()
            .content(content_str)
            .embed(*embed);

        interaction.channel_id.send_message(&ctx.http, msg).await?;
    }

    Ok(())
}

pub async fn handle_set_search_again(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &ComponentInteraction,
    arg: Option<&str>,
) -> anyhow::Result<()> {
    let (locale, _) = super::extract_locale_and_bot_name(app_ctx, interaction.guild_id).await;

    let mut input = CreateInputText::new(
        InputTextStyle::Short,
        t!("modal.set_search.input_label", locale = locale),
        "set_number_input",
    )
    .placeholder(t!("modal.set_search.input_placeholder", locale = locale))
    .required(true);

    if let Some(val) = arg {
        let parts: Vec<&str> = val.split(':').collect();
        if !parts.is_empty() && parts[0] != "none" {
            input = input.value(parts[0]);
        }
    }

    let modal = CreateModal::new(
        "modal_set_search",
        t!("modal.set_search.title", locale = locale),
    )
    .components(vec![CreateActionRow::InputText(input)]);

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;

    Ok(())
}
