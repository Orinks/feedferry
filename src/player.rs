use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::models::ItemWithFeed;

pub fn open_item_url(item: &ItemWithFeed) -> Result<()> {
    let url = item
        .item
        .url
        .as_ref()
        .or(item.item.media_url.as_ref())
        .context("item has no URL to open")?;
    open_target(url)
}

pub fn play_item(item: &ItemWithFeed, player: Option<&str>) -> Result<()> {
    if let Some(player) = player.filter(|value| !value.trim().is_empty()) {
        return run_player(player, player_play_target(item)?);
    }

    open_target(default_play_target(item)?)
}

fn player_play_target(item: &ItemWithFeed) -> Result<&str> {
    item.item
        .downloaded_path
        .as_deref()
        .or(item.item.media_url.as_deref())
        .context("item has no downloaded file or media URL to play")
}

fn default_play_target(item: &ItemWithFeed) -> Result<&str> {
    item.item.downloaded_path.as_deref().context(
        "episode is not downloaded; set a player command in settings or download the episode first",
    )
}

fn run_player(command_line: &str, target: &str) -> Result<()> {
    let mut parts = command_line.split_whitespace();
    let Some(program) = parts.next() else {
        bail!("player command is empty");
    };
    let mut command = Command::new(program);
    for arg in parts {
        command.arg(arg);
    }
    command.arg(target);
    let status = command
        .status()
        .with_context(|| format!("failed to launch player `{command_line}`"))?;
    if !status.success() {
        bail!("player exited with status {status}");
    }
    Ok(())
}

fn open_target(target: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn()
            .context("failed to open target with Windows shell")?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(target)
            .spawn()
            .context("failed to open target with macOS open")?;
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .context("failed to open target with xdg-open")?;
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", unix)))]
    {
        bail!("opening targets is not implemented for this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FeedKind, Item};

    fn podcast_item(downloaded_path: Option<&str>) -> ItemWithFeed {
        ItemWithFeed {
            item: Item {
                id: 7,
                feed_id: 1,
                guid: "episode-7".to_string(),
                url: Some("https://example.com/posts/7".to_string()),
                title: "Episode Seven".to_string(),
                author: None,
                summary: None,
                content: None,
                published: None,
                updated: None,
                media_url: Some("https://example.com/episode-7.mp3".to_string()),
                media_type: Some("audio/mpeg".to_string()),
                media_length: None,
                duration_secs: None,
                is_read: false,
                is_favorite: false,
                downloaded_path: downloaded_path.map(str::to_string),
                added_at: 0,
                last_position_secs: 0,
            },
            feed_title: "Example Podcast".to_string(),
            feed_url: "https://example.com/feed.xml".to_string(),
            feed_kind: FeedKind::Podcast,
        }
    }

    #[test]
    fn default_playback_uses_downloaded_file_only() {
        let item = podcast_item(Some("C:\\Downloads\\episode-7.mp3"));

        assert_eq!(
            default_play_target(&item).unwrap(),
            "C:\\Downloads\\episode-7.mp3"
        );
    }

    #[test]
    fn default_playback_does_not_open_remote_media_url() {
        let item = podcast_item(None);

        let error = default_play_target(&item).unwrap_err().to_string();

        assert!(error.contains("episode is not downloaded"));
    }

    #[test]
    fn configured_player_can_stream_remote_media_url() {
        let item = podcast_item(None);

        assert_eq!(
            player_play_target(&item).unwrap(),
            "https://example.com/episode-7.mp3"
        );
    }
}
