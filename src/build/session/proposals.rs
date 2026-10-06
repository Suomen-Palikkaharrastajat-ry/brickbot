//! Placement proposals and the draft location flow.

use super::{
    BuildCommandConfig, ChannelId, ComponentInteraction, Context, CreateInteractionResponse,
    CreateMessage, DraftContext, LdrawLibrary, Outcome, RenderOptions, Respondable, SceneMode,
    SceneTriangle, StoredDraft, build_cmd, create_draft, db, dispatch, draft, finish_draft,
    library_for, max_size_for, open_draft, persist, post_render, render_slots,
    respond_ephemeral_component, save_draft, scene, session_lock, show_preview,
};

/// `workflow_sessions` kind for a set of offered positions.
pub(super) const PROPOSAL_KIND: &str = "build_proposal";

/// How long an offer stands. Short, because it is answered in a moment and a
/// stale one is refused anyway.
pub(super) const PROPOSAL_TTL_MINS: u32 = 15;

/// The colour a proposed placement is nudged toward, so it stands out from
/// the model it would join without hiding what colour it actually is.
pub(super) const PROPOSAL_HIGHLIGHT: crate::ldraw::colors::Rgb =
    crate::ldraw::colors::Rgb::new(255, 0, 220);

/// How far toward [`PROPOSAL_HIGHLIGHT`] a candidate's real colour is
/// nudged. High enough to read as "this one is new" at a glance; low enough
/// that a white part still looks white and a black one still looks black.
pub(super) const PROPOSAL_TINT: f32 = 0.0;

/// One tile of the proposal picture.
pub(super) const TILE_WIDTH: u32 = 320;
pub(super) const TILE_HEIGHT: u32 = 240;

/// A set of offered positions, waiting on a choice.
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub(super) struct StoredProposal {
    session_id: String,
    /// The positions on offer, in the order they are numbered.
    options: Vec<Vec<crate::build::ops::Placed>>,
    /// The revision they were worked out against. Anything built since and
    /// they no longer mean what the builder was shown.
    based_on: usize,
}

pub(super) async fn load_proposal(
    app_ctx: &crate::workflows::AppContext,
    proposal_id: &str,
    user_id: &str,
) -> Option<StoredProposal> {
    let payload = db::get_workflow_session_payload(&app_ctx.db, proposal_id, user_id)
        .await
        .ok()??;
    serde_json::from_value(payload).ok()
}

/// Works out where the named parts could go and asks.
///
/// Posted publicly rather than ephemerally, and not by choice: this is
/// triggered by a chat message, which carries no interaction token, and
/// Discord will not send an ephemeral message without one. The part picker
/// (`offer_part_choices`) posts publicly for exactly the same reason. In a
/// thread where everyone is building together it is no great loss.
pub(super) struct ProposalContext<'a> {
    pub(super) pool: &'a sqlx::SqlitePool,
    pub(super) session: &'a db::build::BuildSession,
    pub(super) user_id: &'a str,
    pub(super) guild_id: u64,
    pub(super) channel_id: ChannelId,
    pub(super) cfg: &'a BuildCommandConfig,
    pub(super) locale: &'a str,
}

pub(super) async fn offer_proposals(
    ctx: &Context,
    who: &ProposalContext<'_>,
    pending: &dispatch::PendingProposal,
) -> anyhow::Result<()> {
    let ProposalContext {
        pool,
        session,
        user_id,
        guild_id,
        channel_id,
        cfg,
        locale,
    } = *who;
    let history = db::build::load_history(pool, &session.id, &session.model_name).await?;
    let model = history.materialize();

    let candidates =
        crate::build::propose::propose_any(&model, &pending.batches, build_cmd::MAX_PROPOSALS);
    if candidates.is_empty() {
        channel_id
            .say(
                &ctx.http,
                rust_i18n::t!("command.build.propose_none", locale = locale).to_string(),
            )
            .await?;
        return Ok(());
    }

    let stored = StoredProposal {
        session_id: session.id.clone(),
        options: candidates.iter().map(|c| c.placements.clone()).collect(),
        based_on: history.revision(),
    };
    let proposal_id = db::create_workflow_session(
        pool,
        PROPOSAL_KIND,
        user_id,
        &guild_id.to_string(),
        &channel_id.to_string(),
        serde_json::to_value(&stored)?,
        PROPOSAL_TTL_MINS,
    )
    .await?;

    let library = library_for(ctx).await;
    let picture = render_proposals(&model, &candidates, &library, cfg).await;

    let mut message = CreateMessage::new()
        .content(build_cmd::proposal_message(
            &pending.label,
            &candidates,
            locale,
        ))
        .components(build_cmd::build_proposal_buttons(
            &proposal_id,
            candidates.len(),
            locale,
        ));
    if let Some(png) = picture {
        message = message.add_file(serenity::all::CreateAttachment::bytes(png, "proposals.png"));
    }
    channel_id.send_message(&ctx.http, message).await?;
    Ok(())
}

