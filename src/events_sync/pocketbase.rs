use crate::config::PocketBaseConfig;
use crate::events_sync::Event;
use chrono::{DateTime, Utc};

#[derive(serde::Deserialize, Debug, Clone)]
pub struct PbRecord {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub start_date: String,
    pub end_date: String,
    pub location: Option<String>,
    pub url: Option<String>,
    pub image: Option<String>,
    pub tags: Option<Vec<String>>,
    pub all_day: Option<bool>,
    pub state: Option<String>,
    pub updated: String,
}

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
pub struct PbResponse {
    pub items: Vec<PbRecord>,
    pub page: u32,
    #[serde(rename = "totalPages")]
    pub total_pages: u32,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct PbRealtimeMessage {
    pub action: String,
    pub record: PbRecord,
}

#[must_use]
pub fn parse_pb_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.replace(' ', "T");
    chrono::DateTime::parse_from_rfc3339(&s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

pub async fn fetch_pocketbase_events(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &PocketBaseConfig,
    limit: u64,
) -> anyhow::Result<Vec<Event>> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    // We only fetch events starting from one month ago to limit data size
    let one_month_ago = (chrono::Utc::now() - chrono::Duration::days(30)).format("%Y-%m-%d");
    let filter = format!("(state=%27published%27%20%26%26%20start_date%3E=%27{one_month_ago}%27)");

    let mut current_page = 1;
    let mut all_events = Vec::new();

    loop {
        let url = format!(
            "{}/api/collections/{}/records?page={}&perPage=100&filter={}",
            pb_cfg.url.trim_end_matches('/'),
            pb_cfg.collection,
            current_page,
            filter
        );

        let res_text = http
            .get_json_with_auth(&url, auth.as_deref(), true, limit)
            .await?;
        let pb_resp: PbResponse = serde_json::from_str(&res_text)?;

        for item in pb_resp.items {
            if let Some(start_time) = parse_pb_date(&item.start_date) {
                let end_time = parse_pb_date(&item.end_date);

                let image_url = item.image.map(|img| {
                    format!(
                        "{}/api/files/{}/{}/{}",
                        pb_cfg.url.trim_end_matches('/'),
                        pb_cfg.collection,
                        item.id,
                        img
                    )
                });

                all_events.push(Event {
                    uid: item.id.clone(),
                    summary: item.title,
                    description: item.description,
                    start_time,
                    end_time,
                    location: item.location,
                    url: item.url,
                    tags: item.tags,
                    image_url,
                    is_full_day: item.all_day.unwrap_or(false),
                    updated: item.updated,
                });
            }
        }

        if current_page >= pb_resp.total_pages {
            break;
        }
        current_page += 1;
    }

    Ok(all_events)
}

pub async fn stream_pocketbase_events(
    http: &dyn crate::http::HttpProvider,
    pb_cfg: &PocketBaseConfig,
    limit: u64,
    tx: &tokio::sync::broadcast::Sender<crate::events_sync::SyncMessage>,
) -> anyhow::Result<()> {
    let token = std::env::var("POCKETBASE_IMPERSONATE_AUTH_TOKEN").ok();
    let auth = token.as_ref().map(|t| format!("Bearer {t}"));

    let one_month_ago = (chrono::Utc::now() - chrono::Duration::days(30)).format("%Y-%m-%d");
    let filter = format!("(state=%27published%27%20%26%26%20start_date%3E=%27{one_month_ago}%27)");

    let mut current_page = 1;

    let _ = tx.send(crate::events_sync::SyncMessage::SyncStart);

    loop {
        let url = format!(
            "{}/api/collections/{}/records?page={}&perPage=100&filter={}",
            pb_cfg.url.trim_end_matches('/'),
            pb_cfg.collection,
            current_page,
            filter
        );

        let res_text = http
            .get_json_with_auth(&url, auth.as_deref(), true, limit)
            .await?;
        let pb_resp: PbResponse = serde_json::from_str(&res_text)?;

        let mut batch = Vec::new();
        for item in pb_resp.items {
            if let Some(start_time) = parse_pb_date(&item.start_date) {
                let end_time = parse_pb_date(&item.end_date);
                let image_url = item.image.map(|img| {
                    format!(
                        "{}/api/files/{}/{}/{}",
                        pb_cfg.url.trim_end_matches('/'),
                        pb_cfg.collection,
                        item.id,
                        img
                    )
                });

                batch.push(Event {
                    uid: item.id.clone(),
                    summary: item.title,
                    description: item.description,
                    start_time,
                    end_time,
                    location: item.location,
                    url: item.url,
                    tags: item.tags,
                    image_url,
                    is_full_day: item.all_day.unwrap_or(false),
                    updated: item.updated,
                });
            }
        }

        let _ = tx.send(crate::events_sync::SyncMessage::BatchSync(batch));

        if current_page >= pb_resp.total_pages {
            break;
        }
        current_page += 1;
    }

    let _ = tx.send(crate::events_sync::SyncMessage::SyncEnd);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_parse_pb_date() {
        let dt1 = parse_pb_date("2023-10-25 15:30:00.000Z").unwrap();
        assert_eq!(dt1, Utc.with_ymd_and_hms(2023, 10, 25, 15, 30, 0).unwrap());

        let dt2 = parse_pb_date("2023-10-25T15:30:00Z").unwrap();
        assert_eq!(dt2, Utc.with_ymd_and_hms(2023, 10, 25, 15, 30, 0).unwrap());

        assert!(parse_pb_date("invalid_date").is_none());
    }
}
