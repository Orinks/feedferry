use std::time::Duration;

use anyhow::{bail, Context, Result};
use feed_rs::model::{Entry, Link};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, USER_AGENT};
use reqwest::StatusCode;

use crate::models::NewItem;

const USER_AGENT_VALUE: &str = "FeedFerry/0.2 (accessible Rust feed reader)";

#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub title: String,
    pub site_url: Option<String>,
    pub description: Option<String>,
    pub items: Vec<NewItem>,
}

#[derive(Debug, Clone)]
pub enum FetchOutcome {
    NotModified,
    Fetched {
        body: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct FeedFetcher {
    client: Client,
}

impl FeedFetcher {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .user_agent(USER_AGENT_VALUE)
            .build()
            .context("could not build HTTP client")?;
        Ok(Self { client })
    }

    pub fn fetch(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<FetchOutcome> {
        let mut request = self.client.get(url).header(USER_AGENT, USER_AGENT_VALUE);
        if let Some(value) = etag.filter(|v| !v.is_empty()) {
            request = request.header(IF_NONE_MATCH, value);
        }
        if let Some(value) = last_modified.filter(|v| !v.is_empty()) {
            request = request.header(IF_MODIFIED_SINCE, value);
        }

        let response = request
            .send()
            .with_context(|| format!("failed to fetch feed {url}"))?;

        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(FetchOutcome::NotModified);
        }

        if !response.status().is_success() {
            bail!("feed returned HTTP status {}", response.status());
        }

        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let last_modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);
        let body = response
            .bytes()
            .context("failed to read feed response body")?
            .to_vec();

        Ok(FetchOutcome::Fetched {
            body,
            etag,
            last_modified,
        })
    }
}

pub fn parse_feed_bytes(bytes: &[u8]) -> Result<ParsedFeed> {
    let feed = feed_rs::parser::parse(bytes).context("failed to parse feed document")?;
    let title = feed
        .title
        .as_ref()
        .map(|text| clean_text(&text.content))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Untitled feed".to_string());
    let site_url = best_link(&feed.links).map(ToOwned::to_owned);
    let description = feed
        .description
        .as_ref()
        .map(|text| clean_text(&text.content))
        .filter(|value| !value.is_empty());

    let mut items = Vec::with_capacity(feed.entries.len());
    for entry in &feed.entries {
        items.push(entry_to_item(entry));
    }

    Ok(ParsedFeed {
        title,
        site_url,
        description,
        items,
    })
}

fn entry_to_item(entry: &Entry) -> NewItem {
    let title = entry
        .title
        .as_ref()
        .map(|text| clean_text(&text.content))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Untitled item".to_string());
    let url = best_link(&entry.links).map(ToOwned::to_owned);
    let guid = if !entry.id.trim().is_empty() {
        entry.id.clone()
    } else {
        url.clone().unwrap_or_else(|| title.clone())
    };
    let author = entry
        .authors
        .first()
        .map(|person| person.name.clone())
        .filter(|value| !value.trim().is_empty());
    let summary = entry
        .summary
        .as_ref()
        .map(|text| clean_text(&text.content))
        .filter(|value| !value.is_empty());
    let content = entry
        .content
        .as_ref()
        .and_then(|content| content.body.as_ref())
        .map(|body| clean_text(body))
        .filter(|value| !value.is_empty());
    let published = entry.published.map(|dt| dt.timestamp());
    let updated = entry.updated.map(|dt| dt.timestamp());
    let media = find_media(entry);

    NewItem {
        guid,
        url,
        title,
        author,
        summary,
        content,
        published,
        updated,
        media_url: media.as_ref().map(|value| value.url.clone()),
        media_type: media.as_ref().and_then(|value| value.media_type.clone()),
        media_length: media.as_ref().and_then(|value| value.length),
        duration_secs: media.as_ref().and_then(|value| value.duration_secs),
    }
}

#[derive(Debug, Clone)]
struct MediaCandidate {
    url: String,
    media_type: Option<String>,
    length: Option<i64>,
    duration_secs: Option<i64>,
}

fn find_media(entry: &Entry) -> Option<MediaCandidate> {
    for link in &entry.links {
        if is_enclosure(link) || link.media_type.as_deref().is_some_and(is_media_type) {
            return Some(MediaCandidate {
                url: link.href.clone(),
                media_type: link.media_type.clone(),
                length: link.length.and_then(u64_to_i64),
                duration_secs: None,
            });
        }
    }

    if let Some(content) = &entry.content {
        if let Some(src) = &content.src {
            let media_type = Some(content.content_type.to_string());
            if media_type.as_deref().is_some_and(is_media_type)
                || src.rel.as_deref() == Some("enclosure")
            {
                return Some(MediaCandidate {
                    url: src.href.clone(),
                    media_type,
                    length: content.length.and_then(u64_to_i64),
                    duration_secs: None,
                });
            }
        }
    }

    for media in &entry.media {
        for content in &media.content {
            if let Some(url) = &content.url {
                let media_type = content.content_type.as_ref().map(ToString::to_string);
                if media_type.as_deref().is_none_or(is_media_type) {
                    let duration_secs = content
                        .duration
                        .or(media.duration)
                        .and_then(|duration| u64_to_i64(duration.as_secs()));
                    return Some(MediaCandidate {
                        url: url.to_string(),
                        media_type,
                        length: content.size.and_then(u64_to_i64),
                        duration_secs,
                    });
                }
            }
        }
    }

    None
}

