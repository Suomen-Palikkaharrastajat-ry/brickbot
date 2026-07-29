#![allow(unused_imports)]
use serenity::all::{Context, Framework, GuildId};

pub fn build_locations_command(locale: &str) -> serenity::all::CreateCommand {
    use serenity::all::{CommandOptionType, CreateCommand, CreateCommandOption};

    let cmd_name = rust_i18n::t!("command.locations.name", locale = locale).to_string();
    let cmd_desc = rust_i18n::t!("command.locations.desc", locale = locale).to_string();
    let image_arg_name =
        rust_i18n::t!("command.locations.image_arg_name", locale = locale).to_string();
    let image_desc = rust_i18n::t!("command.locations.image_desc", locale = locale).to_string();

    let image_option =
        CreateCommandOption::new(CommandOptionType::Attachment, &image_arg_name, &image_desc)
            .required(false);

    CreateCommand::new(&cmd_name)
        .description(&cmd_desc)
        .add_option(image_option)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_locations_command_localizations() {
        let cmd = build_locations_command("fi-FI");
        let json = serde_json::to_value(&cmd).unwrap();

        assert_eq!(json.get("name").unwrap().as_str().unwrap(), "paikat");
        assert_eq!(
            json.get("description").unwrap().as_str().unwrap(),
            "Hallitse ja ehdota karttapaikkoja"
        );

        let options = json.get("options").unwrap().as_array().unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].get("name").unwrap().as_str().unwrap(), "kuva");
    }

    #[test]
    fn test_build_locations_command_english() {
        let cmd = build_locations_command("en-US");
        let json = serde_json::to_value(&cmd).unwrap();

        assert_eq!(json.get("name").unwrap().as_str().unwrap(), "locations");
    }
}
