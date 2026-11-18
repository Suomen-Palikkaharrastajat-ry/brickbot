use crate::db::feed::DistinctPendingAnnouncement;
use sqlx::SqlitePool;
use std::fmt::Write as _;

/// Formats the list of distinct pending announcements with 1-based index numbering.
#[must_use]
pub fn format_pending_list(items: &[DistinctPendingAnnouncement]) -> String {
    if items.is_empty() {
        return "No feed items waiting to be announced.\n".to_string();
    }

    let mut output = String::new();
    let num_digits = items.len().to_string().len().max(2);

    for (idx, item) in items.iter().enumerate() {
        let num = idx + 1;
        let prefix = format!("{num:>num_digits$}. ");
        let indent = " ".repeat(prefix.len());

        let _ = writeln!(output, "{prefix}{}", item.item_title);
        let _ = writeln!(output, "{indent}Source: {}", item.source_title);
        let _ = writeln!(output, "{indent}Item ID: {}", item.item_id);

        if let Some(published_at) = item.published_at {
            let _ = writeln!(
                output,
                "{indent}Published: {}",
                published_at.format("%Y-%m-%d %H:%M:%S UTC")
            );
        }

        let channel_count = item.channel_ids.len();
        let channel_str = item
            .channel_ids
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let channel_label = if channel_count == 1 {
            "channel"
        } else {
            "channels"
        };
        let _ = writeln!(
            output,
            "{indent}Queued for: {channel_label} {channel_str} ({channel_count} {channel_label})"
        );

        if idx + 1 < items.len() {
            output.push('\n');
        }
    }

    output
}

/// Formats the details of a single distinct pending announcement.
#[must_use]
pub fn format_item_details(item: &DistinctPendingAnnouncement) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Title: {}", item.item_title);
    let _ = writeln!(output, "Source: {}", item.source_title);
    let _ = writeln!(output, "Item ID: {}", item.item_id);
    if let Some(published_at) = item.published_at {
        let _ = writeln!(
            output,
            "Published: {}",
            published_at.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    let channel_count = item.channel_ids.len();
    let channel_str = item
        .channel_ids
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let channel_label = if channel_count == 1 {
        "channel"
    } else {
        "channels"
    };
    let _ = writeln!(
        output,
        "Queued for: {channel_label} {channel_str} ({channel_count} {channel_label})"
    );
    output
}

/// Resolves a selector (either 1-based index or exact `item_id`) to an item in the slice.
#[must_use]
pub fn resolve_item<'a>(
    items: &'a [DistinctPendingAnnouncement],
    selector: &str,
) -> Option<&'a DistinctPendingAnnouncement> {
    let trimmed = selector.trim();
    if let Ok(idx) = trimmed.parse::<usize>() {
        if idx >= 1 && idx <= items.len() {
            return Some(&items[idx - 1]);
        }
    }
    items.iter().find(|item| item.item_id == trimmed)
}

/// Returns true if the user input indicates confirmation (e.g. 'y' or 'yes').
#[must_use]
pub fn is_confirmed(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
}

/// Executes the list command, fetching distinct pending announcements and writing the formatted output.
pub async fn run_list_pending<W: std::io::Write>(
    db: &SqlitePool,
    writer: &mut W,
) -> Result<(), anyhow::Error> {
    let items = crate::db::feed::get_distinct_pending_announcements(db).await?;
    let output = format_pending_list(&items);
    write!(writer, "{output}")?;
    writer.flush()?;
    Ok(())
}

