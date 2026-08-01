//! The `/build` slash command: its declaration and its message builders.
//!
//! Everything here is pure — it builds serenity structures and returns them,
//! with no IO — so it can be tested by serialising the builders to JSON, the
//! same way `src/commands/part.rs` is.
//!
//! The command itself is only the door. Once it has opened a thread, the
//! session in [`crate::build::session`] takes over and listens to chat.

#![allow(clippy::doc_markdown)]

use crate::build::Model;
use crate::discord_limits::{MAX_BUTTON_LABEL, MAX_SELECT_OPTION_LABEL, truncate};
use rust_i18n::t;
use serenity::all::{
    ButtonStyle, CommandOptionType, CreateActionRow, CreateButton, CreateCommand,
    CreateCommandOption, CreateEmbed,
};

/// Prefix for every `/build` component id. The router in
/// `crate::workflows` splits a custom id on its first colon, so the session id
/// follows as the argument.
pub const BUTTON_EXPORT: &str = "build_export";
pub const BUTTON_UNDO: &str = "build_undo";
pub const BUTTON_FINISH: &str = "build_finish";
/// The select menu shown when a part needs disambiguating.
pub const SELECT_PART: &str = "build_part_pick";

/// Opens the private drawing form.
pub const BUTTON_DRAW: &str = "build_draw";
/// The drawing form's own id, submitted back as a modal.
pub const MODAL_DRAW: &str = "build_draw_modal";
/// Buttons on the private preview.
pub const BUTTON_DRAFT_EDIT: &str = "build_draft_edit";
pub const BUTTON_DRAFT_APPLY: &str = "build_draft_apply";
pub const BUTTON_DRAFT_DISCARD: &str = "build_draft_discard";
/// Shown instead of `Apply` once the model has moved underneath a draft.
pub const BUTTON_DRAFT_REPREVIEW: &str = "build_draft_repreview";

/// Opens the two-step legend picker.
pub const BUTTON_INSERT: &str = "build_insert";
/// A page of the colour step, then the part step.
pub const PAGE_INSERT_COLOUR: &str = "build_insert_colour";
pub const PAGE_INSERT_PART: &str = "build_insert_part";
/// A choice made in each step.
pub const PICK_INSERT_COLOUR: &str = "build_insert_colour_pick";
pub const PICK_INSERT_PART: &str = "build_insert_part_pick";

/// Discord shows at most this many options in one select menu.
pub const PAGE_SIZE: usize = 25;

/// The value standing for "no colour", which is a real answer: a legend line
/// without one is valid and takes the drawing's default.
pub const NO_COLOUR: &str = "none";

/// How many pages a list of this length needs.
#[must_use]
pub const fn page_count(total: usize) -> usize {
    if total == 0 {
        1
    } else {
        total.div_ceil(PAGE_SIZE)
    }
}

/// The slice of a list shown on a page, clamping a page out of range.
///
/// A stale button from an older message must land somewhere sensible rather
/// than on an empty menu Discord would reject.
#[must_use]
pub fn page_bounds(total: usize, page: usize) -> (usize, usize) {
    let page = page.min(page_count(total).saturating_sub(1));
    let start = page * PAGE_SIZE;
    (start, (start + PAGE_SIZE).min(total))
}

/// The paging row under a picker.
///
/// `Prev` is disabled on the first page and `Next` on the last, so the ends of
/// the list are visible rather than something to discover by clicking.
fn page_buttons(id: &str, arg: &str, page: usize, total: usize, locale: &str) -> CreateActionRow {
    let last = page_count(total).saturating_sub(1);
    let page = page.min(last);
    let label = |key: &str| truncate(&t!(key, locale = locale), MAX_BUTTON_LABEL);

    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{id}:{arg}:{}", page.saturating_sub(1)))
            .label(label("command.build.page_prev"))
            .style(ButtonStyle::Secondary)
            .disabled(page == 0),
        CreateButton::new(format!("{id}:{arg}:{}", page + 1))
            .label(t!(
                "command.build.page_of",
                locale = locale,
                page = page + 1,
                total = last + 1
            ))
            .style(ButtonStyle::Secondary)
            .disabled(page >= last),
    ])
}

