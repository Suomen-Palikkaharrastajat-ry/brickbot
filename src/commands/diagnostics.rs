use crate::workflows::AppContext;
use rust_i18n::t;
use serenity::all::{
    CommandInteraction, Context, CreateCommand, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage,
};

pub fn build_diagnostics_command(locale: &str) -> CreateCommand {
    let cmd_name = t!("command.diagnostics.name", locale = locale).to_string();
    let cmd_desc = t!("command.diagnostics.desc", locale = locale).to_string();

    CreateCommand::new(&cmd_name).description(&cmd_desc)
}

pub async fn handle_diagnostics_command(
    ctx: &Context,
    app_ctx: &AppContext,
    interaction: &CommandInteraction,
    locale: &str,
) -> anyhow::Result<()> {
    // Check DB
    let db_status = match sqlx::query("SELECT 1").execute(&app_ctx.db).await {
        Ok(_) => t!("command.diagnostics.db_ok", locale = locale).to_string(),
        Err(e) => t!(
            "command.diagnostics.db_err",
            locale = locale,
            err = e.to_string()
        )
        .to_string(),
    };

    // Check PocketBase
    let pb_status = if let Some(pb_cfg) = &app_ctx.config.pocketbase {
        // Just try fetching the events. Even if it returns unauthorized or similar,
        // it verifies connectivity, but `fetch_pocketbase_events` returns Ok if successful.
        match crate::events_sync::fetch_pocketbase_events(
            app_ctx.http.as_ref(),
            pb_cfg,
            app_ctx.config.resource_limits.max_http_body_bytes,
        )
        .await
        {
            Ok(_) => t!("command.diagnostics.pb_ok", locale = locale).to_string(),
            Err(e) => t!(
                "command.diagnostics.pb_err",
                locale = locale,
                err = e.to_string()
            )
            .to_string(),
        }
    } else {
        t!("command.diagnostics.pb_disabled", locale = locale).to_string()
    };

    // Check Zulip
    let zulip_status = if let Some(zulip_cfg) = &app_ctx.config.zulip {
        // Create a test HTTP request to the Zulip endpoint (get streams or just get profile)
        let endpoint = format!("{}/api/v1/users/me", zulip_cfg.url.trim_end_matches('/'));
        let _request = reqwest::Client::new()
            .get(&endpoint)
            .basic_auth(&zulip_cfg.bot_email, Some(""))
            // we don't have the API key here easily except if it's implicitly passed?
            // Actually, in Zulip config, do we have an API key?
            // Let's check `zulip_cfg`. The config only has `url` and `bot_email`. The API key is in ZULIP_API_KEY env.
            .send()
            .await;

        // For diagnostics, maybe just making an API call to test connectivity is fine.
        // Wait, the API key is passed as the password for basic_auth.
        let api_key = std::env::var("ZULIP_API_KEY").unwrap_or_default();

        let request = reqwest::Client::new()
            .get(&endpoint)
            .basic_auth(&zulip_cfg.bot_email, Some(&api_key))
            .send()
            .await;

        match request {
            Ok(resp) => {
                if resp.status().is_success() {
                    t!("command.diagnostics.zulip_ok", locale = locale).to_string()
                } else {
                    t!(
                        "command.diagnostics.zulip_err",
                        locale = locale,
                        err = format!("HTTP {}", resp.status())
                    )
                    .to_string()
                }
            }
            Err(e) => t!(
                "command.diagnostics.zulip_err",
                locale = locale,
                err = e.to_string()
            )
            .to_string(),
        }
    } else {
        t!("command.diagnostics.zulip_disabled", locale = locale).to_string()
    };

    let title = t!("command.diagnostics.title", locale = locale).to_string();

    let description = format!("{db_status}\n{pb_status}\n{zulip_status}");

    let embed = CreateEmbed::new()
        .title(title)
        .description(description)
        .color(0x0000_FF00); // Green

    let data = CreateInteractionResponseMessage::new()
        .add_embed(embed)
        .ephemeral(true);

    interaction
        .create_response(&ctx.http, CreateInteractionResponse::Message(data))
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_diagnostics_command_localizations() {
        let cmd = build_diagnostics_command("fi-FI");
        let json = serde_json::to_value(&cmd).unwrap();

        let fi_name = json.get("name").unwrap().as_str().unwrap();
        assert_eq!(fi_name, "diagnostiikka");

        let fi_desc = json.get("description").unwrap().as_str().unwrap();
        assert_eq!(
            fi_desc,
            "Suorita järjestelmän diagnostiikka tarkistaaksesi botin tilan"
        );
    }
}