/// One proposal tile's triangles: the model as `base` left it, plus this
/// candidate's own parts.
///
/// The candidate's parts are drawn with exactly the geometry `build_scene`
/// would give them once they actually join the model — a real mesh when the
/// library has one, never a stand-in box — so a tile can never show a
/// placement that turns out to sit differently once it is committed. Tinting
/// their colour toward [`PROPOSAL_HIGHLIGHT`] rather than repainting it flat
/// keeps their real colour recognisable while still marking them as the part
/// on offer.
pub(super) async fn candidate_soup(
    base: &[SceneTriangle],
    library: &LdrawLibrary,
    use_meshes: bool,
    candidate: &crate::build::propose::Candidate,
) -> Vec<SceneTriangle> {
    let mut soup = base.to_vec();
    for placed in &candidate.placements {
        let placement = crate::build::model::Placement {
            id: crate::build::model::PlacementId::new(0, 0),
            part: placed.part.clone(),
            color: placed.color,
            pos: placed.pos,
            orientation: placed.orientation,
            group: None,
        };
        let (mut tris, _boxed) = scene::placement_triangles(&placement, library, use_meshes).await;
        for triangle in &mut tris {
            triangle.color = scene::blend(triangle.color, PROPOSAL_HIGHLIGHT, PROPOSAL_TINT);
        }
        soup.extend(tris);
    }
    soup
}

/// Draws every offer as one picture, tiled.
///
/// All the tiles share **one** camera, fitted to everything any of them
/// contains. Framing each to its own bounds would rescale the model between
/// tiles, and the eye would read that reframing as the difference — which is
/// exactly wrong when the pictures are alternatives to compare.
pub(super) async fn render_proposals(
    model: &crate::build::Model,
    candidates: &[crate::build::propose::Candidate],
    library: &LdrawLibrary,
    cfg: &BuildCommandConfig,
) -> Option<Vec<u8>> {
    use crate::math::Aabb;
    use crate::render::{bounds_of, framebuffer_size, render_to_framebuffer_with};

    let mode = if library.is_available() {
        SceneMode::Meshes
    } else {
        SceneMode::Boxes
    };
    let use_meshes = mode == SceneMode::Meshes;
    let (base, _) = scene::build_scene(model, library, mode).await;

    let mut soups: Vec<Vec<SceneTriangle>> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        soups.push(candidate_soup(&base, library, use_meshes, candidate).await);
    }
    if soups.is_empty() {
        return None;
    }

    let options = RenderOptions {
        width: TILE_WIDTH,
        height: TILE_HEIGHT,
        // The tiles are small already; supersampling them costs four times as
        // much for a picture nobody studies closely.
        supersample: 1,
        ..RenderOptions::default()
    };
    let union = soups
        .iter()
        .map(|soup| bounds_of(soup))
        .reduce(|a: Aabb, b: Aabb| Aabb {
            min: a.min.min(b.min),
            max: a.max.max(b.max),
        })?;
    let (frame_w, frame_h) = framebuffer_size(&options);
    let camera = crate::render::Camera::fit_default(union, frame_w, frame_h);
    let background = options.background_top;
    let _ = cfg;

    // One permit for the whole sheet, not one per tile: the semaphore is
    // process-wide with two slots, and four separate acquisitions would starve
    // the live renders every other thread is waiting on.
    let _permit = render_slots().acquire().await.ok()?;
    tokio::task::spawn_blocking(move || {
        let columns = 2u32;
        let rows = u32::try_from(soups.len().div_ceil(2)).unwrap_or(1);
        let mut sheet =
            crate::render::Framebuffer::new(TILE_WIDTH * columns, TILE_HEIGHT * rows, background);
        for (index, soup) in soups.iter().enumerate() {
            let tile = render_to_framebuffer_with(soup, &options, camera);
            let index = u32::try_from(index).unwrap_or(0);
            sheet.blit(
                &tile,
                (index % columns) * TILE_WIDTH,
                (index / columns) * TILE_HEIGHT,
            );
        }
        crate::render::encode_png(&sheet)
    })
    .await
    .ok()?
    .ok()
}