/// One page of the colour step.
///
/// The palette runs to 74 entries, so it cannot be shown at once — which is
/// why this pages rather than truncating the way the rest of the bot does.
pub fn build_colour_page(draft_id: &str, page: usize, locale: &str) -> Vec<CreateActionRow> {
    use serenity::all::{CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption};

    let colours: Vec<(u16, &'static str)> = crate::ldraw::colors::all().collect();
    let (start, end) = page_bounds(colours.len(), page);

    let mut options: Vec<CreateSelectMenuOption> = Vec::new();
    // Offered on every page, because a line with no colour is a real answer
    // and hunting for it would be worse than repeating it.
    options.push(CreateSelectMenuOption::new(
        truncate(
            &t!("command.build.insert_no_colour", locale = locale),
            MAX_SELECT_OPTION_LABEL,
        ),
        NO_COLOUR,
    ));
    for (code, name) in &colours[start..end] {
        options.push(CreateSelectMenuOption::new(
            truncate(name, MAX_SELECT_OPTION_LABEL),
            code.to_string(),
        ));
    }

    let menu = CreateSelectMenu::new(
        format!("{PICK_INSERT_COLOUR}:{draft_id}"),
        CreateSelectMenuKind::String { options },
    )
    .placeholder(truncate(
        &t!("command.build.insert_colour_prompt", locale = locale),
        MAX_SELECT_OPTION_LABEL,
    ));

    vec![
        CreateActionRow::SelectMenu(menu),
        page_buttons(PAGE_INSERT_COLOUR, draft_id, page, colours.len(), locale),
    ]
}

/// One page of the part step, carrying the colour already chosen.
///
/// Shows the built-in catalogue rather than the cache: this is the browse
/// route, and the cache could hold thousands of parts nobody wants to page
/// through. Anything unusual is reached by typing in `/build-legend`.
pub fn build_part_page(
    draft_id: &str,
    colour: &str,
    page: usize,
    locale: &str,
) -> Vec<CreateActionRow> {
    use serenity::all::{CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption};

    let parts: Vec<crate::build::PartRef> = crate::build::parts::catalogue(None).collect();
    let (start, end) = page_bounds(parts.len(), page);

    let options: Vec<CreateSelectMenuOption> = parts[start..end]
        .iter()
        .map(|part| {
            CreateSelectMenuOption::new(
                truncate(
                    &format!("{}x{} {}", part.footprint.0, part.footprint.1, part.name),
                    MAX_SELECT_OPTION_LABEL,
                ),
                // The value is the phrase a legend line wants, not the id, so
                // the line reads as something a person would have typed.
                truncate(
                    &format!(
                        "{}x{} {}",
                        part.footprint.0,
                        part.footprint.1,
                        part.kind.as_str()
                    ),
                    MAX_SELECT_OPTION_LABEL,
                ),
            )
        })
        .collect();

    let menu = CreateSelectMenu::new(
        format!("{PICK_INSERT_PART}:{draft_id}:{colour}"),
        CreateSelectMenuKind::String { options },
    )
    .placeholder(truncate(
        &t!("command.build.insert_part_prompt", locale = locale),
        MAX_SELECT_OPTION_LABEL,
    ));

    vec![
        CreateActionRow::SelectMenu(menu),
        page_buttons(
            PAGE_INSERT_PART,
            &format!("{draft_id}:{colour}"),
            page,
            parts.len(),
            locale,
        ),
    ]
}

/// The private drawing form.
///
/// Four inputs, within Discord's cap of five. A modal holds nothing but text
/// inputs — no dropdowns, no buttons — so parts and colours are picked either
/// side of it rather than inside it.
pub fn build_draw_modal(
    draft_id: &str,
    fields: &crate::build::draft::DraftFields,
    locale: &str,
) -> serenity::all::CreateModal {
    use serenity::all::{CreateInputText, CreateModal, InputTextStyle};

    let label = |key: &str| truncate(&t!(key, locale = locale), 45);

    CreateModal::new(
        format!("{MODAL_DRAW}:{draft_id}"),
        truncate(&t!("command.build.draw_title", locale = locale), 45),
    )
    .components(vec![
        CreateActionRow::InputText(
            CreateInputText::new(
                InputTextStyle::Short,
                label("command.build.draw_field_view"),
                "view",
            )
            .value(&fields.view)
            .placeholder("top")
            .required(true),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(
                InputTextStyle::Paragraph,
                label("command.build.draw_field_legend"),
                "legend",
            )
            .value(&fields.legend)
            .placeholder("b = blue 1x1 brick")
            .required(true),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(
                InputTextStyle::Paragraph,
                label("command.build.draw_field_grid"),
                "grid",
            )
            .value(&fields.grid)
            .placeholder(".bb.\nrbbr")
            .required(true),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(
                InputTextStyle::Short,
                label("command.build.draw_field_location"),
                "location",
            )
            .value(&fields.location)
            .placeholder("at origin")
            .required(false),
        ),
    ])
}

/// The buttons under a private preview.
///
/// `stale` swaps `Apply` for `Re-preview`: once the model has moved, what the
/// builder is looking at is no longer what would land, so the only honest
/// next step is to look again.
pub fn build_draft_buttons(draft_id: &str, locale: &str, stale: bool) -> Vec<CreateActionRow> {
    let label = |key: &str| truncate(&t!(key, locale = locale), MAX_BUTTON_LABEL);

    let mut first = vec![
        CreateButton::new(format!("{BUTTON_DRAFT_EDIT}:{draft_id}"))
            .label(label("command.build.draft_edit"))
            .style(ButtonStyle::Primary),
    ];
    if stale {
        first.push(
            CreateButton::new(format!("{BUTTON_DRAFT_REPREVIEW}:{draft_id}"))
                .label(label("command.build.draft_repreview"))
                .style(ButtonStyle::Primary),
        );
    } else {
        first.push(
            CreateButton::new(format!("{BUTTON_DRAFT_APPLY}:{draft_id}"))
                .label(label("command.build.draft_apply"))
                .style(ButtonStyle::Success),
        );
    }
    first.push(
        CreateButton::new(format!("{BUTTON_INSERT}:{draft_id}"))
            .label(label("command.build.insert_button"))
            .style(ButtonStyle::Secondary),
    );
    first.push(
        CreateButton::new(format!("{BUTTON_DRAFT_DISCARD}:{draft_id}"))
            .label(label("command.build.draft_discard"))
            .style(ButtonStyle::Secondary),
    );

    vec![CreateActionRow::Buttons(first)]
}

/// The body of a private preview: the drawing, then whatever the interpreter
/// had to say about it.
///
/// Capped to one Discord message — a preview that cannot be posted is worse
/// than a shortened one.
#[must_use]
pub fn draft_preview_message(preview: &crate::build::draft::Preview, locale: &str) -> String {
    crate::build::report::draft_preview(preview, locale)
}

/// Builds the slash command, with the name and argument localized.
///
/// The command name *is* the translated string — an `en-US` guild gets
/// `/build` and a `fi-FI` guild gets `/rakenna` — matching how `/part` and
/// `/set` are registered.
pub fn build_build_command(locale: &str) -> CreateCommand {
    let cmd_name = t!("command.build.name", locale = locale).to_string();
    let cmd_desc = t!("command.build.desc", locale = locale).to_string();
    let arg_name = t!("command.build.name_arg_name", locale = locale).to_string();
    let arg_desc = t!("command.build.name_desc", locale = locale).to_string();

    let option =
        CreateCommandOption::new(CommandOptionType::String, &arg_name, &arg_desc).required(true);

    CreateCommand::new(&cmd_name)
        .description(&cmd_desc)
        .add_option(option)
}

/// Builds `/build-legend`, which adds a line to an open private drawing.
///
/// A modal holds nothing but text boxes, so parts and colours cannot be picked
/// inside the form. This command edits the draft from outside it, with both
/// options autocompleted — which is the only live typed-ahead Discord offers.
pub fn build_legend_command(locale: &str) -> CreateCommand {
    let name = t!("command.build.legend_name", locale = locale).to_string();
    let desc = t!("command.build.legend_desc", locale = locale).to_string();

    let part = CreateCommandOption::new(
        CommandOptionType::String,
        t!("command.build.legend_part_name", locale = locale).to_string(),
        t!("command.build.legend_part_desc", locale = locale).to_string(),
    )
    .required(true)
    .set_autocomplete(true);

    let colour = CreateCommandOption::new(
        CommandOptionType::String,
        t!("command.build.legend_colour_name", locale = locale).to_string(),
        t!("command.build.legend_colour_desc", locale = locale).to_string(),
    )
    .required(false)
    .set_autocomplete(true);

    // Chosen automatically when it is left out, preferring the colour's own
    // initial, so a legend reads the way a person would have written it.
    let symbol = CreateCommandOption::new(
        CommandOptionType::String,
        t!("command.build.legend_symbol_name", locale = locale).to_string(),
        t!("command.build.legend_symbol_desc", locale = locale).to_string(),
    )
    .required(false);

    CreateCommand::new(name)
        .description(desc)
        .add_option(part)
        .add_option(colour)
        .add_option(symbol)
}

/// The names `/build-legend`'s arguments may arrive under, across locales.
///
/// Option names are localized, so the handler matches a set rather than one
/// string — the same way [`option_name_matches`] does for `/build`.
#[must_use]
pub fn legend_option(name: &str) -> Option<LegendOption> {
    Some(match name {
        "part" | "osa" => LegendOption::Part,
        "colour" | "color" | "vari" => LegendOption::Colour,
        "symbol" | "merkki" => LegendOption::Symbol,
        _ => return None,
    })
}

/// Which `/build-legend` argument a value arrived under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegendOption {
    Part,
    Colour,
    Symbol,
}

