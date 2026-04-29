use std::fs;
use std::path::Path;
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, MappedRows, OptionalExtension, Row};

use crate::models::{
    now_ts, Feed, FeedKind, Item, ItemFilter, ItemWithFeed, LibraryStats, NewItem, QueueEntry,
};

const ITEM_WITH_FEED_SELECT: &str = "
    i.id, i.feed_id, i.guid, i.url, i.title, i.author, i.summary, i.content,
    i.published, i.updated, i.media_url, i.media_type, i.media_length,
    i.duration_secs, i.is_read, i.is_favorite, i.downloaded_path, i.added_at,
    i.last_position_secs, f.title, f.url, f.kind
";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("could not create database directory {}", parent.display())
            })?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("could not open database {}", path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS feeds (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                url             TEXT NOT NULL UNIQUE,
                title           TEXT NOT NULL,
                kind            TEXT NOT NULL DEFAULT 'mixed',
                folder          TEXT,
                site_url        TEXT,
                description     TEXT,
                etag            TEXT,
                last_modified   TEXT,
                last_checked    INTEGER,
                last_error      TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS items (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                feed_id             INTEGER NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
                guid                TEXT NOT NULL,
                url                 TEXT,
                title               TEXT NOT NULL,
                author              TEXT,
                summary             TEXT,
                content             TEXT,
                published           INTEGER,
                updated             INTEGER,
                media_url           TEXT,
                media_type          TEXT,
                media_length        INTEGER,
                duration_secs       INTEGER,
                is_read             INTEGER NOT NULL DEFAULT 0,
                is_favorite         INTEGER NOT NULL DEFAULT 0,
                downloaded_path     TEXT,
                added_at            INTEGER NOT NULL,
                last_position_secs  INTEGER NOT NULL DEFAULT 0,
                UNIQUE(feed_id, guid)
            );

            CREATE TABLE IF NOT EXISTS queue (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                item_id     INTEGER NOT NULL UNIQUE REFERENCES items(id) ON DELETE CASCADE,
                position    INTEGER NOT NULL,
                created_at  INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_items_feed_id ON items(feed_id);
            CREATE INDEX IF NOT EXISTS idx_items_read ON items(is_read);
            CREATE INDEX IF NOT EXISTS idx_items_favorite ON items(is_favorite);
            CREATE INDEX IF NOT EXISTS idx_items_media ON items(media_url);
            CREATE INDEX IF NOT EXISTS idx_items_published ON items(published);
            CREATE INDEX IF NOT EXISTS idx_queue_position ON queue(position);
            "#,
        )?;
        Ok(())
    }

    pub fn add_feed(
        &self,
        url: &str,
        title: Option<&str>,
        kind: FeedKind,
        folder: Option<&str>,
    ) -> Result<i64> {
        let now = now_ts();
        let initial_title = title
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(url);
        self.conn.execute(
            "INSERT OR IGNORE INTO feeds (url, title, kind, folder, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![url, initial_title, kind.as_str(), folder, now],
        )?;
        self.conn.execute(
            "UPDATE feeds
             SET title = CASE WHEN ?1 IS NULL THEN title ELSE ?1 END,
                 kind = ?2,
                 folder = CASE WHEN ?3 IS NULL THEN folder ELSE ?3 END,
                 updated_at = ?4
             WHERE url = ?5",
            params![title, kind.as_str(), folder, now, url],
        )?;
        let id =
            self.conn
                .query_row("SELECT id FROM feeds WHERE url = ?1", params![url], |row| {
                    row.get(0)
                })?;
        Ok(id)
    }

    pub fn remove_feed(&self, selector: &str) -> Result<usize> {
        let Some(feed) = self.find_feed(selector)? else {
            return Ok(0);
        };
        let rows = self
            .conn
            .execute("DELETE FROM feeds WHERE id = ?1", params![feed.id])?;
        Ok(rows)
    }

    pub fn find_feed(&self, selector: &str) -> Result<Option<Feed>> {
        if let Ok(id) = selector.parse::<i64>() {
            if let Some(feed) = self.feed_by_id(id)? {
                return Ok(Some(feed));
            }
        }

        let by_url = self
            .conn
            .query_row(
                "SELECT * FROM feeds WHERE url = ?1",
                params![selector],
                map_feed,
            )
            .optional()?;
        if by_url.is_some() {
            return Ok(by_url);
        }

        self.conn
            .query_row(
                "SELECT * FROM feeds WHERE title = ?1",
                params![selector],
                map_feed,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn feed_by_id(&self, id: i64) -> Result<Option<Feed>> {
        self.conn
            .query_row("SELECT * FROM feeds WHERE id = ?1", params![id], map_feed)
            .optional()
            .map_err(Into::into)
    }

    pub fn require_feed(&self, selector: &str) -> Result<Feed> {
        self.find_feed(selector)?
            .ok_or_else(|| anyhow!("no feed matched `{selector}`"))
    }

    pub fn list_feeds(&self) -> Result<Vec<Feed>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM feeds ORDER BY COALESCE(folder, ''), lower(title)")?;
        let rows = stmt.query_map([], map_feed)?;
        collect_rows(rows)
    }

    pub fn update_feed_success(
        &self,
        feed_id: i64,
        title: &str,
        site_url: Option<&str>,
        description: Option<&str>,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<()> {
        let now = now_ts();
        self.conn.execute(
            "UPDATE feeds
             SET title = ?1,
                 site_url = ?2,
                 description = ?3,
                 etag = ?4,
                 last_modified = ?5,
                 last_checked = ?6,
                 last_error = NULL,
                 updated_at = ?6
             WHERE id = ?7",
            params![
                title,
                site_url,
                description,
                etag,
                last_modified,
                now,
                feed_id
            ],
        )?;
        Ok(())
    }

    pub fn touch_feed_not_modified(&self, feed_id: i64) -> Result<()> {
        let now = now_ts();
        self.conn.execute(
            "UPDATE feeds SET last_checked = ?1, last_error = NULL, updated_at = ?1 WHERE id = ?2",
            params![now, feed_id],
        )?;
        Ok(())
    }

    pub fn set_feed_error(&self, feed_id: i64, error: &str) -> Result<()> {
        let now = now_ts();
        self.conn.execute(
            "UPDATE feeds SET last_checked = ?1, last_error = ?2, updated_at = ?1 WHERE id = ?3",
            params![now, error, feed_id],
        )?;
        Ok(())
    }

    pub fn upsert_items(&self, feed_id: i64, items: &[NewItem]) -> Result<usize> {
        let now = now_ts();
        let mut changed = 0;
        for item in items {
            changed += self.conn.execute(
                "INSERT INTO items (
                    feed_id, guid, url, title, author, summary, content, published, updated,
                    media_url, media_type, media_length, duration_secs, added_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(feed_id, guid) DO UPDATE SET
                    url = excluded.url,
                    title = excluded.title,
                    author = excluded.author,
                    summary = excluded.summary,
                    content = excluded.content,
                    published = excluded.published,
                    updated = excluded.updated,
                    media_url = excluded.media_url,
                    media_type = excluded.media_type,
                    media_length = excluded.media_length,
                    duration_secs = excluded.duration_secs",
                params![
                    feed_id,
                    &item.guid,
                    item.url.as_deref(),
                    &item.title,
                    item.author.as_deref(),
                    item.summary.as_deref(),
                    item.content.as_deref(),
                    item.published,
                    item.updated,
                    item.media_url.as_deref(),
                    item.media_type.as_deref(),
                    item.media_length,
                    item.duration_secs,
                    now
                ],
            )?;
        }
        Ok(changed)
    }

    pub fn get_item(&self, item_id: i64) -> Result<Option<ItemWithFeed>> {
        let sql = format!(
            "SELECT {ITEM_WITH_FEED_SELECT} FROM items i JOIN feeds f ON f.id = i.feed_id WHERE i.id = ?1"
        );
        self.conn
            .query_row(&sql, params![item_id], map_item_with_feed)
            .optional()
            .map_err(Into::into)
    }

    pub fn require_item(&self, item_id: i64) -> Result<ItemWithFeed> {
        self.get_item(item_id)?
            .ok_or_else(|| anyhow!("no item with id {item_id}"))
    }

    pub fn list_items(&self, filter: &ItemFilter) -> Result<Vec<ItemWithFeed>> {
        let mut where_parts: Vec<String> = Vec::new();
        let mut values: Vec<Value> = Vec::new();

        if let Some(feed_id) = filter.feed_id {
            where_parts.push("i.feed_id = ?".to_string());
            values.push(Value::Integer(feed_id));
        }
        if filter.unread_only {
            where_parts.push("i.is_read = 0".to_string());
        }
        if filter.favorites_only {
            where_parts.push("i.is_favorite = 1".to_string());
        }
        if filter.podcasts_only {
            where_parts.push("i.media_url IS NOT NULL".to_string());
        }
        if filter.news_only {
            where_parts.push("i.media_url IS NULL".to_string());
        }
        if let Some(query) = filter
            .search
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            let pattern = format!("%{}%", query.trim());
            where_parts.push(
                "(i.title LIKE ? OR COALESCE(i.summary, '') LIKE ? OR COALESCE(i.content, '') LIKE ?)"
                    .to_string(),
            );
            values.push(Value::Text(pattern.clone()));
            values.push(Value::Text(pattern.clone()));
            values.push(Value::Text(pattern));
        }

        let where_clause = if where_parts.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_parts.join(" AND "))
        };
        let limit = i64::try_from(filter.limit.max(1)).unwrap_or(i64::MAX);
        values.push(Value::Integer(limit));

        let sql = format!(
            "SELECT {ITEM_WITH_FEED_SELECT}
             FROM items i
             JOIN feeds f ON f.id = i.feed_id
             {where_clause}
             ORDER BY COALESCE(i.published, i.updated, i.added_at) DESC, i.id DESC
             LIMIT ?"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(values.iter()), map_item_with_feed)?;
        collect_rows(rows)
    }

    pub fn search_items(&self, query: &str, limit: usize) -> Result<Vec<ItemWithFeed>> {
        let filter = ItemFilter {
            search: Some(query.to_string()),
            limit,
            ..Default::default()
        };
        self.list_items(&filter)
    }

    pub fn mark_item_read(&self, item_id: i64, read: bool) -> Result<usize> {
        let rows = self.conn.execute(
            "UPDATE items SET is_read = ?1 WHERE id = ?2",
            params![bool_to_int(read), item_id],
        )?;
        Ok(rows)
    }

    pub fn mark_all_read(&self, feed_id: Option<i64>) -> Result<usize> {
        let rows = if let Some(feed_id) = feed_id {
            self.conn.execute(
                "UPDATE items SET is_read = 1 WHERE feed_id = ?1",
                params![feed_id],
            )?
        } else {
            self.conn.execute("UPDATE items SET is_read = 1", [])?
        };
        Ok(rows)
    }

    pub fn set_favorite(&self, item_id: i64, favorite: bool) -> Result<usize> {
        let rows = self.conn.execute(
            "UPDATE items SET is_favorite = ?1 WHERE id = ?2",
            params![bool_to_int(favorite), item_id],
        )?;
        Ok(rows)
    }

    pub fn set_position(&self, item_id: i64, seconds: i64) -> Result<usize> {
        if seconds < 0 {
            bail!("position cannot be negative");
        }
        let rows = self.conn.execute(
            "UPDATE items SET last_position_secs = ?1 WHERE id = ?2",
            params![seconds, item_id],
        )?;
        Ok(rows)
    }

    pub fn set_downloaded_path(&self, item_id: i64, path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET downloaded_path = ?1 WHERE id = ?2",
            params![path, item_id],
        )?;
        Ok(())
    }

    pub fn enqueue_item(&self, item_id: i64) -> Result<()> {
        self.require_item(item_id)?;
        let next_position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position), 0) + 1 FROM queue",
            [],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO queue (item_id, position, created_at) VALUES (?1, ?2, ?3)",
            params![item_id, next_position, now_ts()],
        )?;
        Ok(())
    }

    pub fn list_queue(&self) -> Result<Vec<QueueEntry>> {
        let sql = format!(
            "SELECT q.id, q.position, q.created_at, {ITEM_WITH_FEED_SELECT}
             FROM queue q
             JOIN items i ON i.id = q.item_id
             JOIN feeds f ON f.id = i.feed_id
             ORDER BY q.position, q.id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], map_queue_entry)?;
        collect_rows(rows)
    }

    pub fn remove_from_queue(&self, item_id: i64) -> Result<usize> {
        let rows = self
            .conn
            .execute("DELETE FROM queue WHERE item_id = ?1", params![item_id])?;
        Ok(rows)
    }

    pub fn stats(&self) -> Result<LibraryStats> {
        Ok(LibraryStats {
            feeds: self.scalar_i64("SELECT COUNT(*) FROM feeds")?,
            items: self.scalar_i64("SELECT COUNT(*) FROM items")?,
            unread_items: self.scalar_i64("SELECT COUNT(*) FROM items WHERE is_read = 0")?,
            podcast_episodes: self.scalar_i64("SELECT COUNT(*) FROM items WHERE media_url IS NOT NULL")?,
            queued_episodes: self.scalar_i64("SELECT COUNT(*) FROM queue")?,
            downloaded_episodes: self.scalar_i64(
                "SELECT COUNT(*) FROM items WHERE downloaded_path IS NOT NULL AND downloaded_path <> ''",
            )?,
            favorite_items: self.scalar_i64("SELECT COUNT(*) FROM items WHERE is_favorite = 1")?,
        })
    }

    fn scalar_i64(&self, sql: &str) -> Result<i64> {
        self.conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(Into::into)
    }
}