/// Handles the numbered buttons, the cancel, and the "draw this instead" menu
/// under a set of placement proposals.
pub async fn handle_proposal_button(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &ComponentInteraction,
    base_id: &str,
    arg: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    // `build_place` carries the chosen index after the proposal id.
    let (proposal_id, index_arg) = arg
        .split_once(':')
        .map_or((arg, None), |(id, rest)| (id, Some(rest.to_string())));

    let Some(stored) = load_proposal(app_ctx, proposal_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };

    if base_id == build_cmd::BUTTON_PROPOSE_CANCEL {
        let _ = db::consume_workflow_session(&app_ctx.db, proposal_id, &user_id).await;
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_cancelled", locale = locale.as_str()),
        )
        .await;
    }

    // Opening a modal has to be the *first* response to the interaction, so
    // this has to happen before anything below acknowledges it.
    if base_id == build_cmd::BUTTON_PROPOSE_OTHER {
        return interaction
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Modal(build_cmd::build_propose_location_modal(
                    proposal_id,
                    &locale,
                )),
            )
            .await
            .map_err(Into::into);
    }

    let chosen: usize = index_arg.and_then(|v| v.parse().ok()).unwrap_or(0);
    let Some(placements) = stored.options.get(chosen).cloned() else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };

    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };
    let cfg = app_ctx.config.commands_for(guild_id).build;

    let lock = session_lock(&session.id).await;
    let guard = lock.lock().await;
    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;

    // Someone else building in between moves everything the offer was measured
    // against, so what was shown is no longer what would land.
    if stored.based_on != history.revision() {
        drop(guard);
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    }

    // The placements are already legal — `propose` settled them through
    // `legal` — so they go in as one ordinary operation.
    let outcome = Outcome {
        ops: vec![crate::build::ops::Op::AddMany {
            placements,
            group: None,
        }],
        ops_start: Some(history.revision()),
        cursor: Some(history.revision() + 1),
        ..Outcome::default()
    };
    persist(&app_ctx.db, &session.id, &history, &outcome, &user_id).await?;
    let _ = db::build::touch_session(&app_ctx.db, &session.id, cfg.session_ttl_hours).await;
    drop(guard);

    let _ = db::consume_workflow_session(&app_ctx.db, proposal_id, &user_id).await;
    respond_ephemeral_component(
        ctx,
        interaction,
        &rust_i18n::t!("command.build.propose_placed", locale = locale.as_str()),
    )
    .await?;

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let library = library_for(ctx).await;
    post_render(
        ctx,
        interaction.channel_id,
        &history,
        &library,
        &cfg,
        &session.id,
        &locale,
    )
    .await;
    Ok(())
}

/// Offers positions for the block currently being drafted.
///
/// The drawing is previewed with its own location stripped out, so what comes
/// back is the block in local coordinates — a batch like any other.
pub(super) async fn place_draft(
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
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let cfg = app_ctx.config.commands_for(guild_id).build;

    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    let builders = dispatch::Builders::new([(user_id.to_string(), user_id.to_string())]);
    let mut fields = stored.fields();
    fields.location = String::new();
    let preview = draft::preview(
        &history,
        &fields,
        locale,
        &builders,
        user_id,
        max_size_for(&session, &cfg),
    );

    let Some(pending) = preview.outcome.pending_proposal.clone() else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.draft_nothing_to_apply", locale = locale),
        )
        .await;
    };

    offer_draft_positions(
        ctx,
        app_ctx,
        interaction,
        draft_id,
        &ProposalContext {
            pool: &app_ctx.db,
            session: &session,
            user_id,
            guild_id,
            channel_id: interaction.channel_id,
            cfg: &cfg,
            locale,
        },
        pending,
    )
    .await
}