/// The names the `/build` argument may arrive under, across locales.
#[must_use]
pub fn option_name_matches(name: &str) -> bool {
    matches!(name, "name" | "nimi" | "model" | "malli")
}

/// The announcement posted when a session opens.
pub fn build_intro_embed(model_name: &str, locale: &str) -> CreateEmbed {
    CreateEmbed::new()
        .title(t!(
            "command.build.intro_title",
            locale = locale,
            name = model_name
        ))
        .description(t!("command.build.intro_body", locale = locale))
        .field(
            "\u{200b}",
            t!("command.build.intro_examples", locale = locale),
            false,
        )
        .color(0x00E0_9B3D)
}

/// The buttons attached to every render.
pub fn build_session_buttons(
    session_id: &str,
    locale: &str,
    enable_export: bool,
) -> CreateActionRow {
    let label = |key: &str| truncate(&t!(key, locale = locale), MAX_BUTTON_LABEL);

    let mut buttons = Vec::new();
    if enable_export {
        buttons.push(
            CreateButton::new(format!("{BUTTON_EXPORT}:{session_id}"))
                .label(label("command.build.export_button"))
                .style(ButtonStyle::Primary),
        );
    }
    buttons.push(
        CreateButton::new(format!("{BUTTON_DRAW}:{session_id}"))
            .label(label("command.build.draw_button"))
            .style(ButtonStyle::Secondary),
    );
    buttons.push(
        CreateButton::new(format!("{BUTTON_UNDO}:{session_id}"))
            .label(label("command.build.undo_button"))
            .style(ButtonStyle::Secondary),
    );
    buttons.push(
        CreateButton::new(format!("{BUTTON_FINISH}:{session_id}"))
            .label(label("command.build.finish_button"))
            .style(ButtonStyle::Danger),
    );

    CreateActionRow::Buttons(buttons)
}