/// Executes the remove command with confirmation.
pub async fn run_remove_pending<R: std::io::BufRead, W: std::io::Write>(
    db: &SqlitePool,
    target: &str,
    yes: bool,
    reader: &mut R,
    writer: &mut W,
) -> Result<bool, anyhow::Error> {
    let items = crate::db::feed::get_distinct_pending_announcements(db).await?;
    let Some(item) = resolve_item(&items, target) else {
        anyhow::bail!("Item '{target}' not found in pending announcements list.");
    };

    let details = format_item_details(item);
    write!(writer, "{details}")?;

    let confirmed = if yes {
        true
    } else {
        write!(
            writer,
            "Are you sure you want to remove this item from the announcement list? [y/N]: "
        )?;
        writer.flush()?;

        let mut line = String::new();
        reader.read_line(&mut line)?;
        is_confirmed(&line)
    };

    if confirmed {
        let count =
            crate::db::feed::delete_pending_announcement_by_item_id(db, &item.item_id).await?;
        let queue_label = if count == 1 { "queue" } else { "queues" };
        writeln!(
            writer,
            "Successfully removed item \"{}\" from {} channel {}.",
            item.item_title, count, queue_label
        )?;
        Ok(true)
    } else {
        writeln!(writer, "Aborted: item was not removed.")?;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::feed::{PendingAnnouncement, queue_pending_announcements};

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    #[test]
    fn test_format_pending_list_empty() {
        let output = format_pending_list(&[]);
        assert_eq!(output, "No feed items waiting to be announced.\n");
    }

    #[test]
    fn test_format_pending_list_with_items() {
        let published = chrono::DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
            .unwrap()
            .to_utc();
        let items = vec![
            DistinctPendingAnnouncement {
                item_id: "https://example.com/item1".to_string(),
                feed_url: "https://example.com/feed".to_string(),
                source_title: "Blog Alpha".to_string(),
                item_title: "First Post".to_string(),
                published_at: Some(published),
                channel_ids: vec![12345],
            },
            DistinctPendingAnnouncement {
                item_id: "https://example.com/item2".to_string(),
                feed_url: "https://example.com/feed".to_string(),
                source_title: "Blog Beta".to_string(),
                item_title: "Second Post".to_string(),
                published_at: None,
                channel_ids: vec![12345, 67890],
            },
        ];

        let output = format_pending_list(&items);
        assert!(output.contains(" 1. First Post\n"));
        assert!(output.contains("    Source: Blog Alpha\n"));
        assert!(output.contains("    Item ID: https://example.com/item1\n"));
        assert!(output.contains("    Published: 2026-10-17 12:00:00 UTC\n"));
        assert!(output.contains("    Queued for: channel 12345 (1 channel)\n"));

        assert!(output.contains(" 2. Second Post\n"));
        assert!(output.contains("    Source: Blog Beta\n"));
        assert!(output.contains("    Item ID: https://example.com/item2\n"));
        assert!(!output.contains("Second Post\n    Source: Blog Beta\n    Item ID: https://example.com/item2\n    Published:"));
        assert!(output.contains("    Queued for: channels 12345, 67890 (2 channels)\n"));
    }

    #[test]
    fn test_format_item_details() {
        let published = chrono::DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
            .unwrap()
            .to_utc();
        let item = DistinctPendingAnnouncement {
            item_id: "item_xyz".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Source Name".to_string(),
            item_title: "Article Title".to_string(),
            published_at: Some(published),
            channel_ids: vec![100, 200],
        };

        let details = format_item_details(&item);
        assert_eq!(
            details,
            "Title: Article Title\nSource: Source Name\nItem ID: item_xyz\nPublished: 2026-10-17 12:00:00 UTC\nQueued for: channels 100, 200 (2 channels)\n"
        );
    }

    #[test]
    fn test_resolve_item() {
        let items = vec![
            DistinctPendingAnnouncement {
                item_id: "https://example.com/first".to_string(),
                feed_url: "https://example.com/feed".to_string(),
                source_title: "Source 1".to_string(),
                item_title: "First".to_string(),
                published_at: None,
                channel_ids: vec![1],
            },
            DistinctPendingAnnouncement {
                item_id: "second_guid".to_string(),
                feed_url: "https://example.com/feed".to_string(),
                source_title: "Source 2".to_string(),
                item_title: "Second".to_string(),
                published_at: None,
                channel_ids: vec![2],
            },
        ];

        // 1-based index resolution
        assert_eq!(resolve_item(&items, "1").unwrap().item_title, "First");
        assert_eq!(resolve_item(&items, "2").unwrap().item_title, "Second");
        assert!(resolve_item(&items, "0").is_none());
        assert!(resolve_item(&items, "3").is_none());

        // Exact item_id resolution
        assert_eq!(
            resolve_item(&items, "https://example.com/first")
                .unwrap()
                .item_title,
            "First"
        );
        assert_eq!(
            resolve_item(&items, "second_guid").unwrap().item_title,
            "Second"
        );
        assert_eq!(
            resolve_item(&items, "  second_guid  ").unwrap().item_title,
            "Second"
        );
        assert!(resolve_item(&items, "unknown").is_none());
    }

    #[test]
    fn test_is_confirmed() {
        assert!(is_confirmed("y"));
        assert!(is_confirmed("Y"));
        assert!(is_confirmed("yes"));
        assert!(is_confirmed("Yes"));
        assert!(is_confirmed("YES"));
        assert!(is_confirmed("  y  \n"));

        assert!(!is_confirmed("n"));
        assert!(!is_confirmed("N"));
        assert!(!is_confirmed("no"));
        assert!(!is_confirmed(""));
        assert!(!is_confirmed("\n"));
        assert!(!is_confirmed("anything else"));
    }

    #[tokio::test]
    async fn test_run_list_pending_cli() {
        let pool = setup_db().await;
        let mut output = Vec::new();

        // 1. Empty list
        run_list_pending(&pool, &mut output).await.unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "No feed items waiting to be announced.\n"
        );

        // 2. Add an item and list
        let item = PendingAnnouncement {
            channel_id: 10,
            guild_id: 1,
            item_id: "post1".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Blog".to_string(),
            item_title: "New Post".to_string(),
            published_at: None,
        };
        queue_pending_announcements(&pool, &[item]).await.unwrap();

        let mut output2 = Vec::new();
        run_list_pending(&pool, &mut output2).await.unwrap();
        let text = String::from_utf8(output2).unwrap();
        assert!(text.contains(" 1. New Post"));
        assert!(text.contains("Source: Blog"));
        assert!(text.contains("Item ID: post1"));
        assert!(text.contains("Queued for: channel 10 (1 channel)"));
    }

    #[tokio::test]
    async fn test_run_remove_pending_interactive_confirmed() {
        let pool = setup_db().await;
        let item1 = PendingAnnouncement {
            channel_id: 10,
            guild_id: 1,
            item_id: "post1".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Blog".to_string(),
            item_title: "New Post 1".to_string(),
            published_at: None,
        };
        let item1_ch2 = PendingAnnouncement {
            channel_id: 20,
            guild_id: 2,
            item_id: "post1".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Blog".to_string(),
            item_title: "New Post 1".to_string(),
            published_at: None,
        };
        queue_pending_announcements(&pool, &[item1, item1_ch2])
            .await
            .unwrap();

        let mut input = std::io::Cursor::new(b"y\n");
        let mut output = Vec::new();

        let res = run_remove_pending(&pool, "1", false, &mut input, &mut output)
            .await
            .unwrap();
        assert!(res);

        let out_str = String::from_utf8(output).unwrap();
        assert!(out_str.contains("Title: New Post 1"));
        assert!(out_str.contains(
            "Are you sure you want to remove this item from the announcement list? [y/N]: "
        ));
        assert!(
            out_str.contains("Successfully removed item \"New Post 1\" from 2 channel queues.")
        );

        // Verify database is now empty
        let distinct = crate::db::feed::get_distinct_pending_announcements(&pool)
            .await
            .unwrap();
        assert!(distinct.is_empty());
    }

    #[tokio::test]
    async fn test_run_remove_pending_interactive_aborted() {
        let pool = setup_db().await;
        let item = PendingAnnouncement {
            channel_id: 10,
            guild_id: 1,
            item_id: "post1".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Blog".to_string(),
            item_title: "New Post 1".to_string(),
            published_at: None,
        };
        queue_pending_announcements(&pool, &[item]).await.unwrap();

        let mut input = std::io::Cursor::new(b"n\n");
        let mut output = Vec::new();

        let res = run_remove_pending(&pool, "1", false, &mut input, &mut output)
            .await
            .unwrap();
        assert!(!res);

        let out_str = String::from_utf8(output).unwrap();
        assert!(out_str.contains("Aborted: item was not removed."));

        // Item should still be in the database
        let distinct = crate::db::feed::get_distinct_pending_announcements(&pool)
            .await
            .unwrap();
        assert_eq!(distinct.len(), 1);
    }

    #[tokio::test]
    async fn test_run_remove_pending_with_yes_flag() {
        let pool = setup_db().await;
        let item = PendingAnnouncement {
            channel_id: 10,
            guild_id: 1,
            item_id: "post1".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            source_title: "Blog".to_string(),
            item_title: "New Post 1".to_string(),
            published_at: None,
        };
        queue_pending_announcements(&pool, &[item]).await.unwrap();

        let mut input = std::io::Cursor::new(b"");
        let mut output = Vec::new();

        let res = run_remove_pending(&pool, "post1", true, &mut input, &mut output)
            .await
            .unwrap();
        assert!(res);

        let out_str = String::from_utf8(output).unwrap();
        assert!(out_str.contains("Successfully removed item \"New Post 1\" from 1 channel queue."));
        // Does not prompt for confirmation when yes = true
        assert!(!out_str.contains("Are you sure you want to remove"));

        let distinct = crate::db::feed::get_distinct_pending_announcements(&pool)
            .await
            .unwrap();
        assert!(distinct.is_empty());
    }

    #[tokio::test]
    async fn test_run_remove_pending_not_found() {
        let pool = setup_db().await;
        let mut input = std::io::Cursor::new(b"y\n");
        let mut output = Vec::new();

        let err = run_remove_pending(&pool, "999", false, &mut input, &mut output)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }
}
