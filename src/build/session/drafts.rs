//! Private drafts: the Draw/Insert flows, legend editing and previews.

use super::{
    ChannelId, CommandInteraction, ComponentInteraction, Context, CreateAttachment,
    CreateInteractionResponse, CreateInteractionResponseMessage, ProposalContext, build_cmd, db,
    dispatch, draft, library_for, max_size_for, offer_draft_positions, persist, place_draft,
    post_render, render_model, respond_ephemeral_command, respond_ephemeral_component,
    session_lock,
};

pub(super) const DRAFT_KIND: &str = "build_draft";

/// How long a draft survives without being touched.
pub(super) const DRAFT_TTL_MINS: u32 = 60;

/// A draft as it is stored between interactions.
///
/// Kept in `workflow_sessions`, which is **owner-scoped** — the draft belongs
/// to one person, where the model belongs to the thread. That is the whole
/// privacy property, and it comes from the existing table rather than any new
/// schema.
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub(super) struct StoredDraft {
    pub(super) session_id: String,
    pub(super) view: String,
    pub(super) legend: String,
    pub(super) grid: String,
    pub(super) location: String,
    /// The revision the last preview was taken against.
    pub(super) based_on: usize,
    /// Bumped on every preview. A render that finishes after the builder has
    /// moved on carries a stale sequence and drops its edit.
    pub(super) preview_seq: u64,
}

impl StoredDraft {
    pub(super) fn fields(&self) -> draft::DraftFields {
        draft::DraftFields {
            view: self.view.clone(),
            legend: self.legend.clone(),
            grid: self.grid.clone(),
            location: self.location.clone(),
        }
    }
}

pub(super) async fn load_draft(
    app_ctx: &crate::workflows::AppContext,
    draft_id: &str,
    user_id: &str,
) -> Option<StoredDraft> {
    let payload = db::get_workflow_session_payload(&app_ctx.db, draft_id, user_id)
        .await
        .ok()??;
    serde_json::from_value(payload).ok()
}

/// The draft this person already has open in this channel, if any.
///
/// A slash command knows only who ran it and where, never a draft id, so this
/// is how `/build-legend` and a repeated `[Draw…]` find the same drawing.
pub(super) async fn open_draft(
    app_ctx: &crate::workflows::AppContext,
    user_id: &str,
    channel_id: &str,
) -> Option<(String, StoredDraft)> {
    let (draft_id, payload) =
        db::find_latest_workflow_session(&app_ctx.db, DRAFT_KIND, user_id, channel_id)
            .await
            .ok()??;
    let stored: StoredDraft = serde_json::from_value(payload).ok()?;
    Some((draft_id, stored))
}

/// Opens a new draft, prefilled with the model as it stands.
///
/// Editing what is already built is the easiest way to name parts and colours
/// that certainly exist, so a draft starts from the current drawing rather
/// than a blank page.
pub(super) async fn create_draft(
    app_ctx: &crate::workflows::AppContext,
    session: &db::build::BuildSession,
    user_id: &str,
    guild_id: u64,
    channel_id: &str,
) -> anyhow::Result<(String, StoredDraft)> {
    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let model = history.materialize();
    let fields = crate::build::textview::render(&model, crate::build::units::GridView::Top, None)
        .map(|view| draft::split_fields(&view))
        .unwrap_or_default();

    let stored = StoredDraft {
        session_id: session.id.clone(),
        view: fields.view,
        legend: fields.legend,
        grid: fields.grid,
        location: fields.location,
        based_on: history.revision(),
        preview_seq: 0,
    };
    let draft_id = db::create_workflow_session(
        &app_ctx.db,
        DRAFT_KIND,
        user_id,
        &guild_id.to_string(),
        channel_id,
        serde_json::to_value(&stored).unwrap_or_default(),
        DRAFT_TTL_MINS,
    )
    .await?;
    Ok((draft_id, stored))
}

/// Closes a draft for good, so the next lookup does not find it again.
pub(super) async fn finish_draft(
    app_ctx: &crate::workflows::AppContext,
    draft_id: &str,
    user_id: &str,
) {
    let _ = db::consume_workflow_session(&app_ctx.db, draft_id, user_id).await;
}

