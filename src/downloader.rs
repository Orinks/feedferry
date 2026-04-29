use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use url::Url;

use crate::models::ItemWithFeed;

#[derive(Debug, Clone)]
pub struct Downloader {
    client: Client,
}

impl Downloader {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .user_agent("FeedFerry/0.1 downloader")
            .build()
            .context("could not build downloader HTTP client")?;
        Ok(Self { client })
    }

    pub fn download_item(
        &self,
        item: &ItemWithFeed,
        directory: &Path,
        force: bool,
    ) -> Result<PathBuf> {
        fs::create_dir_all(directory).with_context(|| {
            format!(
                "could not create download directory {}",
                directory.display()
            )
        })?;

        let media_url = item
            .item
            .media_url
            .as_ref()
            .context("item has no podcast/media enclosure")?;
        let file_name = file_name_for_item(item, media_url);
        let destination = directory.join(file_name);

        if destination.exists() && !force {
            return Ok(destination);
        }

        let mut response = self
            .client
            .get(media_url)
            .send()
            .with_context(|| format!("failed to download {media_url}"))?;
        if !response.status().is_success() {
            bail!("download returned HTTP status {}", response.status());
        }

        let tmp = destination.with_extension("part");
        let mut file = File::create(&tmp)
            .with_context(|| format!("could not create temporary file {}", tmp.display()))?;
        io::copy(&mut response, &mut file).context("failed while writing downloaded media")?;
        fs::rename(&tmp, &destination).with_context(|| {
            format!(
                "could not move temporary file {} to {}",
                tmp.display(),
                destination.display()
            )
        })?;
        Ok(destination)
    }
}

fn file_name_for_item(item: &ItemWithFeed, media_url: &str) -> String {
    let stem = sanitize_file_stem(&format!("{}-{}", item.item.id, item.item.title));
    let extension = extension_from_url(media_url)
        .or_else(|| extension_from_media_type(item.item.media_type.as_deref()))
        .unwrap_or("bin");
    format!("{stem}.{extension}")
}

fn extension_from_url(media_url: &str) -> Option<&str> {
    let url = Url::parse(media_url).ok()?;
    let segment = url.path_segments()?.next_back()?;
    let (_, ext) = segment.rsplit_once('.')?;
    let ext = ext.trim();
    if ext.is_empty() || ext.len() > 8 || !ext.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return None;
    }
    Some(Box::leak(ext.to_ascii_lowercase().into_boxed_str()))
}

fn extension_from_media_type(media_type: Option<&str>) -> Option<&'static str> {
    match media_type?.to_ascii_lowercase().as_str() {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/mp4" | "audio/x-m4a" => Some("m4a"),
        "audio/aac" => Some("aac"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/flac" => Some("flac"),
        "audio/wav" | "audio/x-wav" => Some("wav"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        _ => None,
    }
}

fn sanitize_file_stem(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' ') {
            out.push(ch);
        } else {
            out.push('_');
        }
        if out.len() >= 100 {
            break;
        }
    }
    let out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        "episode".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FeedKind, Item};

    #[test]
    fn builds_safe_file_names() {
        let item = ItemWithFeed {
            item: Item {
                id: 3,
                feed_id: 1,
                guid: "g".to_string(),
                url: None,
                title: "A/B:C*D?".to_string(),
                author: None,
                summary: None,
                content: None,
                published: None,
                updated: None,
                media_url: Some("https://example.com/audio.MP3?x=1".to_string()),
                media_type: Some("audio/mpeg".to_string()),
                media_length: None,
                duration_secs: None,
                is_read: false,
                is_favorite: false,
                downloaded_path: None,
                added_at: 0,
                last_position_secs: 0,
            },
            feed_title: "Feed".to_string(),
            feed_url: "https://example.com/feed".to_string(),
            feed_kind: FeedKind::Podcast,
        };
        assert_eq!(
            file_name_for_item(&item, "https://example.com/audio.MP3?x=1"),
            "3-A_B_C_D_.mp3"
        );
    }
}
