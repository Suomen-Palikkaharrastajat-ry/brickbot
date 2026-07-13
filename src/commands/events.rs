#![allow(unused_imports)]
use serenity::all::{Context, Framework, GuildId};

pub fn build_events_command(locale: &str) -> serenity::all::CreateCommand {
    use serenity::all::{CommandOptionType, CreateCommand, CreateCommandOption};

    let cmd_name = rust_i18n::t!("command.events.name", locale = locale).to_string();
    let cmd_desc = rust_i18n::t!("command.events.desc", locale = locale).to_string();
    let image_desc = rust_i18n::t!("command.events.image_desc", locale = locale).to_string();

    let image_option =
        CreateCommandOption::new(CommandOptionType::Attachment, "image", &image_desc)
            .required(false);

    CreateCommand::new(&cmd_name)
        .description(&cmd_desc)
        .add_option(image_option)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_events_command_localizations() {
        let cmd = build_events_command("fi-FI");
        // Since the inner fields of `CreateCommand` are not easily readable from the outside,
        // we can serialize it to JSON using serde and inspect the resulting JSON value!
        let json = serde_json::to_value(&cmd).unwrap();

        let fi_name = json.get("name").unwrap().as_str().unwrap();
        assert_eq!(fi_name, "tapahtumat");

        let fi_desc = json.get("description").unwrap().as_str().unwrap();
        assert_eq!(fi_desc, "Hallitse ja synkronoi tapahtumia");
    }
}