pub(super) async fn save_draft(
    app_ctx: &crate::workflows::AppContext,
    draft_id: &str,
    user_id: &str,
    stored: &StoredDraft,
) {
    if let Ok(payload) = serde_json::to_value(stored) {
        let _ = db::update_workflow_session_payload(&app_ctx.db, draft_id, user_id, payload).await;
    }
}

/// Handles the `[Draw…]` button: opens the form, prefilled with the model.
///
/// Starting from what is already built is the easiest way to pick parts that
/// certainly exist, so an established session opens showing itself rather than
/// a blank page.
pub async fn handle_draw_button(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    session_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    let channel = interaction.channel_id.get().to_string();
    let Some(session) = db::build::get_active_session_by_channel(&app_ctx.db, &channel).await?
    else {
        return Ok(());
    };
    if session.id != session_id {
        return Ok(());
    }

    // Pressing the button again picks up where you left off rather than
    // starting a second draft: two live drafts in one channel would make
    // "your open drawing" ambiguous for `/build-legend`, and would quietly
    // strand whatever was in the first one.
    if let Some((draft_id, stored)) = open_draft(app_ctx, &user_id, &channel).await {
        return interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Modal(build_cmd::build_draw_modal(
                    &draft_id,
                    &stored.fields(),
                    &locale,
                )),
            )
            .await
            .map_err(Into::into);
    }

    let (draft_id, stored) = create_draft(app_ctx, &session, &user_id, guild_id, &channel).await?;

    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Modal(build_cmd::build_draw_modal(
                &draft_id,
                &stored.fields(),
                &locale,
            )),
        )
        .await?;
    Ok(())
}

/// Handles the two-step legend picker: `[Insert…]`, its pages, and its choices.
///
/// A colour on its own is not a legend line — `symbol = colour part` needs
/// both — so this is one flow in two steps rather than two buttons. The colour
/// already chosen rides in the custom id as its numeric LDraw code, which is
/// short, has no spaces, and cannot collide with the `:` separator.
pub async fn handle_insert(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    base_id: &str,
    arg: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    // `arg` carries everything after the first colon, which each step splits
    // into as many fields as it put there.
    let mut parts = arg.split(':');
    let draft_id = parts.next().unwrap_or_default().to_string();

    let Some(mut stored) = load_draft(app_ctx, &draft_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale.as_str()),
        )
        .await;
    };

    match base_id {
        // Step one: open the colour list.
        build_cmd::BUTTON_INSERT => {
            replace_with(
                ctx,
                interaction,
                build_cmd::build_colour_page(&draft_id, 0, &locale),
            )
            .await
        }
        build_cmd::PAGE_INSERT_COLOUR => {
            let page = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            replace_with(
                ctx,
                interaction,
                build_cmd::build_colour_page(&draft_id, page, &locale),
            )
            .await
        }
        // Step two: a colour is chosen, so show the parts.
        build_cmd::PICK_INSERT_COLOUR => {
            let Some(colour) = selected_value(interaction) else {
                return Ok(());
            };
            replace_with(
                ctx,
                interaction,
                build_cmd::build_part_page(&draft_id, &colour, 0, &locale),
            )
            .await
        }
        build_cmd::PAGE_INSERT_PART => {
            let colour = parts.next().unwrap_or(build_cmd::NO_COLOUR).to_string();
            let page = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
            replace_with(
                ctx,
                interaction,
                build_cmd::build_part_page(&draft_id, &colour, page, &locale),
            )
            .await
        }
        // Both chosen: write the line and show the drawing again.
        build_cmd::PICK_INSERT_PART => {
            let colour = parts.next().unwrap_or(build_cmd::NO_COLOUR).to_string();
            let Some(part) = selected_value(interaction) else {
                return Ok(());
            };
            let colour_name = colour_name_of(&colour);

            let mut fields = stored.fields();
            let symbol = draft::next_symbol(&fields.legend, colour_name);
            draft::append_legend_line(&mut fields, symbol, colour_name, &part);
            stored.legend = fields.legend;

            show_preview(
                ctx,
                app_ctx,
                interaction,
                &mut stored,
                &DraftContext {
                    draft_id: &draft_id,
                    user_id: &user_id,
                    locale: &locale,
                    guild_id,
                    channel_id: interaction.channel_id,
                },
            )
            .await
        }
        _ => Ok(()),
    }
}