fn collect_rows<T, F>(rows: MappedRows<'_, F>) -> Result<Vec<T>>
where
    F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

fn map_feed(row: &Row<'_>) -> rusqlite::Result<Feed> {
    let kind_text: String = row.get("kind")?;
    Ok(Feed {
        id: row.get("id")?,
        url: row.get("url")?,
        title: row.get("title")?,
        kind: FeedKind::from_str(&kind_text).unwrap_or(FeedKind::Mixed),
        folder: row.get("folder")?,
        site_url: row.get("site_url")?,
        description: row.get("description")?,
        etag: row.get("etag")?,
        last_modified: row.get("last_modified")?,
        last_checked: row.get("last_checked")?,
        last_error: row.get("last_error")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn map_item_with_feed(row: &Row<'_>) -> rusqlite::Result<ItemWithFeed> {
    map_item_with_feed_at(row, 0)
}

fn map_item_with_feed_at(row: &Row<'_>, start: usize) -> rusqlite::Result<ItemWithFeed> {
    let kind_text: String = row.get(start + 21)?;
    let is_read: i64 = row.get(start + 14)?;
    let is_favorite: i64 = row.get(start + 15)?;
    Ok(ItemWithFeed {
        item: Item {
            id: row.get(start)?,
            feed_id: row.get(start + 1)?,
            guid: row.get(start + 2)?,
            url: row.get(start + 3)?,
            title: row.get(start + 4)?,
            author: row.get(start + 5)?,
            summary: row.get(start + 6)?,
            content: row.get(start + 7)?,
            published: row.get(start + 8)?,
            updated: row.get(start + 9)?,
            media_url: row.get(start + 10)?,
            media_type: row.get(start + 11)?,
            media_length: row.get(start + 12)?,
            duration_secs: row.get(start + 13)?,
            is_read: is_read != 0,
            is_favorite: is_favorite != 0,
            downloaded_path: row.get(start + 16)?,
            added_at: row.get(start + 17)?,
            last_position_secs: row.get(start + 18)?,
        },
        feed_title: row.get(start + 19)?,
        feed_url: row.get(start + 20)?,
        feed_kind: FeedKind::from_str(&kind_text).unwrap_or(FeedKind::Mixed),
    })
}

fn map_queue_entry(row: &Row<'_>) -> rusqlite::Result<QueueEntry> {
    Ok(QueueEntry {
        id: row.get(0)?,
        position: row.get(1)?,
        created_at: row.get(2)?,
        item: map_item_with_feed_at(row, 3)?,
    })
}

fn bool_to_int(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_item(guid: &str, title: &str, media: bool) -> NewItem {
        NewItem {
            guid: guid.to_string(),
            url: Some(format!("https://example.com/{guid}")),
            title: title.to_string(),
            author: Some("Example Author".to_string()),
            summary: Some("Summary".to_string()),
            content: Some("Full content".to_string()),
            published: Some(1_735_689_600),
            updated: None,
            media_url: media.then(|| format!("https://example.com/{guid}.mp3")),
            media_type: media.then(|| "audio/mpeg".to_string()),
            media_length: media.then_some(100),
            duration_secs: media.then_some(60),
        }
    }

    #[test]
    fn add_feed_and_upsert_items_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        let feed_id = store
            .add_feed(
                "https://example.com/feed.xml",
                Some("Example"),
                FeedKind::Mixed,
                Some("Demo"),
            )
            .unwrap();
        let count = store
            .upsert_items(
                feed_id,
                &[
                    sample_item("one", "First", false),
                    sample_item("two", "Second", true),
                ],
            )
            .unwrap();
        assert_eq!(count, 2);

        let items = store
            .list_items(&ItemFilter {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| item.item.is_podcast_episode()));
    }

    #[test]
    fn mark_read_and_search_work() {
        let store = Store::open_in_memory().unwrap();
        let feed_id = store
            .add_feed(
                "https://example.com/feed.xml",
                Some("Example"),
                FeedKind::News,
                None,
            )
            .unwrap();
        store
            .upsert_items(feed_id, &[sample_item("one", "Rust News", false)])
            .unwrap();
        let item = store.search_items("Rust", 5).unwrap().remove(0);
        assert!(!item.item.is_read);
        store.mark_item_read(item.item.id, true).unwrap();
        let item = store.require_item(item.item.id).unwrap();
        assert!(item.item.is_read);
    }

    #[test]
    fn queue_and_stats_work() {
        let store = Store::open_in_memory().unwrap();
        let feed_id = store
            .add_feed(
                "https://example.com/feed.xml",
                Some("Example"),
                FeedKind::Podcast,
                None,
            )
            .unwrap();
        store
            .upsert_items(feed_id, &[sample_item("episode", "Episode", true)])
            .unwrap();
        let item = store.search_items("Episode", 5).unwrap().remove(0);
        store.enqueue_item(item.item.id).unwrap();
        assert_eq!(store.list_queue().unwrap().len(), 1);
        let stats = store.stats().unwrap();
        assert_eq!(stats.queued_episodes, 1);
        assert_eq!(stats.podcast_episodes, 1);
    }
}