/// The caption posted with a rendered image.
#[must_use]
pub fn render_caption(model: &Model, revision: usize, locale: &str) -> String {
    crate::build::report::render_caption(model, revision, locale)
}

/// The message posted when a session ends.
#[must_use]
pub fn finished_message(model: &Model, locale: &str) -> String {
    t!(
        "command.build.finished",
        locale = locale,
        name = &model.name,
        parts = model.len()
    )
    .to_string()
}

/// Trims a model name to something usable as a thread title.
///
/// Discord caps channel names at 100 characters; an empty name falls back to a
/// generic one rather than producing an unnamed thread.
#[must_use]
pub fn thread_name(model_name: &str, locale: &str) -> String {
    let trimmed = model_name.trim();
    if trimmed.is_empty() {
        return t!("command.build.default_thread_name", locale = locale).to_string();
    }
    truncate(trimmed, 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_name_is_localized() {
        let english = serde_json::to_value(build_build_command("en-US")).unwrap();
        assert_eq!(english["name"], "build");

        let finnish = serde_json::to_value(build_build_command("fi-FI")).unwrap();
        assert_eq!(finnish["name"], "rakenna");
    }

    #[test]
    fn the_command_takes_one_required_name_argument() {
        let json = serde_json::to_value(build_build_command("en-US")).unwrap();
        let options = json["options"].as_array().unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0]["name"], "name");
        assert_eq!(options[0]["required"], true);

        // The argument name is localized too, which is why handlers match a
        // set of aliases.
        let finnish = serde_json::to_value(build_build_command("fi-FI")).unwrap();
        assert_eq!(finnish["options"][0]["name"], "nimi");
    }

    #[test]
    fn both_localized_argument_names_are_recognised() {
        assert!(option_name_matches("name"));
        assert!(option_name_matches("nimi"));
        assert!(!option_name_matches("query"));
    }

    #[test]
    fn the_intro_embed_names_the_model_and_shows_examples() {
        let json = serde_json::to_value(build_intro_embed("Castle", "en-US")).unwrap();
        assert!(json["title"].as_str().unwrap().contains("Castle"));

        let body = json["description"].as_str().unwrap();
        assert!(!body.is_empty());
        assert!(!body.contains("%{"), "unsubstituted placeholder: {body}");

        let examples = json["fields"][0]["value"].as_str().unwrap();
        assert!(examples.contains("Add a red 2x4 brick"), "got: {examples}");
    }

    #[test]
    fn the_intro_embed_is_localized() {
        let english = serde_json::to_value(build_intro_embed("Castle", "en-US")).unwrap();
        let finnish = serde_json::to_value(build_intro_embed("Castle", "fi-FI")).unwrap();
        assert_ne!(english["description"], finnish["description"]);
        // The model name survives translation.
        assert!(finnish["title"].as_str().unwrap().contains("Castle"));
    }

    #[test]
    fn the_buttons_carry_the_session_id() {
        let json =
            serde_json::to_value(build_session_buttons("abc123def456", "en-US", true)).unwrap();
        let ids: Vec<&str> = json["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["custom_id"].as_str().unwrap())
            .collect();

        assert_eq!(ids.len(), 4);
        assert!(ids.contains(&"build_export:abc123def456"));
        assert!(ids.contains(&"build_draw:abc123def456"));
        assert!(ids.contains(&"build_undo:abc123def456"));
        assert!(ids.contains(&"build_finish:abc123def456"));
    }

    #[test]
    fn custom_ids_stay_inside_discords_limit() {
        // Session ids are 12 characters, so this has plenty of headroom, but
        // the limit is worth asserting because a long id would break the
        // component silently.
        let json =
            serde_json::to_value(build_session_buttons("abc123def456", "en-US", true)).unwrap();
        for component in json["components"].as_array().unwrap() {
            let id = component["custom_id"].as_str().unwrap();
            assert!(id.len() <= 100, "custom_id too long: {id}");
        }
    }

    #[test]
    fn the_export_button_can_be_turned_off() {
        let json = serde_json::to_value(build_session_buttons("abc", "en-US", false)).unwrap();
        let ids: Vec<&str> = json["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["custom_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 3);
        assert!(!ids.iter().any(|id| id.starts_with("build_export")));
        // Drafting is not an export, so it survives the export switch.
        assert!(ids.iter().any(|id| id.starts_with("build_draw")));
    }

    /// The rows of a picker, as `(custom_id, disabled)` for buttons and the
    /// menu's option values.
    fn menu_values(rows: &serde_json::Value) -> Vec<String> {
        rows[0]["components"][0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["value"].as_str().unwrap().to_string())
            .collect()
    }

    fn nav(rows: &serde_json::Value) -> Vec<(String, bool)> {
        rows[1]["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["custom_id"].as_str().unwrap().to_string(),
                    b.get("disabled").and_then(serde_json::Value::as_bool) == Some(true),
                )
            })
            .collect()
    }

    #[test]
    fn a_list_is_divided_into_whole_pages() {
        assert_eq!(page_count(0), 1, "an empty list is still one page");
        assert_eq!(page_count(1), 1);
        assert_eq!(page_count(PAGE_SIZE), 1);
        assert_eq!(page_count(PAGE_SIZE + 1), 2);
        assert_eq!(page_count(74), 3);
    }

    #[test]
    fn a_page_out_of_range_lands_on_the_last_one() {
        // A stale button from an older message must not produce an empty menu,
        // which Discord rejects outright.
        assert_eq!(page_bounds(74, 99), (50, 74));
        assert_eq!(page_bounds(10, 5), (0, 10));
        assert_eq!(page_bounds(0, 3), (0, 0));
    }

    #[test]
    fn pages_cover_the_whole_list_without_gaps_or_overlap() {
        for total in [0, 1, 24, 25, 26, 38, 74, 100] {
            let mut covered = 0;
            for page in 0..page_count(total) {
                let (start, end) = page_bounds(total, page);
                assert_eq!(start, covered, "gap at page {page} of {total}");
                covered = end;
            }
            assert_eq!(covered, total, "did not cover {total}");
        }
    }

    #[test]
    fn the_colour_picker_pages_the_whole_palette() {
        // 74 colours cannot be shown at once, which is the reason this pages
        // at all rather than truncating the way the rest of the bot does.
        let total: usize = crate::ldraw::colors::all().count();
        let mut seen: Vec<String> = Vec::new();
        for page in 0..page_count(total) {
            let rows = serde_json::to_value(build_colour_page("d", page, "en-US")).unwrap();
            seen.extend(menu_values(&rows).into_iter().filter(|v| v != NO_COLOUR));
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), total, "the palette is not fully reachable");
    }

    #[test]
    fn every_colour_page_offers_the_no_colour_choice() {
        // A line without a colour is a real answer, so hunting for it on one
        // particular page would be worse than repeating it.
        for page in 0..3 {
            let rows = serde_json::to_value(build_colour_page("d", page, "en-US")).unwrap();
            assert!(
                menu_values(&rows).contains(&NO_COLOUR.to_string()),
                "page {page}"
            );
        }
    }

    #[test]
    fn paging_stops_at_both_ends() {
        let first = serde_json::to_value(build_colour_page("d", 0, "en-US")).unwrap();
        let buttons = nav(&first);
        assert!(buttons[0].1, "Back should be dead on the first page");
        assert!(!buttons[1].1, "Next should work on the first page");

        let last_page = page_count(crate::ldraw::colors::all().count()) - 1;
        let last = serde_json::to_value(build_colour_page("d", last_page, "en-US")).unwrap();
        let buttons = nav(&last);
        assert!(!buttons[0].1, "Back should work on the last page");
        assert!(buttons[1].1, "Next should be dead on the last page");
    }

    #[test]
    fn the_part_picker_carries_the_colour_between_steps() {
        // The colour rides in the custom id as its LDraw code, so no state is
        // held anywhere between the two steps.
        let rows = serde_json::to_value(build_part_page("d", "1", 0, "en-US")).unwrap();
        assert_eq!(
            rows[0]["components"][0]["custom_id"],
            "build_insert_part_pick:d:1"
        );
        for (id, _) in nav(&rows) {
            assert!(id.starts_with("build_insert_part:d:1:"), "got: {id}");
        }
    }

    #[test]
    fn a_part_choice_reads_as_something_a_legend_would_say() {
        // The value goes straight into `b = blue 2x4 brick`, so it has to be
        // the phrase, not the part number.
        let rows = serde_json::to_value(build_part_page("d", NO_COLOUR, 0, "en-US")).unwrap();
        let values = menu_values(&rows);
        assert!(values.contains(&"2x4 brick".to_string()), "got: {values:?}");
    }

    #[test]
    fn the_part_picker_reaches_the_whole_catalogue() {
        let total = crate::build::parts::catalogue(None).count();
        let mut seen: Vec<String> = Vec::new();
        for page in 0..page_count(total) {
            let rows =
                serde_json::to_value(build_part_page("d", NO_COLOUR, page, "en-US")).unwrap();
            seen.extend(menu_values(&rows));
        }
        assert_eq!(seen.len(), total, "the catalogue is not fully reachable");
    }

    #[test]
    fn picker_labels_stay_within_discords_limits_in_both_locales() {
        for locale in ["en-US", "fi-FI"] {
            for rows in [
                serde_json::to_value(build_colour_page("d", 0, locale)).unwrap(),
                serde_json::to_value(build_part_page("d", NO_COLOUR, 0, locale)).unwrap(),
            ] {
                let menu = &rows[0]["components"][0];
                let options = menu["options"].as_array().unwrap();
                assert!(
                    !options.is_empty(),
                    "{locale}: Discord rejects an empty menu"
                );
                assert!(options.len() <= PAGE_SIZE + 1, "{locale}: too many options");
                for option in options {
                    let label = option["label"].as_str().unwrap();
                    assert!(!label.is_empty());
                    assert!(
                        label.chars().count() <= MAX_SELECT_OPTION_LABEL,
                        "{locale}: {label}"
                    );
                    assert!(!label.contains("%{"), "{locale}: {label}");
                }
                for button in rows[1]["components"].as_array().unwrap() {
                    let label = button["label"].as_str().unwrap();
                    assert!(
                        label.chars().count() <= MAX_BUTTON_LABEL,
                        "{locale}: {label}"
                    );
                    assert!(!label.contains("%{"), "{locale}: {label}");
                }
            }
        }
    }

    #[test]
    fn the_legend_command_takes_a_part_a_colour_and_a_symbol() {
        let json = serde_json::to_value(build_legend_command("en-US")).unwrap();
        let options = json["options"].as_array().unwrap();
        assert_eq!(options.len(), 3);

        // Only the part is required: a colour-less line is still valid, and
        // the symbol is chosen for you.
        assert_eq!(options[0]["name"], "part");
        assert_eq!(options[0]["required"], true);
        assert_eq!(options[1]["name"], "colour");
        assert_eq!(options[1]["required"], false);
        assert_eq!(options[2]["name"], "symbol");
        assert_eq!(options[2]["required"], false);
    }

    #[test]
    fn the_legend_commands_part_and_colour_ask_for_suggestions() {
        // Without this Discord never sends an autocomplete interaction, and
        // the whole point of the command is lost.
        for locale in ["en-US", "fi-FI"] {
            let json = serde_json::to_value(build_legend_command(locale)).unwrap();
            let options = json["options"].as_array().unwrap();
            assert_eq!(options[0]["autocomplete"], true, "{locale}: part");
            assert_eq!(options[1]["autocomplete"], true, "{locale}: colour");
            // A symbol is a single character the builder chooses, not a search.
            assert!(options[2].get("autocomplete").is_none_or(|v| v == false));
        }
    }

    #[test]
    fn the_legend_command_is_named_something_the_dispatcher_answers() {
        // The command name is localized and `main.rs` matches it exactly, so
        // the two must agree or the command silently never fires in one locale.
        for locale in ["en-US", "fi-FI"] {
            let json = serde_json::to_value(build_legend_command(locale)).unwrap();
            let name = json["name"].as_str().unwrap();
            assert!(
                matches!(name, "build-legend" | "selite"),
                "{locale}: main.rs does not dispatch {name:?}"
            );
            // Discord rejects names with spaces or capitals.
            assert!(
                name.chars()
                    .all(|c| c.is_lowercase() || c.is_numeric() || c == '-' || c == '_'),
                "{locale}: {name:?} is not a valid command name"
            );
        }
    }

    #[test]
    fn the_legend_options_are_named_something_both_routers_answer() {
        // The same trap one level down: option names are localized, and both
        // the handler and the autocomplete router match on them.
        for locale in ["en-US", "fi-FI"] {
            let json = serde_json::to_value(build_legend_command(locale)).unwrap();
            for option in json["options"].as_array().unwrap() {
                let name = option["name"].as_str().unwrap();
                assert!(
                    legend_option(name).is_some(),
                    "{locale}: the handler does not read {name:?}"
                );
            }
        }
    }

    #[test]
    fn legend_option_names_map_to_their_meanings() {
        assert_eq!(legend_option("part"), Some(LegendOption::Part));
        assert_eq!(legend_option("osa"), Some(LegendOption::Part));
        assert_eq!(legend_option("colour"), Some(LegendOption::Colour));
        assert_eq!(legend_option("vari"), Some(LegendOption::Colour));
        assert_eq!(legend_option("merkki"), Some(LegendOption::Symbol));
        assert_eq!(legend_option("query"), None);
    }

    #[test]
    fn the_legend_command_is_described_in_both_locales() {
        for locale in ["en-US", "fi-FI"] {
            let json = serde_json::to_value(build_legend_command(locale)).unwrap();
            assert!(!json["description"].as_str().unwrap().contains("%{"));
            for option in json["options"].as_array().unwrap() {
                let desc = option["description"].as_str().unwrap();
                assert!(!desc.is_empty(), "{locale}");
                assert!(!desc.contains("%{"), "{locale}: {desc}");
                // Discord caps option descriptions at 100 characters.
                assert!(desc.chars().count() <= 100, "{locale}: {desc}");
            }
        }
    }

    #[test]
    fn the_drawing_form_has_four_text_inputs_carrying_the_draft_id() {
        let fields = crate::build::draft::DraftFields::default();
        let json = serde_json::to_value(build_draw_modal("draft123", &fields, "en-US")).unwrap();

        assert_eq!(json["custom_id"], "build_draw_modal:draft123");
        let rows = json["components"].as_array().unwrap();
        // Discord allows five; a modal holds nothing but text inputs, which is
        // why parts and colours are picked either side of it.
        assert_eq!(rows.len(), 4);

        let ids: Vec<&str> = rows
            .iter()
            .map(|row| row["components"][0]["custom_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["view", "legend", "grid", "location"]);
    }

    #[test]
    fn the_drawing_form_opens_prefilled() {
        let fields = crate::build::draft::DraftFields {
            view: "north".to_string(),
            legend: "b = blue 1x1 brick".to_string(),
            grid: "bb".to_string(),
            location: "at origin".to_string(),
        };
        let json = serde_json::to_value(build_draw_modal("d", &fields, "en-US")).unwrap();
        let rows = json["components"].as_array().unwrap();
        assert_eq!(rows[0]["components"][0]["value"], "north");
        assert_eq!(rows[1]["components"][0]["value"], "b = blue 1x1 brick");
        assert_eq!(rows[2]["components"][0]["value"], "bb");
    }

    #[test]
    fn the_form_title_and_labels_fit_discords_limits_in_both_locales() {
        let fields = crate::build::draft::DraftFields::default();
        for locale in ["en-US", "fi-FI"] {
            let json = serde_json::to_value(build_draw_modal("d", &fields, locale)).unwrap();
            let title = json["title"].as_str().unwrap();
            assert!(
                title.chars().count() <= crate::discord_limits::MAX_MODAL_TITLE,
                "{locale}: {title}"
            );
            assert!(!title.contains("%{"));

            for row in json["components"].as_array().unwrap() {
                let label = row["components"][0]["label"].as_str().unwrap();
                assert!(label.chars().count() <= 45, "{locale}: {label}");
                assert!(!label.is_empty());
                assert!(!label.contains("%{"));
            }
        }
    }

    #[test]
    fn a_fresh_preview_offers_apply_but_a_stale_one_offers_another_look() {
        let fresh = serde_json::to_value(build_draft_buttons("d", "en-US", false)).unwrap();
        let ids = |v: &serde_json::Value| -> Vec<String> {
            v[0]["components"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["custom_id"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(
            ids(&fresh),
            vec![
                "build_draft_edit:d",
                "build_draft_apply:d",
                "build_insert:d",
                "build_draft_discard:d"
            ]
        );

        // Once the model has moved, applying what was previewed would land
        // something the builder never saw, so the only offer is to look again.
        let stale = serde_json::to_value(build_draft_buttons("d", "en-US", true)).unwrap();
        assert_eq!(
            ids(&stale),
            vec![
                "build_draft_edit:d",
                "build_draft_repreview:d",
                "build_insert:d",
                "build_draft_discard:d"
            ]
        );
    }

    #[test]
    fn draft_button_labels_are_localized_and_within_limits() {
        for locale in ["en-US", "fi-FI"] {
            for stale in [false, true] {
                let json = serde_json::to_value(build_draft_buttons("d", locale, stale)).unwrap();
                for component in json[0]["components"].as_array().unwrap() {
                    let label = component["label"].as_str().unwrap();
                    assert!(!label.is_empty());
                    assert!(label.chars().count() <= MAX_BUTTON_LABEL);
                    assert!(!label.contains("%{"));
                }
            }
        }
    }

    #[test]
    fn button_labels_are_localized_and_within_limits() {
        for locale in ["en-US", "fi-FI"] {
            let json = serde_json::to_value(build_session_buttons("abc", locale, true)).unwrap();
            for component in json["components"].as_array().unwrap() {
                let label = component["label"].as_str().unwrap();
                assert!(!label.is_empty());
                assert!(label.chars().count() <= MAX_BUTTON_LABEL);
                assert!(!label.contains("%{"));
            }
        }
    }

    #[test]
    fn the_finished_message_reports_the_model_and_its_size() {
        let model = Model::new("Castle");
        for locale in ["en-US", "fi-FI"] {
            let text = finished_message(&model, locale);
            assert!(text.contains("Castle"), "got: {text}");
            assert!(!text.contains("%{"));
        }
    }

    #[test]
    fn thread_names_are_trimmed_and_never_empty() {
        assert_eq!(thread_name("Castle", "en-US"), "Castle");
        assert_eq!(thread_name("  Castle  ", "en-US"), "Castle");
        assert_eq!(thread_name("", "en-US"), "Build");
        assert_eq!(thread_name("   ", "en-US"), "Build");
        assert_eq!(thread_name("", "fi-FI"), "Rakennelma");
        assert!(thread_name(&"x".repeat(500), "en-US").chars().count() <= 100);
    }
}
