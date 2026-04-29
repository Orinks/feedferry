use std::fmt::{Display, Formatter};
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedKind {
    News,
    Podcast,
    Mixed,
}

impl FeedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::News => "news",
            Self::Podcast => "podcast",
            Self::Mixed => "mixed",
        }
    }
}

impl Display for FeedKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FeedKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "news" | "rss" | "atom" => Ok(Self::News),
            "podcast" | "podcasts" | "audio" => Ok(Self::Podcast),
            "mixed" | "auto" | "both" => Ok(Self::Mixed),
            other => Err(format!("unknown feed kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feed {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub kind: FeedKind,
    pub folder: Option<String>,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_checked: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewItem {
    pub guid: String,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub published: Option<i64>,
    pub updated: Option<i64>,
    pub media_url: Option<String>,
    pub media_type: Option<String>,
    pub media_length: Option<i64>,
    pub duration_secs: Option<i64>,
}

impl NewItem {
    pub fn has_media(&self) -> bool {
        self.media_url.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    pub feed_id: i64,
    pub guid: String,
    pub url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub published: Option<i64>,
    pub updated: Option<i64>,
    pub media_url: Option<String>,
    pub media_type: Option<String>,
    pub media_length: Option<i64>,
    pub duration_secs: Option<i64>,
    pub is_read: bool,
    pub is_favorite: bool,
    pub downloaded_path: Option<String>,
    pub added_at: i64,
    pub last_position_secs: i64,
}

impl Item {
    pub fn is_podcast_episode(&self) -> bool {
        self.media_url.is_some()
    }

    pub fn primary_timestamp(&self) -> Option<i64> {
        self.published.or(self.updated).or(Some(self.added_at))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemWithFeed {
    pub item: Item,
    pub feed_title: String,
    pub feed_url: String,
    pub feed_kind: FeedKind,
}

#[derive(Debug, Clone, Default)]
pub struct ItemFilter {
    pub feed_id: Option<i64>,
    pub unread_only: bool,
    pub favorites_only: bool,
    pub podcasts_only: bool,
    pub news_only: bool,
    pub search: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub id: i64,
    pub position: i64,
    pub created_at: i64,
    pub item: ItemWithFeed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStats {
    pub feeds: i64,
    pub items: i64,
    pub unread_items: i64,
    pub podcast_episodes: i64,
    pub queued_episodes: i64,
    pub downloaded_episodes: i64,
    pub favorite_items: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedUpdateReport {
    pub feed_id: i64,
    pub title: String,
    pub fetched: bool,
    pub new_or_updated_items: usize,
    pub error: Option<String>,
}

pub fn now_ts() -> i64 {
    Utc::now().timestamp()
}

pub fn timestamp_to_rfc3339(timestamp: Option<i64>) -> Option<String> {
    timestamp
        .and_then(|value| DateTime::<Utc>::from_timestamp(value, 0))
        .map(|dt| dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_kind_from_str_accepts_aliases() {
        assert_eq!(FeedKind::from_str("rss").unwrap(), FeedKind::News);
        assert_eq!(FeedKind::from_str("podcasts").unwrap(), FeedKind::Podcast);
        assert_eq!(FeedKind::from_str("auto").unwrap(), FeedKind::Mixed);
    }
}
