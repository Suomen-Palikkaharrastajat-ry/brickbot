use crate::config::PocketBaseConfig;
use crate::events_sync::{Event, PbRecord, parse_pb_date};
use chrono::Utc;

use serenity::all::{GuildId, Http, ScheduledEvent, ScheduledEventStatus, ScheduledEventType};
use serenity::builder::{CreateScheduledEvent, EditScheduledEvent};
use sqlx::SqlitePool;

#[allow(clippy::too_many_lines)]
pub async fn upsert_single_event(
    discord_http: &Http,
    http_client: &dyn crate::http::HttpProvider,
    _db: &SqlitePool,
    guild_id: GuildId,
    pb_ev: &Event,
    discord_events: &[ScheduledEvent],
    limit: u64,
) -> anyhow::Result<()> {
    let now = Utc::now();
    if pb_ev.start_time <= now {
        return Ok(()); // Skip past events
    }

    let uid_marker = format!("🆔 {}", pb_ev.uid);
    let updated_marker = format!("🕒 {}", pb_ev.updated);

    let mut discord_event_exists = None;
    let mut needs_update = false;

    if let Some(existing) = discord_events
        .iter()
        .find(|e| e.description.as_deref().unwrap_or("").contains(&uid_marker))
    {
        discord_event_exists = Some(existing);
        let desc = existing.description.as_deref().unwrap_or("");

        let found_timestamp = desc
            .find("🕒 ")
            .map(|idx| {
                let rest = &desc[idx + "🕒 ".len()..];
                let end = rest.find('\n').unwrap_or(rest.len());
                rest[..end].trim().to_string()
            })
            .unwrap_or_default();

        if found_timestamp < pb_ev.updated {
            needs_update = true;
        }

        if !needs_update {
            return Ok(());
        }
    }

    let end_time = pb_ev
        .end_time
        .unwrap_or_else(|| pb_ev.start_time + chrono::Duration::hours(1));

    let location = pb_ev
        .url
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| pb_ev.location.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "TBD".to_string());

    let mut safe_name = pb_ev.summary.clone();
    if let Some(phys_loc) = pb_ev.location.as_ref().filter(|s| !s.is_empty()) {
        safe_name = format!("{safe_name} | {phys_loc}");
    }
    if safe_name.len() > 100 {
        let mut cutoff = 97;
        while !safe_name.is_char_boundary(cutoff) && cutoff > 0 {
            cutoff -= 1;
        }
        safe_name.truncate(cutoff);
        safe_name.push_str("...");
    }

    let mut description = pb_ev.description.clone().unwrap_or_default();
    if description.len() > 950 {
        let mut cutoff = 947;
        while !description.is_char_boundary(cutoff) && cutoff > 0 {
            cutoff -= 1;
        }
        description.truncate(cutoff);
        description.push_str("...");
    }

    let description = format!("{description}\n\n{uid_marker}\n{updated_marker}");

    if let Some(existing) = discord_event_exists {
        let event_id = existing.id;
        let mut edit = EditScheduledEvent::new()
            .name(&safe_name)
            .description(&description)
            .start_time(
                serenity::model::Timestamp::from_unix_timestamp(pb_ev.start_time.timestamp())
                    .unwrap(),
            )
            .end_time(
                serenity::model::Timestamp::from_unix_timestamp(end_time.timestamp()).unwrap(),
            )
            .location(&location);

        if let Some(image_url) = &pb_ev.image_url {
            if let Ok(bytes) = http_client.get_bounded_bytes(image_url, limit, false).await {
                let attachment = serenity::builder::CreateAttachment::bytes(bytes, "cover.png");
                edit = edit.image(&attachment);
            }
        }

        if let Err(e) = guild_id
            .edit_scheduled_event(discord_http, event_id, edit)
            .await
        {
            tracing::error!(
                "Failed to edit scheduled event {} for PB Event '{}' ({}). Error: {}",
                event_id,
                pb_ev.summary,
                pb_ev.uid,
                e
            );
        }
    } else {
        let fresh_events = discord_http.get_scheduled_events(guild_id, false).await?;
        if fresh_events
            .iter()
            .any(|e| e.description.as_deref().unwrap_or("").contains(&uid_marker))
        {
            return Ok(());
        }

        let mut create = CreateScheduledEvent::new(
            ScheduledEventType::External,
            &safe_name,
            serenity::model::Timestamp::from_unix_timestamp(pb_ev.start_time.timestamp()).unwrap(),
        )
        .end_time(serenity::model::Timestamp::from_unix_timestamp(end_time.timestamp()).unwrap())
        .location(&location)
        .description(&description);

        if let Some(image_url) = &pb_ev.image_url {
            if let Ok(bytes) = http_client.get_bounded_bytes(image_url, limit, false).await {
                let attachment = serenity::builder::CreateAttachment::bytes(bytes, "cover.png");
                create = create.image(&attachment);
            }
        }

        if let Err(e) = guild_id.create_scheduled_event(discord_http, create).await {
            tracing::error!(
                "Failed to create scheduled event for PB Event '{}' ({}). Error: {}",
                pb_ev.summary,
                pb_ev.uid,
                e
            );
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn sync_single_realtime_event(
    discord_http: &Http,
    http_client: &dyn crate::http::HttpProvider,
    db: &SqlitePool,
    guild_id: GuildId,
    action: &str,
    record: PbRecord,
    pb_cfg: &PocketBaseConfig,
    limit: u64,
) -> anyhow::Result<()> {
    let mut actual_action = action;
    if action == "update" || action == "create" {
        let hidden = crate::db::get_hidden_events(db, &guild_id.to_string()).await?;
        if hidden.contains(&record.id) {
            actual_action = "delete";
        }
    }

    if actual_action == "delete" {
        let discord_events = discord_http.get_scheduled_events(guild_id, false).await?;
        let uid_marker = format!("🆔 {}", record.id);
        for e in discord_events {
            let desc = e.description.as_deref().unwrap_or("");
            if desc.contains(&uid_marker) {
                let _ = guild_id.delete_scheduled_event(discord_http, e.id).await;
            }
        }
        return Ok(());
    }

    if let Some(start_time) = parse_pb_date(&record.start_date) {
        let end_time = parse_pb_date(&record.end_date);
        let image_url = record.image.as_ref().map(|img| {
            format!(
                "{}/api/files/{}/{}/{}",
                pb_cfg.url.trim_end_matches('/'),
                pb_cfg.collection,
                record.id,
                img
            )
        });
        let ev = Event {
            uid: record.id,
            summary: record.title,
            description: record.description,
            start_time,
            end_time,
            location: record.location,
            url: record.url,
            tags: record.tags,
            image_url,
            is_full_day: record.all_day.unwrap_or(false),
            updated: record.updated,
        };
        let discord_events = discord_http.get_scheduled_events(guild_id, false).await?;
        upsert_single_event(
            discord_http,
            http_client,
            db,
            guild_id,
            &ev,
            &discord_events,
            limit,
        )
        .await?;
    }

    Ok(())
}

pub async fn sync_events_with_discord(
    discord_http: &Http,
    http_client: &dyn crate::http::HttpProvider,
    db: &SqlitePool,
    guild_id: GuildId,
    pb_events: Vec<Event>,
    limit: u64,
) -> anyhow::Result<()> {
    let discord_events = discord_http.get_scheduled_events(guild_id, false).await?;

    let hidden_events = crate::db::get_hidden_events(db, &guild_id.to_string()).await?;
    let hidden_set: std::collections::HashSet<String> = hidden_events.into_iter().collect();

    let mut current_uids = std::collections::HashSet::new();

    for pb_ev in pb_events {
        if hidden_set.contains(&pb_ev.uid) {
            continue;
        }
        current_uids.insert(pb_ev.uid.clone());
        let _ = upsert_single_event(
            discord_http,
            http_client,
            db,
            guild_id,
            &pb_ev,
            &discord_events,
            limit,
        )
        .await;
    }

    for discord_ev in discord_events {
        let desc = discord_ev.description.as_deref().unwrap_or("");
        let found_uid = desc.find("🆔 ").map(|idx| {
            let rest = &desc[idx + "🆔 ".len()..];
            let end = rest.find('\n').unwrap_or(rest.len());
            rest[..end].trim().to_string()
        });

        if let Some(uid) = found_uid {
            if !current_uids.contains(&uid) && discord_ev.status != ScheduledEventStatus::Completed
            {
                let _ = guild_id
                    .delete_scheduled_event(discord_http, discord_ev.id)
                    .await;
            }
        }
    }

    Ok(())
}
