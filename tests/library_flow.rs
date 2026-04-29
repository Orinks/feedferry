use feedferry::database::Store;
use feedferry::feed::parse_feed_bytes;
use feedferry::models::{FeedKind, ItemFilter};
use feedferry::opml::{export_opml, parse_opml};

const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Integration Podcast</title>
    <link>https://example.com</link>
    <description>Feed used by integration tests</description>
    <item>
      <title>Episode One</title>
      <guid>episode-1</guid>
      <link>https://example.com/episode-1</link>
      <description>Episode summary</description>
      <enclosure url="https://example.com/episode-1.mp3" length="2048" type="audio/mpeg" />
    </item>
  </channel>
</rss>"#;

#[test]
fn full_library_flow_from_feed_parse_to_queue() {
    let parsed = parse_feed_bytes(RSS.as_bytes()).unwrap();
    let store = Store::open_in_memory().unwrap();
    let feed_id = store
        .add_feed(
            "https://example.com/feed.xml",
            Some(&parsed.title),
            FeedKind::Podcast,
            Some("Podcasts"),
        )
        .unwrap();
    store
        .update_feed_success(
            feed_id,
            &parsed.title,
            parsed.site_url.as_deref(),
            parsed.description.as_deref(),
            None,
            None,
        )
        .unwrap();
    store.upsert_items(feed_id, &parsed.items).unwrap();

    let podcasts = store
        .list_items(&ItemFilter {
            podcasts_only: true,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(podcasts.len(), 1);
    assert_eq!(podcasts[0].item.media_type.as_deref(), Some("audio/mpeg"));

    let item_id = podcasts[0].item.id;
    store.enqueue_item(item_id).unwrap();
    store.mark_item_read(item_id, true).unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.feeds, 1);
    assert_eq!(stats.items, 1);
    assert_eq!(stats.unread_items, 0);
    assert_eq!(stats.queued_episodes, 1);
}

#[test]
fn opml_round_trip_exports_subscriptions() {
    let store = Store::open_in_memory().unwrap();
    store
        .add_feed(
            "https://example.com/feed.xml",
            Some("Example Feed"),
            FeedKind::News,
            Some("News"),
        )
        .unwrap();
    let feeds = store.list_feeds().unwrap();
    let xml = export_opml(&feeds, "Test subscriptions");
    let parsed = parse_opml(&xml).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].title, "Example Feed");
    assert_eq!(parsed[0].folder.as_deref(), Some("News"));
}
