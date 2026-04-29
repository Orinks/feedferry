use anyhow::Result;
use clap::ValueEnum;

use crate::models::{timestamp_to_rfc3339, Feed, ItemWithFeed, LibraryStats, QueueEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn render_feeds(feeds: &[Feed], format: OutputFormat, linear: bool) -> Result<String> {
    if format == OutputFormat::Json {
        return Ok(serde_json::to_string_pretty(feeds)?);
    }

    let mut out = String::new();
    if feeds.is_empty() {
        out.push_str("No feeds subscribed.\n");
        return Ok(out);
    }

    out.push_str(&format!("{} feeds.\n", feeds.len()));
    for (index, feed) in feeds.iter().enumerate() {
        if linear {
            out.push_str(&format!("\nFeed {} of {}.\n", index + 1, feeds.len()));
            out.push_str(&format!("ID: {}.\n", feed.id));
            out.push_str(&format!("Title: {}.\n", feed.title));
            out.push_str(&format!("Kind: {}.\n", feed.kind));
            out.push_str(&format!("URL: {}.\n", feed.url));
            if let Some(folder) = &feed.folder {
                out.push_str(&format!("Folder: {folder}.\n"));
            }
            if let Some(checked) = timestamp_to_rfc3339(feed.last_checked) {
                out.push_str(&format!("Last checked: {checked}.\n"));
            }
            if let Some(error) = &feed.last_error {
                out.push_str(&format!("Last error: {error}.\n"));
            }
        } else {
            let status = feed.last_error.as_ref().map(|_| "error").unwrap_or("ok");
            out.push_str(&format!(
                "{}: {} [{}] {} ({})\n",
                feed.id, feed.title, feed.kind, feed.url, status
            ));
        }
    }
    Ok(out)
}

pub fn render_items(items: &[ItemWithFeed], format: OutputFormat, linear: bool) -> Result<String> {
    if format == OutputFormat::Json {
        return Ok(serde_json::to_string_pretty(items)?);
    }

    let mut out = String::new();
    if items.is_empty() {
        out.push_str("No items matched.\n");
        return Ok(out);
    }

    out.push_str(&format!("{} items.\n", items.len()));
    for (index, wrapped) in items.iter().enumerate() {
        let item = &wrapped.item;
        let status = if item.is_read { "read" } else { "unread" };
        let favorite = if item.is_favorite {
            "favorite"
        } else {
            "not favorite"
        };
        let media = media_label(wrapped);
        let date = timestamp_to_rfc3339(item.primary_timestamp())
            .unwrap_or_else(|| "unknown date".to_string());

        if linear {
            out.push_str(&format!("\nItem {} of {}.\n", index + 1, items.len()));
            out.push_str(&format!("ID: {}.\n", item.id));
            out.push_str(&format!("Title: {}.\n", item.title));
            out.push_str(&format!("Feed: {}.\n", wrapped.feed_title));
            out.push_str(&format!("Status: {status}.\n"));
            out.push_str(&format!("Favorite: {favorite}.\n"));
            out.push_str(&format!("Date: {date}.\n"));
            out.push_str(&format!("Media: {media}.\n"));
            if item.last_position_secs > 0 {
                out.push_str(&format!(
                    "Saved position: {}.\n",
                    format_duration(item.last_position_secs)
                ));
            }
            if let Some(url) = &item.url {
                out.push_str(&format!("URL: {url}.\n"));
            }
        } else {
            let podcast_marker = if item.is_podcast_episode() {
                " podcast"
            } else {
                ""
            };
            let star = if item.is_favorite { " *" } else { "" };
            out.push_str(&format!(
                "{}: [{}{}{}] {} — {} — {}\n",
                item.id,
                status,
                podcast_marker,
                star,
                truncate(&item.title, 90),
                wrapped.feed_title,
                date
            ));
        }
    }
    Ok(out)
}

pub fn render_item_detail(wrapped: &ItemWithFeed, format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        return Ok(serde_json::to_string_pretty(wrapped)?);
    }

    let item = &wrapped.item;
    let mut out = String::new();
    out.push_str(&format!("Title: {}\n", item.title));
    out.push_str(&format!("ID: {}\n", item.id));
    out.push_str(&format!("Feed: {}\n", wrapped.feed_title));
    out.push_str(&format!(
        "Status: {}\n",
        if item.is_read { "read" } else { "unread" }
    ));
    out.push_str(&format!(
        "Favorite: {}\n",
        if item.is_favorite { "yes" } else { "no" }
    ));
    if let Some(author) = &item.author {
        out.push_str(&format!("Author: {author}\n"));
    }
    if let Some(date) = timestamp_to_rfc3339(item.published) {
        out.push_str(&format!("Published: {date}\n"));
    }
    if let Some(url) = &item.url {
        out.push_str(&format!("URL: {url}\n"));
    }
    out.push_str(&format!("Media: {}\n", media_label(wrapped)));
    if let Some(downloaded_path) = &item.downloaded_path {
        out.push_str(&format!("Downloaded: {downloaded_path}\n"));
    }
    if item.last_position_secs > 0 {
        out.push_str(&format!(
            "Saved position: {}\n",
            format_duration(item.last_position_secs)
        ));
    }
    if let Some(summary) = &item.summary {
        out.push_str("\nSummary:\n");
        out.push_str(summary.trim());
        out.push('\n');
    }
    if let Some(content) = &item.content {
        out.push_str("\nContent:\n");
        out.push_str(content.trim());
        out.push('\n');
    }
    Ok(out)
}