/// The colour a picker step is carrying, or `None` for "no colour".
pub(super) fn colour_name_of(code: &str) -> Option<&'static str> {
    if code == build_cmd::NO_COLOUR {
        return None;
    }
    code.parse::<u16>()
        .ok()
        .and_then(crate::ldraw::colors::name_of)
}

/// The single value chosen from a select menu.
pub(super) fn selected_value(interaction: &ComponentInteraction) -> Option<String> {
    match &interaction.data.kind {
        serenity::all::ComponentInteractionDataKind::StringSelect { values } => {
            values.first().cloned()
        }
        _ => None,
    }
}

/// Swaps the picker's own message for its next step, rather than posting a new
/// one per page.
pub(super) async fn replace_with(
    ctx: &Context,
    interaction: &ComponentInteraction,
    components: Vec<serenity::all::CreateActionRow>,
) -> anyhow::Result<()> {
    interaction
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new().components(components),
            ),
        )
        .await?;
    Ok(())
}

/// Handles `/build-legend`: adds a line to the open drawing.
///
/// Opens a draft when there is none, prefilled from the current model — one
/// command should get you from nothing to a drawing with the part you picked
/// already in it. Only the absence of a `/build` session is a refusal, because
/// then there is nothing to draft against.
pub async fn handle_legend_command(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    command: &CommandInteraction,
) -> anyhow::Result<()> {
    let guild_id = command.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = command.user.id.get().to_string();
    let channel = command.channel_id.get().to_string();

    let (mut part, mut colour, mut symbol) = (None, None, None);
    for option in &command.data.options {
        let Some(which) = build_cmd::legend_option(&option.name) else {
            continue;
        };
        let Some(value) = option.value.as_str() else {
            continue;
        };
        match which {
            build_cmd::LegendOption::Part => part = Some(value.to_string()),
            build_cmd::LegendOption::Colour => colour = Some(value.to_string()),
            build_cmd::LegendOption::Symbol => symbol = value.chars().next(),
        }
    }

    let Some(part) = part else {
        return Ok(());
    };

    let Some(session) = db::build::get_active_session_by_channel(&app_ctx.db, &channel).await?
    else {
        return respond_ephemeral_command(
            ctx,
            command,
            &rust_i18n::t!("command.build.legend_no_session", locale = locale.as_str()),
        )
        .await;
    };

    let (draft_id, mut stored) = match open_draft(app_ctx, &user_id, &channel).await {
        Some(found) => found,
        None => create_draft(app_ctx, &session, &user_id, guild_id, &channel).await?,
    };

    let mut fields = stored.fields();
    let symbol = symbol.unwrap_or_else(|| draft::next_symbol(&fields.legend, colour.as_deref()));
    draft::append_legend_line(&mut fields, symbol, colour.as_deref(), &part);
    stored.legend = fields.legend;

    show_preview(
        ctx,
        app_ctx,
        command,
        &mut stored,
        &DraftContext {
            draft_id: &draft_id,
            user_id: &user_id,
            locale: &locale,
            guild_id,
            channel_id: command.channel_id,
        },
    )
    .await
}

/// Handles the drawing form's submission.
///
/// Interprets against a *clone* of the history and never persists, so the
/// thread sees nothing. Answers with the text preview straight away — well
/// inside Discord's three seconds — then edits the picture in when it is ready.
pub async fn handle_draw_modal(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &serenity::all::ModalInteraction,
    draft_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    let Some(mut stored) = load_draft(app_ctx, draft_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale.as_str()),
        )
        .await;
    };

    let fields = read_draw_modal(interaction);
    stored.view = fields.view.clone();
    stored.legend = fields.legend.clone();
    stored.grid = fields.grid.clone();
    stored.location = fields.location.clone();

    show_preview(
        ctx,
        app_ctx,
        interaction,
        &mut stored,
        &DraftContext {
            draft_id,
            user_id: &user_id,
            locale: &locale,
            guild_id,
            channel_id: interaction.channel_id,
        },
    )
    .await
}

/// Reads the four inputs back out of a submitted drawing form.
pub(super) fn read_draw_modal(interaction: &serenity::all::ModalInteraction) -> draft::DraftFields {
    let mut fields = draft::DraftFields::default();
    for row in &interaction.data.components {
        if let Some(serenity::all::ActionRowComponent::InputText(input)) = row.components.first() {
            let value = input.value.clone().unwrap_or_default();
            match input.custom_id.as_str() {
                "view" => fields.view = value,
                "legend" => fields.legend = value,
                "grid" => fields.grid = value,
                "location" => fields.location = value,
                _ => {}
            }
        }
    }
    fields
}

