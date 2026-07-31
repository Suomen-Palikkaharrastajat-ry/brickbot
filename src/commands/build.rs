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
use crate::discord_limits::{MAX_BUTTON_LABEL, truncate};
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

        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"build_export:abc123def456"));
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
        assert_eq!(ids.len(), 2);
        assert!(!ids.iter().any(|id| id.starts_with("build_export")));
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