pub fn render_queue(entries: &[QueueEntry], format: OutputFormat, linear: bool) -> Result<String> {
    if format == OutputFormat::Json {
        return Ok(serde_json::to_string_pretty(entries)?);
    }
    let items: Vec<ItemWithFeed> = entries.iter().map(|entry| entry.item.clone()).collect();
    if items.is_empty() {
        return Ok("Queue is empty.\n".to_string());
    }
    let mut out = format!("{} queued episodes.\n", entries.len());
    out.push_str(&render_items(&items, OutputFormat::Text, linear)?);
    Ok(out)
}

pub fn render_stats(stats: &LibraryStats, format: OutputFormat) -> Result<String> {
    if format == OutputFormat::Json {
        return Ok(serde_json::to_string_pretty(stats)?);
    }
    Ok(format!(
        "Feeds: {}\nItems: {}\nUnread items: {}\nPodcast episodes: {}\nQueued episodes: {}\nDownloaded episodes: {}\nFavorite items: {}\n",
        stats.feeds,
        stats.items,
        stats.unread_items,
        stats.podcast_episodes,
        stats.queued_episodes,
        stats.downloaded_episodes,
        stats.favorite_items,
    ))
}

fn media_label(wrapped: &ItemWithFeed) -> String {
    let item = &wrapped.item;
    let Some(media_url) = &item.media_url else {
        return "none".to_string();
    };
    let mut parts = Vec::new();
    parts.push(
        item.media_type
            .clone()
            .unwrap_or_else(|| "media".to_string()),
    );
    if let Some(length) = item.media_length {
        parts.push(format!("{} bytes", length));
    }
    if let Some(duration) = item.duration_secs {
        parts.push(format_duration(duration));
    }
    if item.downloaded_path.is_some() {
        parts.push("downloaded".to_string());
    }
    parts.push(media_url.clone());
    parts.join(", ")
}

pub fn format_duration(seconds: i64) -> String {
    if seconds <= 0 {
        return "0 seconds".to_string();
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hours > 0 {
        format!("{hours} hours, {minutes} minutes, {secs} seconds")
    } else if minutes > 0 {
        format!("{minutes} minutes, {secs} seconds")
    } else {
        format!("{secs} seconds")
    }
}

fn truncate(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FeedKind, Item};

    fn wrapped_item() -> ItemWithFeed {
        ItemWithFeed {
            item: Item {
                id: 7,
                feed_id: 1,
                guid: "guid".to_string(),
                url: Some("https://example.com/item".to_string()),
                title: "Accessible Rust".to_string(),
                author: None,
                summary: Some("Summary".to_string()),
                content: None,
                published: Some(1_735_689_600),
                updated: None,
                media_url: Some("https://example.com/audio.mp3".to_string()),
                media_type: Some("audio/mpeg".to_string()),
                media_length: Some(42),
                duration_secs: Some(65),
                is_read: false,
                is_favorite: true,
                downloaded_path: None,
                added_at: 1_735_689_600,
                last_position_secs: 0,
            },
            feed_title: "Example".to_string(),
            feed_url: "https://example.com/feed.xml".to_string(),
            feed_kind: FeedKind::Podcast,
        }
    }

    #[test]
    fn linear_item_output_contains_stable_labels() {
        let rendered = render_items(&[wrapped_item()], OutputFormat::Text, true).unwrap();
        assert!(rendered.contains("ID: 7."));
        assert!(rendered.contains("Status: unread."));
        assert!(rendered.contains("Media: audio/mpeg"));
    }

    #[test]
    fn duration_formats_readably() {
        assert_eq!(format_duration(65), "1 minutes, 5 seconds");
        assert_eq!(format_duration(3661), "1 hours, 1 minutes, 1 seconds");
    }
}