/// Turns a location-less drawing into an ordinary position proposal: ends the
/// draft, then hands the batch to the same ranked-position picker a bare part
/// name gets.
///
/// Shared by the explicit `[Place]` button and by [`show_preview`] itself —
/// a drawing left without a location is exactly the question a bare part name
/// asks, so leaving the form's `location` field blank should reach the same
/// answer without the builder needing to know a second button exists.
pub(super) async fn offer_draft_positions<R: Respondable>(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &R,
    draft_id: &str,
    who: &ProposalContext<'_>,
    pending: dispatch::PendingProposal,
) -> anyhow::Result<()> {
    // The draft is done with; what happens next is an ordinary proposal.
    finish_draft(app_ctx, draft_id, who.user_id).await;
    respond_ephemeral_component(
        ctx,
        interaction,
        &rust_i18n::t!("command.build.propose_pick", locale = who.locale),
    )
    .await?;
    offer_proposals(ctx, who, &pending).await
}

/// Handles the `[Type a position…]` modal: previews the proposal's own
/// shape at whatever was typed, exactly the way any other drawing is.
///
/// Reuses the drawing form's machinery rather than parsing the typed phrase
/// itself — the location grammar, its legalisation and its preview are
/// already `show_preview`'s job, and a hand-rolled second parser would only
/// be another place for the two to drift apart.
pub async fn handle_propose_location_modal(
    ctx: &Context,
    app_ctx: &crate::workflows::AppContext,
    interaction: &serenity::all::ModalInteraction,
    proposal_id: &str,
) -> anyhow::Result<()> {
    let guild_id = interaction.guild_id.unwrap_or_default().get();
    let locale = app_ctx.config.locale_or_default(guild_id).to_string();
    let user_id = interaction.user.id.get().to_string();

    let Some(stored) = load_proposal(app_ctx, proposal_id, &user_id).await else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };
    let Some(placements) = stored.options.first().cloned() else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };
    let Some(session) = db::build::get_active_session_by_channel(
        &app_ctx.db,
        &interaction.channel_id.get().to_string(),
    )
    .await?
    else {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    };
    let history = db::build::load_history(&app_ctx.db, &session.id, &session.model_name).await?;
    if stored.based_on != history.revision() {
        return respond_ephemeral_component(
            ctx,
            interaction,
            &rust_i18n::t!("command.build.propose_expired", locale = locale.as_str()),
        )
        .await;
    }

    let channel = interaction.channel_id.get().to_string();
    let (draft_id, mut stored_draft) = match open_draft(app_ctx, &user_id, &channel).await {
        Some(found) => found,
        None => create_draft(app_ctx, &session, &user_id, guild_id, &channel).await?,
    };

    // The proposal's own parts, in their own relative shape — `textview`
    // always anchors a rendered drawing at the origin, so what the builder
    // typed is the only thing deciding where this actually lands.
    let mut sketch = crate::build::Model::new(&session.model_name);
    for (seq, placed) in placements.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        sketch.placements.push(crate::build::model::Placement {
            id: crate::build::model::PlacementId::new(0, seq as u32),
            part: placed.part.clone(),
            color: placed.color,
            pos: placed.pos,
            orientation: placed.orientation,
            group: None,
        });
    }
    if let Some(view) =
        crate::build::textview::render(&sketch, crate::build::units::GridView::Top, None)
    {
        let sketch_fields = draft::split_fields(&view);
        stored_draft.view = sketch_fields.view;
        stored_draft.legend = sketch_fields.legend;
        stored_draft.grid = sketch_fields.grid;
    }
    stored_draft.location = read_location_modal(interaction);
    save_draft(app_ctx, &draft_id, &user_id, &stored_draft).await;

    let _ = db::consume_workflow_session(&app_ctx.db, proposal_id, &user_id).await;

    show_preview(
        ctx,
        app_ctx,
        interaction,
        &mut stored_draft,
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

/// Reads the typed position back out of the `[Type a position…]` modal.
pub(super) fn read_location_modal(interaction: &serenity::all::ModalInteraction) -> String {
    interaction
        .data
        .components
        .first()
        .and_then(|row| row.components.first())
        .and_then(|component| match component {
            serenity::all::ActionRowComponent::InputText(input) => input.value.clone(),
            _ => None,
        })
        .unwrap_or_default()
}
