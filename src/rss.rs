#![allow(clippy::or_fun_call, unused_imports, clippy::cast_possible_wrap)]
use crate::http::{HttpClient, HttpProvider};
use anyhow::Result;
use rss::Channel;
use serenity::all::{ChannelId, Framework, FutureExt, Http};
use sqlx::SqlitePool;

use std::sync::Arc;
use tokio::time::{Duration, sleep};

use crate::db::insert_feed_item;

#[derive(Debug)]
pub enum Feed {
    Rss(Box<rss::Channel>),
    Atom(Box<atom_syndication::Feed>),
}

pub fn parse_feed(body: &[u8]) -> Result<Feed> {
    if let Ok(channel) = rss::Channel::read_from(body) {
        return Ok(Feed::Rss(Box::new(channel)));
    }

    if let Ok(feed) = atom_syndication::Feed::read_from(body) {
        return Ok(Feed::Atom(Box::new(feed)));
    }

    anyhow::bail!("the input could not be parsed as RSS or Atom");
}

pub async fn fetch_feed(client: &HttpClient, url: &str, limit: u64) -> Result<Feed> {
    let body = client.get_bounded_bytes(url, limit, false).await?;
    parse_feed(&body[..])
}

pub async fn poll_once(
    _http: &Arc<Http>,
    db: &SqlitePool,
    feed_url: &str,
    client: &HttpClient,
    limit: u64,
) -> anyhow::Result<()> {
    let feed = fetch_feed(client, feed_url, limit).await?;

    match feed {
        Feed::Rss(channel) => {
            let source_title = channel.title();
            for item in channel.items() {
                let id = item
                    .link()
                    .or_else(|| item.guid().map(rss::Guid::value))
                    .unwrap_or_default();
                let title = item.title().unwrap_or_default();
                let description = item.description().unwrap_or_default();

                if !id.is_empty() {
                    let _ = insert_feed_item(db, id, source_title, title, description).await;
                }
            }
        }
        Feed::Atom(feed) => {
            let source_title = feed.title.as_str();
            for entry in &feed.entries {
                let id = entry
                    .links
                    .first()
                    .map_or(entry.id.as_str(), |l| l.href.as_str());
                let title = entry.title.as_str();
                let description = entry
                    .summary
                    .as_ref()
                    .map(atom_syndication::Text::as_str)
                    .or_else(|| entry.content.as_ref().and_then(|c| c.value.as_deref()))
                    .unwrap_or_default();

                if !id.is_empty() {
                    let _ = insert_feed_item(db, id, source_title, title, description).await;
                }
            }
        }
    }

    Ok(())
}

fn extract_opml_urls(outlines: &[opml::Outline], urls: &mut Vec<String>) {
    for outline in outlines {
        if let Some(xml_url) = &outline.xml_url {
            urls.push(xml_url.clone());
        }
        extract_opml_urls(&outline.outlines, urls);
    }
}