/// Who is drafting, and where — gathered so [`show_preview`] stays under
/// clippy's argument-count lint, the same way `MessageContext` does for the
/// debug log.
pub(super) struct DraftContext<'a> {
    pub(super) draft_id: &'a str,
    pub(super) user_id: &'a str,
    pub(super) locale: &'a str,
    pub(super) guild_id: u64,
    pub(super) channel_id: ChannelId,
}

/// Anything that can answer an interaction with an ephemeral preview.
///
/// The form submit and the `[Re-preview]` button do exactly the same thing
/// from here on, and they arrive as different interaction types.
pub(super) trait Respondable {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()>;
    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>);
}

impl Respondable for serenity::all::ModalInteraction {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()> {
        self.create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;
        Ok(())
    }

    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>) {
        let _ = self
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new()
                    .content(content)
                    .new_attachment(CreateAttachment::bytes(png, "preview.png")),
            )
            .await;
    }
}

impl Respondable for CommandInteraction {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()> {
        self.create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;
        Ok(())
    }

    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>) {
        let _ = self
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new()
                    .content(content)
                    .new_attachment(CreateAttachment::bytes(png, "preview.png")),
            )
            .await;
    }
}

impl Respondable for ComponentInteraction {
    async fn reply_ephemeral(
        &self,
        ctx: &Context,
        content: String,
        components: Vec<serenity::all::CreateActionRow>,
    ) -> anyhow::Result<()> {
        self.create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;
        Ok(())
    }

    async fn attach_render(&self, ctx: &Context, content: String, png: Vec<u8>) {
        let _ = self
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new()
                    .content(content)
                    .new_attachment(CreateAttachment::bytes(png, "preview.png")),
            )
            .await;
    }
}

/// Previews a draft and answers with it, then edits the picture in.
pub(super) async fn show_preview<R: Respondable>(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &R,
    stored: &mut StoredDraft,
    who: &DraftContext<'_>,
) -> anyhow::Result<()> {
    let (draft_id, locale, user_id) = (who.draft_id, who.locale, who.user_id);
    let cfg = app_ctx.config.commands_for(who.guild_id).build;

    let Some(session) =
        db::build::get_active_session_by_channel(&app_ctx.db, &who.channel_id.get().to_string())
            .await?
    else {
        return interaction
            .reply_ephemeral(
                ctx,
                rust_i18n::t!("command.build.draft_expired", locale = locale).to_string(),
                Vec::new(),
            )
            .await;
    };

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let builders = dispatch::Builders::new([(user_id.to_string(), user_id.to_string())]);
    let preview = draft::preview(
        &history,
        &stored.fields(),
        locale,
        &builders,
        user_id,
        max_size_for(&session, &cfg),
    );

    // A drawing with nowhere said to put it is the same question a bare part
    // name asks — answer it the same way, rather than showing a preview that
    // looks like nothing happened and leaving the `[Place]` button to be
    // discovered separately.
    if let Some(pending) = preview.outcome.pending_proposal.clone() {
        return offer_draft_positions(
            ctx,
            app_ctx,
            interaction,
            draft_id,
            &ProposalContext {
                pool: &app_ctx.db,
                session: &session,
                user_id,
                guild_id: who.guild_id,
                channel_id: who.channel_id,
                cfg: &cfg,
                locale,
            },
            pending,
        )
        .await;
    }

    stored.based_on = preview.based_on;
    stored.preview_seq = stored.preview_seq.saturating_add(1);
    save_draft(app_ctx, draft_id, user_id, stored).await;

    let text = build_cmd::draft_preview_message(&preview, locale);
    let buttons = build_cmd::build_draft_buttons(draft_id, locale, false);
    interaction
        .reply_ephemeral(ctx, text.clone(), buttons)
        .await?;

    // A failed drawing has no picture worth drawing.
    if preview.failed() {
        return Ok(());
    }

    // Then the render, which is slow enough that it cannot share the first
    // response. If the builder has moved on by the time it lands, the stored
    // sequence has advanced and the edit is dropped.
    let seq = stored.preview_seq;
    let library = library_for(ctx).await;
    let mut projected = history;
    for op in &preview.outcome.ops {
        projected.push(op.clone(), user_id);
    }
    if let Some(png) = render_model(&projected, &library, &cfg).await {
        if load_draft(app_ctx, draft_id, user_id)
            .await
            .is_some_and(|current| current.preview_seq == seq)
        {
            interaction.attach_render(ctx, text, png).await;
        }
    }
    Ok(())
}