fn u64_to_i64(value: u64) -> Option<i64> {
    i64::try_from(value).ok()
}

fn is_enclosure(link: &Link) -> bool {
    link.rel
        .as_deref()
        .map(|rel| rel.eq_ignore_ascii_case("enclosure"))
        .unwrap_or(false)
}

fn is_media_type(media_type: &str) -> bool {
    let lower = media_type.to_ascii_lowercase();
    lower.starts_with("audio/")
        || lower.starts_with("video/")
        || lower == "application/ogg"
        || lower == "application/x-mpegurl"
        || lower == "application/vnd.apple.mpegurl"
}

fn best_link(links: &[Link]) -> Option<&str> {
    links
        .iter()
        .find(|link| {
            link.rel
                .as_deref()
                .map(|rel| rel.eq_ignore_ascii_case("alternate"))
                .unwrap_or(true)
        })
        .or_else(|| links.first())
        .map(|link| link.href.as_str())
}

pub fn clean_text(input: &str) -> String {
    let without_scripts = Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>")
        .expect("valid regex")
        .replace_all(input, " ");
    let with_breaks = Regex::new(r"(?i)</?(p|div|br|li|tr|h[1-6]|blockquote)[^>]*>")
        .expect("valid regex")
        .replace_all(&without_scripts, "\n");
    let without_tags = Regex::new(r"(?is)<[^>]+>")
        .expect("valid regex")
        .replace_all(&with_breaks, " ");
    let decoded = html_escape::decode_html_entities(&without_tags);
    remove_spaces_before_punctuation(&normalize_whitespace(&decoded))
}

fn normalize_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_was_space = false;
    let mut last_was_newline = false;
    let mut newline_count = 0;

    for ch in input.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' {
            if !last_was_newline && !out.is_empty() {
                out.push('\n');
                newline_count = 1;
            } else if newline_count < 2 && !out.is_empty() {
                out.push('\n');
                newline_count += 1;
            }
            last_was_newline = true;
            last_was_space = false;
        } else if ch.is_whitespace() {
            if !last_was_space && !last_was_newline && !out.is_empty() {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
            last_was_newline = false;
            newline_count = 0;
        }
    }

    out.trim().to_string()
}

fn remove_spaces_before_punctuation(input: &str) -> String {
    Regex::new(r"\s+([.,;:!?])")
        .expect("valid regex")
        .replace_all(input, "$1")
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Example Podcast</title>
    <link>https://example.com</link>
    <description>A test podcast</description>
    <item>
      <title>Episode &amp; One</title>
      <guid>episode-1</guid>
      <link>https://example.com/1</link>
      <description><![CDATA[<p>Hello <strong>world</strong>.</p>]]></description>
      <pubDate>Wed, 01 Jan 2025 12:00:00 GMT</pubDate>
      <enclosure url="https://example.com/audio.mp3" length="1234" type="audio/mpeg" />
    </item>
  </channel>
</rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Feed</title>
  <link href="https://example.org/" />
  <updated>2025-01-01T00:00:00Z</updated>
  <id>atom-feed</id>
  <entry>
    <title>Atom Entry</title>
    <link href="https://example.org/entry" />
    <id>entry-1</id>
    <updated>2025-01-01T00:00:00Z</updated>
    <summary>Summary text</summary>
  </entry>
</feed>"#;

    #[test]
    fn parses_rss_podcast_enclosure() {
        let parsed = parse_feed_bytes(RSS.as_bytes()).unwrap();
        assert_eq!(parsed.title, "Example Podcast");
        assert_eq!(parsed.items.len(), 1);
        let item = &parsed.items[0];
        assert_eq!(item.title, "Episode & One");
        assert_eq!(
            item.media_url.as_deref(),
            Some("https://example.com/audio.mp3")
        );
        assert_eq!(item.media_type.as_deref(), Some("audio/mpeg"));
        assert_eq!(item.media_length, Some(1234));
        assert_eq!(item.summary.as_deref(), Some("Hello world."));
    }

    #[test]
    fn parses_atom_news_item() {
        let parsed = parse_feed_bytes(ATOM.as_bytes()).unwrap();
        assert_eq!(parsed.title, "Atom Feed");
        assert_eq!(parsed.items[0].title, "Atom Entry");
        assert!(parsed.items[0].media_url.is_none());
    }

    #[test]
    fn clean_text_strips_html_and_decodes_entities() {
        assert_eq!(
            clean_text("<p>Tom &amp; Jerry</p><script>x</script>"),
            "Tom & Jerry"
        );
    }
}
