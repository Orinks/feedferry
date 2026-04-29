use anyhow::{Context, Result};

use crate::database::Store;
use crate::feed::{parse_feed_bytes, FeedFetcher, FetchOutcome};
use crate::models::{Feed, FeedUpdateReport};

/// Receives human-readable progress messages during feed refreshes.
///
/// GUI code can connect this to a status bar or live announcement area;
/// command-line code can connect it to stderr/speech output.
pub trait UpdateObserver {
    fn status(&self, message: &str) -> Result<()>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoopObserver;

impl UpdateObserver for NoopObserver {
    fn status(&self, _message: &str) -> Result<()> {
        Ok(())
    }
}

impl UpdateObserver for crate::accessibility::Accessibility {
    fn status(&self, message: &str) -> Result<()> {
        crate::accessibility::Accessibility::status(self, message)
    }
}

pub fn update_selected_feeds(
    store: &Store,
    selectors: &[String],
    observer: &impl UpdateObserver,
) -> Result<Vec<FeedUpdateReport>> {
    let feeds = if selectors.is_empty() {
        store.list_feeds()?
    } else {
        selectors
            .iter()
            .map(|selector| store.require_feed(selector))
            .collect::<Result<Vec<_>>>()?
    };

    let fetcher = FeedFetcher::new()?;
    let mut reports = Vec::with_capacity(feeds.len());
    for feed in feeds {
        reports.push(update_one_feed(store, &fetcher, &feed, observer)?);
    }
    Ok(reports)
}

pub fn update_one_feed(
    store: &Store,
    fetcher: &FeedFetcher,
    feed: &Feed,
    observer: &impl UpdateObserver,
) -> Result<FeedUpdateReport> {
    observer.status(&format!("Updating {}.", feed.title))?;
    match update_one_feed_inner(store, fetcher, feed) {
        Ok(report) => Ok(report),
        Err(error) => {
            let message = error.to_string();
            store.set_feed_error(feed.id, &message)?;
            Ok(FeedUpdateReport {
                feed_id: feed.id,
                title: feed.title.clone(),
                fetched: false,
                new_or_updated_items: 0,
                error: Some(message),
            })
        }
    }
}

fn update_one_feed_inner(
    store: &Store,
    fetcher: &FeedFetcher,
    feed: &Feed,
) -> Result<FeedUpdateReport> {
    match fetcher
        .fetch(
            &feed.url,
            feed.etag.as_deref(),
            feed.last_modified.as_deref(),
        )
        .with_context(|| format!("could not update feed `{}`", feed.title))?
    {
        FetchOutcome::NotModified => {
            store.touch_feed_not_modified(feed.id)?;
            Ok(FeedUpdateReport {
                feed_id: feed.id,
                title: feed.title.clone(),
                fetched: false,
                new_or_updated_items: 0,
                error: None,
            })
        }
        FetchOutcome::Fetched {
            body,
            etag,
            last_modified,
        } => {
            let parsed = parse_feed_bytes(&body)?;
            let changed = store.upsert_items(feed.id, &parsed.items)?;
            store.update_feed_success(
                feed.id,
                &parsed.title,
                parsed.site_url.as_deref(),
                parsed.description.as_deref(),
                etag.as_deref(),
                last_modified.as_deref(),
            )?;
            Ok(FeedUpdateReport {
                feed_id: feed.id,
                title: parsed.title,
                fetched: true,
                new_or_updated_items: changed,
                error: None,
            })
        }
    }
}

pub fn format_update_reports(reports: &[FeedUpdateReport]) -> String {
    if reports.is_empty() {
        return "No feeds to update.".to_string();
    }

    let mut lines = Vec::with_capacity(reports.len());
    for report in reports {
        if let Some(error) = &report.error {
            lines.push(format!("{}: error: {}", report.title, error));
        } else if report.fetched {
            lines.push(format!(
                "{}: fetched; {} item(s) inserted or updated.",
                report.title, report.new_or_updated_items
            ));
        } else {
            lines.push(format!("{}: not modified.", report.title));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_update_reports_for_accessible_status() {
        let text = format_update_reports(&[
            FeedUpdateReport {
                feed_id: 1,
                title: "News".to_string(),
                fetched: true,
                new_or_updated_items: 2,
                error: None,
            },
            FeedUpdateReport {
                feed_id: 2,
                title: "Podcast".to_string(),
                fetched: false,
                new_or_updated_items: 0,
                error: Some("offline".to_string()),
            },
        ]);
        assert!(text.contains("News: fetched; 2 item"));
        assert!(text.contains("Podcast: error: offline"));
    }
}