/// Handles the `[Edit]`, `[Apply]`, `[Discard]` and `[Preview again]` buttons.
pub async fn handle_draft_button(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    base_id: &str,
    draft_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    let Some(mut stored) = load_draft(app_ctx, draft_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale.as_str()),
        )
        .await;
    };

    match base_id {
        build_cmd::BUTTON_DRAFT_EDIT => {
            interaction
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Modal(build_cmd::build_draw_modal(
                        draft_id,
                        &stored.fields(),
                        &locale,
                    )),
                )
                .await?;
            Ok(())
        }
        build_cmd::BUTTON_DRAFT_REPREVIEW => {
            show_preview(
                ctx,
                app_ctx,
                interaction,
                &mut stored,
                &DraftContext {
                    draft_id,
                    user_id: &user_id,
                    locale: &locale,
                    guild_id,
                    channel_id: interaction.channel_id,
                },
            )
            .await
        }
        build_cmd::BUTTON_DRAFT_PLACE => {
            // The drafted block, offered positions instead of taking the one
            // written in the form. Same engine, same picture, bigger batch.
            place_draft(
                ctx,
                app_ctx,
                interaction,
                draft_id,
                &stored,
                &locale,
                &user_id,
            )
            .await
        }
        build_cmd::BUTTON_DRAFT_DISCARD => {
            // Actually close it, or the next `[Draw…]` or `/build-legend`
            // would pick the discarded drawing straight back up.
            finish_draft(app_ctx, draft_id, &user_id).await;
            respond_ephemeral_component(
                ctx,
                interaction,
                &rust_i18n::t!("command.build.draft_discarded", locale = locale.as_str()),
            )
            .await
        }
        build_cmd::BUTTON_DRAFT_APPLY => {
            apply_draft(
                ctx,
                app_ctx,
                interaction,
                draft_id,
                &stored,
                &locale,
                &user_id,
            )
            .await
        }
        _ => Ok(()),
    }
}

/// Commits a draft to the shared model and posts it publicly.
///
/// Refuses when the session has moved since the preview: what the builder saw
/// must be exactly what lands, and a placement id from a stale preview may no
/// longer resolve. The draft is kept so `[Preview again]` can pick it up.
pub(super) async fn apply_draft(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    draft_id: &str,
    stored: &StoredDraft,
    locale: &str,
    user_id: &str,
) -> anyhow::Result<()> {
    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_expired", locale = locale),
        )
        .await;
    };

    let cfg = app_ctx
        .config
        .commands_for(interaction.guild_id.unwrap_or_default().get())
        .build;

    let lock = session_lock(&session.id).await;
    let guard = lock.lock().await;

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let builders = dispatch::Builders::new([(user_id.to_string(), user_id.to_string())]);
    let preview = draft::preview(
        &history,
        &stored.fields(),
        locale,
        &builders,
        user_id,
        max_size_for(&session, &cfg),
    );

    if draft::is_stale(&preview, &history) || stored.based_on != history.revision() {
        drop(guard);
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_stale", locale = locale),
        )
        .await;
    }
    if preview.failed() {
        drop(guard);
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_nothing_to_apply", locale = locale),
        )
        .await;
    }

    persist(
        &app_ctx.db,
        &session.id,
        &history,
        &preview.outcome,
        user_id,
    )
    .await?;
    let _ = db::build::touch_session(&app_ctx.db, &session.id, cfg.session_ttl_hours).await;
    drop(guard);

    // The drawing is in the model now, so the draft is over.
    finish_draft(app_ctx, draft_id, user_id).await;

    respond_ephemeral_component(
        ctx,
        interaction,
        &rust_i18n::t!("command.build.draft_applied", locale = locale),
    )
    .await?;

    // Now it is public, like anything else the thread builds.
    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let library = library_for(ctx).await;
    post_render(
        ctx,
        interaction.channel_id,
        &history,
        &library,
        &cfg,
        &session.id,
        locale,
    )
    .await;
    Ok(())
}
