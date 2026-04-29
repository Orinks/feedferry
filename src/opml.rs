use anyhow::{Context, Result};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};

use crate::models::{Feed, FeedKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpmlSubscription {
    pub title: String,
    pub xml_url: String,
    pub html_url: Option<String>,
    pub kind: FeedKind,
    pub folder: Option<String>,
}

pub fn parse_opml(input: &str) -> Result<Vec<OpmlSubscription>> {
    let doc = Document::parse(input).context("invalid OPML/XML document")?;
    let mut subscriptions = Vec::new();
    let root = doc.root_element();
    let mut folders = Vec::new();
    walk_outlines(root, &mut folders, &mut subscriptions);
    Ok(subscriptions)
}

fn walk_outlines(node: Node<'_, '_>, folders: &mut Vec<String>, out: &mut Vec<OpmlSubscription>) {
    for child in node.children().filter(|node| node.is_element()) {
        if child.tag_name().name() != "outline" {
            walk_outlines(child, folders, out);
            continue;
        }

        let title = child
            .attribute("title")
            .or_else(|| child.attribute("text"))
            .unwrap_or("Untitled feed")
            .trim()
            .to_string();

        if let Some(xml_url) = child
            .attribute("xmlUrl")
            .or_else(|| child.attribute("xmlurl"))
        {
            let type_attr = child.attribute("type").unwrap_or_default();
            let kind = if type_attr.eq_ignore_ascii_case("podcast") {
                FeedKind::Podcast
            } else {
                FeedKind::Mixed
            };
            out.push(OpmlSubscription {
                title,
                xml_url: xml_url.to_string(),
                html_url: child.attribute("htmlUrl").map(ToOwned::to_owned),
                kind,
                folder: if folders.is_empty() {
                    child.attribute("category").map(ToOwned::to_owned)
                } else {
                    Some(folders.join("/"))
                },
            });
        } else {
            folders.push(title);
            walk_outlines(child, folders, out);
            folders.pop();
        }
    }
}

pub fn export_opml(feeds: &[Feed], title: &str) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<opml version=\"2.0\">\n");
    out.push_str("  <head>\n");
    out.push_str("    <title>");
    out.push_str(&escape_xml(title));
    out.push_str("</title>\n");
    out.push_str("  </head>\n");
    out.push_str("  <body>\n");

    for feed in feeds {
        out.push_str("    <outline text=\"");
        out.push_str(&escape_xml(&feed.title));
        out.push_str("\" title=\"");
        out.push_str(&escape_xml(&feed.title));
        out.push_str("\" type=\"");
        out.push_str(match feed.kind {
            FeedKind::Podcast => "podcast",
            FeedKind::News | FeedKind::Mixed => "rss",
        });
        out.push_str("\" xmlUrl=\"");
        out.push_str(&escape_xml(&feed.url));
        out.push('"');
        if let Some(site_url) = &feed.site_url {
            out.push_str(" htmlUrl=\"");
            out.push_str(&escape_xml(site_url));
            out.push('"');
        }
        if let Some(folder) = &feed.folder {
            out.push_str(" category=\"");
            out.push_str(&escape_xml(folder));
            out.push('"');
        }
        out.push_str(" />\n");
    }

    out.push_str("  </body>\n");
    out.push_str("</opml>\n");
    out
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::now_ts;

    const OPML: &str = r#"<?xml version="1.0"?>
<opml version="2.0">
  <body>
    <outline text="Tech">
      <outline text="Rust Blog" title="Rust Blog" type="rss" xmlUrl="https://blog.rust-lang.org/feed.xml" htmlUrl="https://blog.rust-lang.org/" />
    </outline>
    <outline text="Podcast" type="podcast" xmlUrl="https://example.com/podcast.xml" />
  </body>
</opml>"#;

    #[test]
    fn parses_nested_opml() {
        let subscriptions = parse_opml(OPML).unwrap();
        assert_eq!(subscriptions.len(), 2);
        assert_eq!(subscriptions[0].folder.as_deref(), Some("Tech"));
        assert_eq!(subscriptions[1].kind, FeedKind::Podcast);
    }

    #[test]
    fn exports_valid_opml() {
        let feed = Feed {
            id: 1,
            url: "https://example.com/feed.xml".to_string(),
            title: "Example & Feed".to_string(),
            kind: FeedKind::News,
            folder: Some("News".to_string()),
            site_url: Some("https://example.com".to_string()),
            description: None,
            etag: None,
            last_modified: None,
            last_checked: None,
            last_error: None,
            created_at: now_ts(),
            updated_at: now_ts(),
        };
        let xml = export_opml(&[feed], "Subscriptions");
        assert!(xml.contains("Example &amp; Feed"));
        assert!(parse_opml(&xml).unwrap().len() == 1);
    }
}