pub async fn global_poll_loop(http: Arc<Http>, db: SqlitePool, config: Arc<crate::config::Config>) {
    let client = HttpClient::new();
    let mut opml_cache: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    loop {
        let mut feed_intervals: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();

        for rss_feed in &config.feeds {
            let interval = config.poll_interval.unwrap_or(3600);

            for opml_url in &rss_feed.opml_urls {
                let last_polled_at = crate::db::get_last_polled_at(&db, opml_url).await;
                let mut should_poll = false;
                if !opml_cache.contains_key(opml_url) {
                    should_poll = true;
                } else if let Some(last_polled_at) = last_polled_at {
                    let duration_since = chrono::Utc::now().naive_utc() - last_polled_at;
                    if duration_since.num_seconds() >= interval as i64 {
                        should_poll = true;
                    }
                } else {
                    should_poll = true;
                }

                if should_poll {
                    if let Ok(opml_content) = client
                        .get_bounded_text(
                            opml_url,
                            config.resource_limits.max_http_body_bytes,
                            false,
                        )
                        .await
                    {
                        if let Ok(document) = opml::OPML::from_str(&opml_content) {
                            let mut extracted = Vec::new();
                            extract_opml_urls(&document.body.outlines, &mut extracted);
                            opml_cache.insert(opml_url.clone(), extracted);

                            let _ = crate::db::mark_polled(&db, opml_url).await;
                        } else {
                            tracing::error!("failed to parse OPML from {}", opml_url);
                        }
                    } else {
                        tracing::error!("failed to fetch OPML from {}", opml_url);
                    }
                }

                if let Some(extracted) = opml_cache.get(opml_url) {
                    for url in extracted {
                        let e = feed_intervals.entry(url.clone()).or_insert(interval);
                        *e = (*e).min(interval);
                    }
                }
            }

            for feed_url in &rss_feed.feed_urls {
                let e = feed_intervals.entry(feed_url.clone()).or_insert(interval);
                *e = (*e).min(interval);
            }
        }

        for (feed_url, interval) in feed_intervals {
            let last_polled_at = crate::db::get_last_polled_at(&db, &feed_url).await;
            let mut should_poll = false;
            if let Some(last_polled_at) = last_polled_at {
                let duration_since = chrono::Utc::now().naive_utc() - last_polled_at;
                if duration_since.num_seconds() >= interval as i64 {
                    should_poll = true;
                }
            } else {
                should_poll = true;
            }

            if should_poll {
                if let Err(err) = poll_once(
                    &http,
                    &db,
                    &feed_url,
                    &client,
                    config.resource_limits.max_http_body_bytes,
                )
                .await
                {
                    tracing::error!("poll error for {}: {}", feed_url, err);
                }

                let _ = crate::db::mark_polled(&db, &feed_url).await;
            }
        }

        sleep(Duration::from_mins(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_opml_urls() {
        let opml_str = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
        <opml version="2.0">
            <head>
                <title>mySubscriptions.opml</title>
            </head>
            <body>
                <outline text="News">
                    <outline text="CNN" type="rss" xmlUrl="http://rss.cnn.com/rss/cnn_topstories.rss" />
                    <outline text="NYT" type="rss" xmlUrl="https://rss.nytimes.com/services/xml/rss/nyt/HomePage.xml" />
                </outline>
            </body>
        </opml>"#;

        let document = opml::OPML::from_str(opml_str).unwrap();
        let mut urls = Vec::new();
        extract_opml_urls(&document.body.outlines, &mut urls);

        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "http://rss.cnn.com/rss/cnn_topstories.rss");
        assert_eq!(
            urls[1],
            "https://rss.nytimes.com/services/xml/rss/nyt/HomePage.xml"
        );
    }

    #[test]
    fn test_parse_rss_feed() {
        let rss_data = r#"<?xml version="1.0" encoding="UTF-8" ?>
        <rss version="2.0">
        <channel>
          <title>W3Schools Home Page</title>
          <link>https://www.w3schools.com</link>
          <description>Free web building tutorials</description>
          <item>
            <title>RSS Tutorial</title>
            <link>https://www.w3schools.com/xml/xml_rss.asp</link>
            <description>New RSS tutorial on W3Schools</description>
          </item>
        </channel>
        </rss>"#;

        let feed = parse_feed(rss_data.as_bytes()).expect("Failed to parse valid RSS");
        if let Feed::Rss(channel) = feed {
            assert_eq!(channel.title, "W3Schools Home Page");
            assert_eq!(channel.items.len(), 1);
            assert_eq!(channel.items[0].title.as_deref(), Some("RSS Tutorial"));
        } else {
            panic!("Expected Feed::Rss");
        }
    }

    #[test]
    fn test_parse_atom_feed() {
        let atom_data = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <title>Example Feed</title>
          <link href="http://example.org/"/>
          <updated>2003-12-13T18:30:02Z</updated>
          <author>
            <name>John Doe</name>
          </author>
          <id>urn:uuid:60a76c80-d399-11d9-b93C-0003939e0af6</id>
          <entry>
            <title>Atom-Powered Robots Run Amok</title>
            <link href="http://example.org/2003/12/13/atom03"/>
            <id>urn:uuid:1225c695-cfb8-4ebb-aaaa-80da344efa6a</id>
            <updated>2003-12-13T18:30:02Z</updated>
            <summary>Some text.</summary>
          </entry>
        </feed>"#;

        let feed = parse_feed(atom_data.as_bytes()).expect("Failed to parse valid ATOM");
        if let Feed::Atom(feed) = feed {
            assert_eq!(feed.title.as_str(), "Example Feed");
            assert_eq!(feed.entries.len(), 1);
            assert_eq!(
                feed.entries[0].title.as_str(),
                "Atom-Powered Robots Run Amok"
            );
        } else {
            panic!("Expected Feed::Atom");
        }
    }

    #[test]
    fn test_parse_feed_invalid() {
        let invalid_data = "This is not an XML document";
        let err = parse_feed(invalid_data.as_bytes()).expect_err("Expected parse error");
        assert_eq!(
            err.to_string(),
            "the input could not be parsed as RSS or Atom"
        );
    }
}
