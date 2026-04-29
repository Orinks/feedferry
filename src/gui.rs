use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use wxdragon::dialogs::dir_dialog::DirDialog;
use wxdragon::dialogs::file_dialog::{FileDialog, FileDialogStyle};
use wxdragon::dialogs::message_dialog::{MessageDialog, MessageDialogStyle};
use wxdragon::dialogs::text_entry_dialog::TextEntryDialog;
use wxdragon::id::{ID_OK, ID_YES};
use wxdragon::prelude::*;

use crate::accessibility::Accessibility;
use crate::config;
use crate::database::Store;
use crate::downloader::Downloader;
use crate::models::{Feed, FeedKind, ItemFilter, ItemWithFeed, LibraryStats, QueueEntry};
use crate::opml::{export_opml, parse_opml};
use crate::player::{open_item_url, play_item};
use crate::sync::{self, NoopObserver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKindFilter {
    All,
    News,
    Podcasts,
}

impl ItemKindFilter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All items",
            Self::News => "News only",
            Self::Podcasts => "Podcasts only",
        }
    }

    fn from_choice(index: Option<u32>) -> Self {
        match index {
            Some(1) => Self::News,
            Some(2) => Self::Podcasts,
            _ => Self::All,
        }
    }
}

#[derive(Debug, Clone)]
struct GuiFilter {
    search: String,
    unread_only: bool,
    favorites_only: bool,
    kind: ItemKindFilter,
    limit: usize,
}

impl Default for GuiFilter {
    fn default() -> Self {
        Self {
            search: String::new(),
            unread_only: false,
            favorites_only: false,
            kind: ItemKindFilter::All,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpdateSummary {
    fetched: usize,
    new_or_updated_items: usize,
    errors: Vec<String>,
}

impl UpdateSummary {
    fn message(&self) -> String {
        if self.errors.is_empty() {
            format!(
                "Updated {} feed(s); {} new or changed item(s).",
                self.fetched, self.new_or_updated_items
            )
        } else {
            format!(
                "Updated {} feed(s); {} new or changed item(s); {} error(s): {}.",
                self.fetched,
                self.new_or_updated_items,
                self.errors.len(),
                self.errors.join("; ")
            )
        }
    }
}

fn update_summary_for_store(store: &Store, selectors: &[String]) -> Result<UpdateSummary> {
    let reports = sync::update_selected_feeds(store, selectors, &NoopObserver)?;
    Ok(UpdateSummary {
        fetched: reports.iter().filter(|report| report.fetched).count(),
        new_or_updated_items: reports
            .iter()
            .map(|report| report.new_or_updated_items)
            .sum(),
        errors: reports
            .iter()
            .filter_map(|report| report.error.clone())
            .collect(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiPreferences {
    pub screen_reader_mode: bool,
    pub speech_command: String,
    pub player_command: String,
    pub download_dir: Option<PathBuf>,
    pub mark_read_on_open: bool,
    pub update_after_add: bool,
    pub show_status_log: bool,
}

impl Default for GuiPreferences {
    fn default() -> Self {
        Self {
            screen_reader_mode: false,
            speech_command: String::new(),
            player_command: String::new(),
            download_dir: None,
            mark_read_on_open: true,
            update_after_add: true,
            show_status_log: true,
        }
    }
}

impl GuiPreferences {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("could not create {}", parent.display()))?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("could not write preferences to {}", path.display()))
    }
}

pub struct FeedFerryApp {
    preference_path: PathBuf,
    db_path: PathBuf,
    store: Option<Store>,
    open_error: Option<String>,
    feeds: Vec<Feed>,
    items: Vec<ItemWithFeed>,
    queue: Vec<QueueEntry>,
    stats: Option<LibraryStats>,
    selected_feed_id: Option<i64>,
    selected_item_id: Option<i64>,
    selected_item: Option<ItemWithFeed>,
    filter: GuiFilter,
    preferences: GuiPreferences,
    download_dir_text: String,
    latest_status: String,
    status_log: Vec<String>,
}

impl FeedFerryApp {
    pub fn new(data_dir: PathBuf, db_path: PathBuf) -> Self {
        let preference_path = data_dir.join("gui_preferences.json");
        let preferences = GuiPreferences::load(&preference_path);
        let download_dir = preferences
            .download_dir
            .clone()
            .or_else(|| config::default_download_dir(&data_dir).ok())
            .unwrap_or_else(|| data_dir.join("downloads"));
        let store_result = Store::open(&db_path);
        let open_error = store_result
            .as_ref()
            .err()
            .map(|error| format!("Could not open database at {}: {error}", db_path.display()));

        let mut app = Self {
            preference_path,
            db_path,
            store: store_result.ok(),
            open_error,
            feeds: Vec::new(),
            items: Vec::new(),
            queue: Vec::new(),
            stats: None,
            selected_feed_id: None,
            selected_item_id: None,
            selected_item: None,
            filter: GuiFilter::default(),
            preferences,
            download_dir_text: download_dir.to_string_lossy().to_string(),
            latest_status: "Ready.".to_string(),
            status_log: Vec::new(),
        };
        app.refresh_all();
        app
    }

    fn refresh_all(&mut self) {
        let filter = self.current_item_filter();
        let Some(store) = self.store.as_ref() else {
            if let Some(error) = self.open_error.clone() {
                self.latest_status = error;
            }
            return;
        };

        let feeds_result = store.list_feeds();
        let items_result = store.list_items(&filter);
        let queue_result = store.list_queue();
        let stats_result = store.stats();

        match feeds_result {
            Ok(feeds) => self.feeds = feeds,
            Err(error) => self.record_error(error),
        }
        match items_result {
            Ok(items) => self.items = items,
            Err(error) => self.record_error(error),
        }
        match queue_result {
            Ok(queue) => self.queue = queue,
            Err(error) => self.record_error(error),
        }
        match stats_result {
            Ok(stats) => self.stats = Some(stats),
            Err(error) => self.record_error(error),
        }

        if let Some(item_id) = self.selected_item_id {
            self.refresh_selected_item(item_id);
        }
    }

    fn current_item_filter(&self) -> ItemFilter {
        ItemFilter {
            feed_id: self.selected_feed_id,
            unread_only: self.filter.unread_only,
            favorites_only: self.filter.favorites_only,
            podcasts_only: self.filter.kind == ItemKindFilter::Podcasts,
            news_only: self.filter.kind == ItemKindFilter::News,
            search: optional_trimmed(&self.filter.search),
            limit: self.filter.limit,
        }
    }

    fn refresh_selected_item(&mut self, item_id: i64) {
        self.selected_item = self
            .items
            .iter()
            .find(|item| item.item.id == item_id)
            .cloned();
        if self.selected_item.is_none() {
            self.selected_item_id = None;
        }
    }

    fn select_feed_index(&mut self, index: i32) {
        self.selected_feed_id = if index <= 0 {
            None
        } else {
            self.feeds.get((index - 1) as usize).map(|feed| feed.id)
        };
        self.selected_item_id = None;
        self.selected_item = None;
        self.refresh_all();
        self.announce("Feed selection changed.");
    }

    fn selected_feed_row(&self) -> i64 {
        self.selected_feed_id
            .and_then(|id| self.feeds.iter().position(|feed| feed.id == id))
            .map(|idx| idx as i64 + 1)
            .unwrap_or(0)
    }

    fn update_summary(&self, selectors: &[String]) -> Result<UpdateSummary> {
        let Some(store) = self.store.as_ref() else {
            return Ok(UpdateSummary {
                fetched: 0,
                new_or_updated_items: 0,
                errors: Vec::new(),
            });
        };

        update_summary_for_store(store, selectors)
    }

    fn update_feeds(&mut self, selectors: Vec<String>) {
        self.announce("Updating feeds.");
        match self.update_summary(&selectors) {
            Ok(summary) => {
                self.refresh_all();
                self.announce(summary.message());
            }
            Err(error) => self.record_error(error),
        }
    }

    fn import_opml_path(&mut self, path: PathBuf, ui_state: UiStateHandle) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|text| parse_opml(&text))
        {
            Ok(entries) => {
                let mut imported = 0;
                for entry in entries {
                    if store
                        .add_feed(
                            &entry.xml_url,
                            Some(&entry.title),
                            entry.kind,
                            entry.folder.as_deref(),
                        )
                        .is_ok()
                    {
                        imported += 1;
                    }
                }
                self.refresh_all();
                self.announce(format!(
                    "Imported {imported} feed(s). Updating feeds in the background."
                ));
                let db_path = self.db_path.clone();
                std::thread::spawn(move || {
                    let message = match Store::open(&db_path)
                        .and_then(|store| update_summary_for_store(&store, &[]))
                    {
                        Ok(summary) => {
                            format!("Imported {imported} feed(s). {}", summary.message())
                        }
                        Err(error) => {
                            format!("Imported {imported} feed(s). Error updating feeds: {error}")
                        }
                    };
                    call_after(Box::new(move || {
                        ui_state.refresh_after_background_update(message);
                    }));
                });
            }
            Err(error) => self.record_error(error),
        }
    }

    fn select_item_index(&mut self, index: Option<u32>) {
        if let Some(item) = index.and_then(|idx| self.items.get(idx as usize)) {
            self.selected_item_id = Some(item.item.id);
            self.selected_item = Some(item.clone());
            self.announce(readable_item_label(item));
        } else {
            self.selected_item_id = None;
            self.selected_item = None;
        }
    }

    fn apply_filter(
        &mut self,
        search: String,
        unread: bool,
        favorites: bool,
        kind: ItemKindFilter,
        limit: usize,
    ) {
        self.filter.search = search;
        self.filter.unread_only = unread;
        self.filter.favorites_only = favorites;
        self.filter.kind = kind;
        self.filter.limit = limit.clamp(1, 1000);
        self.refresh_all();
        self.announce("Filter applied.");
    }

    fn add_feed_url(&mut self, url: &str) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let url = url.trim();
        if url.is_empty() {
            self.record_error("Feed URL cannot be empty.");
            return;
        }

        match store.add_feed(url, None, FeedKind::Mixed, None) {
            Ok(feed_id) => {
                self.selected_feed_id = Some(feed_id);
                self.refresh_all();
                self.announce(format!("Added feed {url}."));
            }
            Err(error) => self.record_error(error),
        }
    }

    fn remove_selected_feed(&mut self) {
        let Some(feed_id) = self.selected_feed_id else {
            self.record_error("Select a feed first.");
            return;
        };
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.remove_feed(&feed_id.to_string()) {
            Ok(rows) => {
                self.selected_feed_id = None;
                self.selected_item_id = None;
                self.selected_item = None;
                self.refresh_all();
                self.announce(format!("Removed {rows} feed(s)."));
            }
            Err(error) => self.record_error(error),
        }
    }

    fn update_all_feeds(&mut self) {
        self.update_feeds(Vec::new());
    }

    fn update_selected_feed(&mut self) {
        let selectors = self
            .selected_feed_id
            .map(|id| vec![id.to_string()])
            .unwrap_or_default();
        self.update_feeds(selectors);
    }

    fn mark_selected_read_state(&mut self, read: bool) {
        let Some(item_id) = self.selected_item_id else {
            self.record_error("Select an item first.");
            return;
        };
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.mark_item_read(item_id, read) {
            Ok(_) => {
                self.refresh_all();
                self.announce(if read {
                    "Marked read."
                } else {
                    "Marked unread."
                });
            }
            Err(error) => self.record_error(error),
        }
    }

    fn mark_all_visible_read(&mut self) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.mark_all_read(self.selected_feed_id) {
            Ok(count) => {
                self.refresh_all();
                self.announce(format!("Marked {count} item(s) read."));
            }
            Err(error) => self.record_error(error),
        }
    }

    fn toggle_selected_favorite(&mut self) {
        let Some(item) = self.selected_item.as_ref() else {
            self.record_error("Select an item first.");
            return;
        };
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let favorite = !item.item.is_favorite;
        match store.set_favorite(item.item.id, favorite) {
            Ok(_) => {
                self.refresh_all();
                self.announce(if favorite {
                    "Added favorite."
                } else {
                    "Removed favorite."
                });
            }
            Err(error) => self.record_error(error),
        }
    }

    fn queue_selected(&mut self) {
        let Some(item) = self.selected_item.as_ref() else {
            self.record_error("Select an item first.");
            return;
        };
        if !item.item.is_podcast_episode() {
            self.record_error("Only podcast episodes can be queued.");
            return;
        }
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.enqueue_item(item.item.id) {
            Ok(()) => {
                self.refresh_all();
                self.announce("Queued episode.");
            }
            Err(error) => self.record_error(error),
        }
    }

    fn open_selected(&mut self) {
        let Some(item) = self.selected_item.clone() else {
            self.record_error("Select an item first.");
            return;
        };
        if self.preferences.mark_read_on_open {
            if let Some(store) = self.store.as_ref() {
                let _ = store.mark_item_read(item.item.id, true);
            }
        }
        match open_item_url(&item) {
            Ok(()) => {
                self.refresh_all();
                self.announce("Opened item.");
            }
            Err(error) => self.record_error(error),
        }
    }

    fn play_selected(&mut self) {
        let Some(item) = self.selected_item.clone() else {
            self.record_error("Select a podcast episode first.");
            return;
        };
        let player = optional_trimmed(&self.preferences.player_command);
        match play_item(&item, player.as_deref()) {
            Ok(()) => self.announce("Started playback."),
            Err(error) => self.record_error(error),
        }
    }

    fn download_selected(&mut self, force: bool) {
        let Some(item) = self.selected_item.clone() else {
            self.record_error("Select a podcast episode first.");
            return;
        };
        let download_dir = PathBuf::from(self.download_dir_text.trim());
        match Downloader::new()
            .and_then(|downloader| downloader.download_item(&item, &download_dir, force))
        {
            Ok(path) => {
                if let Some(store) = self.store.as_ref() {
                    let _ = store.set_downloaded_path(item.item.id, &path.to_string_lossy());
                }
                self.refresh_all();
                self.announce(format!("Downloaded to {}.", path.display()));
            }
            Err(error) => self.record_error(error),
        }
    }

    fn download_queue(&mut self, force: bool) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let download_dir = PathBuf::from(self.download_dir_text.trim());
        let downloader = match Downloader::new() {
            Ok(downloader) => downloader,
            Err(error) => {
                self.record_error(error);
                return;
            }
        };
        let mut downloaded = 0;
        let mut errors = Vec::new();
        for entry in self.queue.clone() {
            match downloader.download_item(&entry.item, &download_dir, force) {
                Ok(path) => {
                    let _ = store.set_downloaded_path(entry.item.item.id, &path.to_string_lossy());
                    downloaded += 1;
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        self.refresh_all();
        if errors.is_empty() {
            self.announce(format!("Downloaded {downloaded} queued episode(s)."));
        } else {
            self.announce(format!(
                "Downloaded {downloaded} queued episode(s); {} error(s): {}",
                errors.len(),
                errors.join("; ")
            ));
        }
    }

    fn export_opml_path(&mut self, path: PathBuf) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match store.list_feeds() {
            Ok(feeds) => {
                let xml = export_opml(&feeds, "FeedFerry subscriptions");
                match fs::write(&path, xml) {
                    Ok(()) => {
                        self.announce(format!("Exported OPML to {}.", path.display()));
                    }
                    Err(error) => self.record_error(error),
                }
            }
            Err(error) => self.record_error(error),
        }
    }

    fn set_download_dir(&mut self, path: PathBuf) {
        self.download_dir_text = path.to_string_lossy().to_string();
        self.save_preferences();
    }

    fn save_preferences(&mut self) {
        self.preferences.download_dir = Some(PathBuf::from(self.download_dir_text.trim()));
        match self.preferences.save(&self.preference_path) {
            Ok(()) => self.announce("Settings saved."),
            Err(error) => self.record_error(error),
        }
    }

    fn announce(&mut self, message: impl AsRef<str>) {
        let message = sanitize_announcement(message.as_ref());
        self.latest_status = message.clone();
        self.status_log.push(message.clone());
        if self.status_log.len() > 50 {
            self.status_log.remove(0);
        }
        if self.preferences.screen_reader_mode {
            let command = self.preferences.speech_command.trim().to_string();
            if !command.is_empty() {
                let _ = Accessibility::new(true, Some(command)).status(&message);
            }
        }
    }

    fn record_error(&mut self, error: impl std::fmt::Display) {
        self.announce(format!("Error: {error}"));
    }
}

#[derive(Clone, Copy)]
struct WxControls {
    frame: Frame,
    feeds: ListCtrl,
    items: ListBox,
    detail: TextCtrl,
    queue: ListBox,
    status: StaticText,
}

#[derive(Clone, Copy)]
struct UiStateHandle(*const RefCell<(FeedFerryApp, WxControls)>);

unsafe impl Send for UiStateHandle {}

impl UiStateHandle {
    fn new(state: &Rc<RefCell<(FeedFerryApp, WxControls)>>) -> Self {
        Self(Rc::as_ptr(state))
    }

    fn refresh_after_background_update(self, message: String) {
        let state = unsafe { &*self.0 };
        let mut state = state.borrow_mut();
        let controls = state.1;
        state.0.refresh_all();
        state.0.announce(message);
        controls.refresh(&state.0);
    }
}

impl WxControls {
    fn refresh(&self, app: &FeedFerryApp) {
        self.feeds.delete_all_items();
        self.feeds.insert_item(0, "All feeds", None);
        for (idx, feed) in app.feeds.iter().enumerate() {
            self.feeds
                .insert_item(idx as i64 + 1, &feed_accessible_label(feed), None);
        }
        let selected_row = app.selected_feed_row();
        self.feeds.set_item_state(
            selected_row,
            ListItemState::Selected | ListItemState::Focused,
            ListItemState::Selected | ListItemState::Focused,
        );
        self.feeds.ensure_visible(selected_row);

        self.items.clear();
        for item in &app.items {
            self.items.append(&readable_item_label(item));
        }
        if let Some(item_id) = app.selected_item_id {
            if let Some(index) = app.items.iter().position(|item| item.item.id == item_id) {
                self.items.set_selection(index as u32, true);
            }
        }

        self.queue.clear();
        for entry in &app.queue {
            self.queue.append(&format!(
                "{}: {}",
                entry.position,
                readable_item_label(&entry.item)
            ));
        }

        self.refresh_detail_and_status(app);
    }

    fn refresh_detail_and_status(&self, app: &FeedFerryApp) {
        self.detail.set_value(
            &app.selected_item
                .as_ref()
                .map(item_detail_text)
                .unwrap_or_default(),
        );
        self.status.set_label(&status_text(app));
        self.frame.set_status_text(&app.latest_status, 0);
    }
}

fn status_text(app: &FeedFerryApp) -> String {
    let stats = app.stats.as_ref().map_or_else(
        || "No library statistics available.".to_string(),
        |stats| {
            format!(
                "Feeds: {}. Items: {}. Unread: {}. Podcasts: {}. Queued: {}. Downloaded: {}. Favorites: {}.",
                stats.feeds,
                stats.items,
                stats.unread_items,
                stats.podcast_episodes,
                stats.queued_episodes,
                stats.downloaded_episodes,
                stats.favorite_items
            )
        },
    );
    format!("{stats}\n{}", app.latest_status)
}

fn with_state(
    state: &Rc<RefCell<(FeedFerryApp, WxControls)>>,
    action: impl FnOnce(&mut FeedFerryApp, WxControls),
) {
    let mut state = state.borrow_mut();
    let controls = state.1;
    action(&mut state.0, controls);
    controls.refresh(&state.0);
}

fn add_button(parent: &Panel, row: &BoxSizer, label: &str) -> Button {
    let button = Button::builder(parent).with_label(label).build();
    describe_control(&button, label);
    row.add(&button, 0, SizerFlag::All, 3);
    button
}

fn describe_control(control: &impl WxWidget, description: &str) {
    control.set_name(description);
    control.set_tooltip(description);
}

fn add_label(parent: &Panel, row: &BoxSizer, label: &str) {
    let text = StaticText::builder(parent).with_label(label).build();
    text.set_name(label);
    row.add(&text, 0, SizerFlag::All, 4);
}

fn build_ui(data_dir: PathBuf, db_path: PathBuf) {
    let frame = Frame::builder()
        .with_title("FeedFerry")
        .with_size(Size::new(1280, 820))
        .build();
    set_top_window(&frame);
    frame.create_status_bar(1, 0, wxdragon::id::ID_ANY as i32, "status");

    let panel = Panel::builder(&frame).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();
    let toolbar = BoxSizer::builder(Orientation::Horizontal).build();

    let add_feed = add_button(&panel, &toolbar, "Add feed");
    let remove_feed = add_button(&panel, &toolbar, "Remove selected feed");
    let update_all = add_button(&panel, &toolbar, "Update all feeds");
    let update_selected = add_button(&panel, &toolbar, "Update selected feed");
    let import_opml = add_button(&panel, &toolbar, "Import OPML");
    let export_opml_button = add_button(&panel, &toolbar, "Export OPML");
    let settings = add_button(&panel, &toolbar, "Settings");
    let about = add_button(&panel, &toolbar, "About");
    root.add_sizer(&toolbar, 0, SizerFlag::Expand, 0);

    let filters = BoxSizer::builder(Orientation::Horizontal).build();
    add_label(&panel, &filters, "Search feeds and items");
    let search = TextCtrl::builder(&panel)
        .with_size(Size::new(240, -1))
        .build();
    describe_control(&search, "Search feeds and items");
    filters.add(&search, 1, SizerFlag::All | SizerFlag::Expand, 4);
    let unread_only = CheckBox::builder(&panel)
        .with_label("Unread items only")
        .build();
    describe_control(&unread_only, "Show unread items only");
    filters.add(
        &unread_only,
        0,
        SizerFlag::All | SizerFlag::AlignCenterVertical,
        4,
    );
    let favorites_only = CheckBox::builder(&panel)
        .with_label("Favorite items only")
        .build();
    describe_control(&favorites_only, "Show favorite items only");
    filters.add(
        &favorites_only,
        0,
        SizerFlag::All | SizerFlag::AlignCenterVertical,
        4,
    );
    let kind = Choice::builder(&panel)
        .with_choices(vec![
            ItemKindFilter::All.label().to_string(),
            ItemKindFilter::News.label().to_string(),
            ItemKindFilter::Podcasts.label().to_string(),
        ])
        .with_selection(Some(0))
        .build();
    describe_control(&kind, "Item type filter");
    filters.add(&kind, 0, SizerFlag::All | SizerFlag::AlignCenterVertical, 4);
    add_label(&panel, &filters, "Item limit");
    let limit = TextCtrl::builder(&panel)
        .with_value("100")
        .with_size(Size::new(70, -1))
        .build();
    describe_control(&limit, "Maximum number of items to show");
    filters.add(
        &limit,
        0,
        SizerFlag::All | SizerFlag::AlignCenterVertical,
        4,
    );
    let apply_filter = add_button(&panel, &filters, "Apply filter");
    root.add_sizer(&filters, 0, SizerFlag::Expand, 0);

    let body = BoxSizer::builder(Orientation::Horizontal).build();
    let feeds_column = BoxSizer::builder(Orientation::Vertical).build();
    add_label(&panel, &feeds_column, "Feeds");
    let feeds = ListCtrl::builder(&panel)
        .with_size(Size::new(280, -1))
        .with_style(ListCtrlStyle::Report | ListCtrlStyle::SingleSel | ListCtrlStyle::NoHeader)
        .build();
    feeds.insert_column(0, "Feeds", ListColumnFormat::Left, 260);
    describe_control(&feeds, "Feeds list");
    feeds_column.add(&feeds, 1, SizerFlag::All | SizerFlag::Expand, 4);
    body.add_sizer(&feeds_column, 0, SizerFlag::Expand, 0);

    let center = BoxSizer::builder(Orientation::Vertical).build();
    add_label(&panel, &center, "Items");
    let items = ListBox::builder(&panel)
        .with_style(
            ListBoxStyle::Default
                | ListBoxStyle::AlwaysScrollbar
                | ListBoxStyle::HorizontalScrollbar,
        )
        .build();
    describe_control(&items, "Items list");
    center.add(&items, 1, SizerFlag::All | SizerFlag::Expand, 4);
    let item_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let open = add_button(&panel, &item_buttons, "Open selected item");
    let play = add_button(&panel, &item_buttons, "Play selected episode");
    let mark_read = add_button(&panel, &item_buttons, "Mark selected read");
    let mark_unread = add_button(&panel, &item_buttons, "Mark selected unread");
    let favorite = add_button(&panel, &item_buttons, "Toggle selected favorite");
    let queue_button = add_button(&panel, &item_buttons, "Queue selected item");
    let download = add_button(&panel, &item_buttons, "Download selected episode");
    center.add_sizer(&item_buttons, 0, SizerFlag::Expand, 0);
    let queue_buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let download_queue = add_button(&panel, &queue_buttons, "Download queue");
    let mark_visible = add_button(&panel, &queue_buttons, "Mark visible read");
    center.add_sizer(&queue_buttons, 0, SizerFlag::Expand, 0);
    add_label(&panel, &center, "Download queue");
    let queue = ListBox::builder(&panel)
        .with_size(Size::new(-1, 110))
        .with_style(ListBoxStyle::Default | ListBoxStyle::AlwaysScrollbar)
        .build();
    describe_control(&queue, "Download queue list");
    center.add(&queue, 0, SizerFlag::All | SizerFlag::Expand, 4);
    body.add_sizer(&center, 1, SizerFlag::Expand, 0);

    let detail_column = BoxSizer::builder(Orientation::Vertical).build();
    add_label(&panel, &detail_column, "Selected item details");
    let detail = TextCtrl::builder(&panel)
        .with_style(TextCtrlStyle::MultiLine | TextCtrlStyle::ReadOnly | TextCtrlStyle::WordWrap)
        .with_size(Size::new(390, -1))
        .build();
    describe_control(&detail, "Selected item details");
    detail_column.add(&detail, 1, SizerFlag::All | SizerFlag::Expand, 4);
    body.add_sizer(&detail_column, 0, SizerFlag::Expand, 0);
    root.add_sizer(&body, 1, SizerFlag::Expand, 0);

    let status = StaticText::builder(&panel).with_label("Ready.").build();
    status.set_name("Status");
    root.add(&status, 0, SizerFlag::All | SizerFlag::Expand, 4);

    panel.set_sizer(root, true);

    let controls = WxControls {
        frame,
        feeds,
        items,
        detail,
        queue,
        status,
    };
    let state = Rc::new(RefCell::new((
        FeedFerryApp::new(data_dir, db_path),
        controls,
    )));
    controls.refresh(&state.borrow().0);

    {
        let state = Rc::clone(&state);
        feeds.on_item_selected(move |event| {
            with_state(&state, |app, _| {
                app.select_feed_index(event.get_item_index())
            });
        });
    }
    {
        let state = Rc::clone(&state);
        items.on_selection_changed(move |_| {
            let index = items.get_selection();
            let mut state = state.borrow_mut();
            let controls = state.1;
            state.0.select_item_index(index);
            controls.refresh_detail_and_status(&state.0);
        });
    }
    {
        let state = Rc::clone(&state);
        apply_filter.on_click(move |_| {
            let limit_value = limit.get_value().trim().parse::<usize>().unwrap_or(100);
            with_state(&state, |app, _| {
                app.apply_filter(
                    search.get_value(),
                    unread_only.get_value(),
                    favorites_only.get_value(),
                    ItemKindFilter::from_choice(kind.get_selection()),
                    limit_value,
                );
            });
        });
    }
    {
        let state = Rc::clone(&state);
        add_feed.on_click(move |_| {
            let dialog = TextEntryDialog::builder(&frame, "Feed URL:", "Add feed").build();
            if dialog.show_modal() == ID_OK {
                if let Some(url) = dialog.get_value() {
                    with_state(&state, |app, _| app.add_feed_url(&url));
                }
            }
        });
    }
    {
        let state = Rc::clone(&state);
        remove_feed.on_click(move |_| {
            let dialog = MessageDialog::builder(&frame, "Remove the selected feed?", "Remove feed")
                .with_style(MessageDialogStyle::YesNo | MessageDialogStyle::IconQuestion)
                .build();
            if dialog.show_modal() == ID_YES {
                with_state(&state, |app, _| app.remove_selected_feed());
            }
        });
    }
    {
        let state = Rc::clone(&state);
        update_all.on_click(move |_| with_state(&state, |app, _| app.update_all_feeds()));
    }
    {
        let state = Rc::clone(&state);
        update_selected.on_click(move |_| with_state(&state, |app, _| app.update_selected_feed()));
    }
    {
        let state = Rc::clone(&state);
        open.on_click(move |_| with_state(&state, |app, _| app.open_selected()));
    }
    {
        let state = Rc::clone(&state);
        play.on_click(move |_| with_state(&state, |app, _| app.play_selected()));
    }
    {
        let state = Rc::clone(&state);
        mark_read
            .on_click(move |_| with_state(&state, |app, _| app.mark_selected_read_state(true)));
    }
    {
        let state = Rc::clone(&state);
        mark_unread
            .on_click(move |_| with_state(&state, |app, _| app.mark_selected_read_state(false)));
    }
    {
        let state = Rc::clone(&state);
        favorite.on_click(move |_| with_state(&state, |app, _| app.toggle_selected_favorite()));
    }
    {
        let state = Rc::clone(&state);
        queue_button.on_click(move |_| with_state(&state, |app, _| app.queue_selected()));
    }
    {
        let state = Rc::clone(&state);
        download.on_click(move |_| with_state(&state, |app, _| app.download_selected(false)));
    }
    {
        let state = Rc::clone(&state);
        download_queue.on_click(move |_| with_state(&state, |app, _| app.download_queue(false)));
    }
    {
        let state = Rc::clone(&state);
        mark_visible.on_click(move |_| with_state(&state, |app, _| app.mark_all_visible_read()));
    }
    {
        let state = Rc::clone(&state);
        import_opml.on_click(move |_| {
            let dialog = FileDialog::builder(&frame)
                .with_message("Import OPML")
                .with_wildcard("OPML files (*.opml;*.xml)|*.opml;*.xml|All files (*.*)|*.*")
                .with_style(FileDialogStyle::Open | FileDialogStyle::FileMustExist)
                .build();
            if dialog.show_modal() == ID_OK {
                if let Some(path) = dialog.get_path() {
                    let ui_state = UiStateHandle::new(&state);
                    with_state(&state, |app, _| {
                        app.import_opml_path(PathBuf::from(path), ui_state)
                    });
                }
            }
        });
    }
    {
        let state = Rc::clone(&state);
        export_opml_button.on_click(move |_| {
            let dialog = FileDialog::builder(&frame)
                .with_message("Export OPML")
                .with_wildcard("OPML files (*.opml)|*.opml|All files (*.*)|*.*")
                .with_style(FileDialogStyle::Save | FileDialogStyle::OverwritePrompt)
                .build();
            if dialog.show_modal() == ID_OK {
                if let Some(path) = dialog.get_path() {
                    with_state(&state, |app, _| app.export_opml_path(PathBuf::from(path)));
                }
            }
        });
    }
    {
        let state = Rc::clone(&state);
        settings.on_click(move |_| {
            let current = state.borrow().0.download_dir_text.clone();
            let dialog = DirDialog::builder(&frame, "Choose download directory", &current).build();
            if dialog.show_modal() == ID_OK {
                if let Some(path) = dialog.get_path() {
                    with_state(&state, |app, _| app.set_download_dir(PathBuf::from(path)));
                }
            }
        });
    }
    about.on_click(move |_| {
        MessageDialog::builder(
            &frame,
            "FeedFerry is now using wxDragon/wxWidgets native controls for stronger platform accessibility.",
            "About FeedFerry accessibility",
        )
        .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
        .build()
        .show_modal();
    });

    frame.centre();
    frame.show(true);
}

pub fn readable_item_label(item: &ItemWithFeed) -> String {
    let read = if item.item.is_read { "read" } else { "unread" };
    let favorite = if item.item.is_favorite {
        ", favorite"
    } else {
        ""
    };
    let kind = if item.item.is_podcast_episode() {
        "podcast episode"
    } else {
        "article"
    };
    let duration = format_duration(item.item.duration_secs)
        .map(|duration| format!(", duration {duration}"))
        .unwrap_or_default();
    let date = format_timestamp(item.item.primary_timestamp());
    format!(
        "{}: {}, {}{} from {}, {}, id {}{}",
        item.item.title, read, favorite, kind, item.feed_title, date, item.item.id, duration
    )
}

pub fn item_detail_text(item: &ItemWithFeed) -> String {
    let mut parts = Vec::new();
    parts.push(item.item.title.clone());
    parts.push(format!("Feed: {}", item.feed_title));
    parts.push(format!(
        "Status: {}",
        if item.item.is_read { "read" } else { "unread" }
    ));
    parts.push(format!(
        "Favorite: {}",
        if item.item.is_favorite { "yes" } else { "no" }
    ));
    parts.push(format!(
        "Date: {}",
        format_timestamp(item.item.primary_timestamp())
    ));
    if let Some(author) = &item.item.author {
        parts.push(format!("Author: {author}"));
    }
    if let Some(url) = &item.item.url {
        parts.push(format!("Article URL: {url}"));
    }
    if let Some(media_url) = &item.item.media_url {
        parts.push(format!("Media URL: {media_url}"));
    }
    if let Some(media_type) = &item.item.media_type {
        parts.push(format!("Media type: {media_type}"));
    }
    if let Some(duration) = format_duration(item.item.duration_secs) {
        parts.push(format!("Duration: {duration}"));
    }
    if let Some(path) = &item.item.downloaded_path {
        parts.push(format!("Downloaded file: {path}"));
    }
    if item.item.last_position_secs > 0 {
        parts.push(format!(
            "Saved playback position: {}",
            format_duration(Some(item.item.last_position_secs))
                .unwrap_or_else(|| item.item.last_position_secs.to_string())
        ));
    }
    parts.push(String::new());
    if let Some(summary) = &item.item.summary {
        parts.push(summary.clone());
    }
    if let Some(content) = &item.item.content {
        if item.item.summary.as_deref() != Some(content.as_str()) {
            parts.push(String::new());
            parts.push(content.clone());
        }
    }
    parts.join("\n")
}

fn feed_accessible_label(feed: &Feed) -> String {
    let folder = feed
        .folder
        .as_ref()
        .map(|folder| format!(", folder {folder}"))
        .unwrap_or_default();
    let checked = feed
        .last_checked
        .map(|ts| format!(", last checked {}", format_timestamp(Some(ts))))
        .unwrap_or_else(|| ", never checked".to_string());
    let error = feed
        .last_error
        .as_ref()
        .map(|err| format!(", error {err}"))
        .unwrap_or_default();
    format!(
        "{}, {} feed, id {}{}{}{}",
        feed.title, feed.kind, feed.id, folder, checked, error
    )
}

fn format_timestamp(timestamp: Option<i64>) -> String {
    let Some(timestamp) = timestamp else {
        return "unknown date".to_string();
    };
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| {
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "unknown date".to_string())
}

fn format_duration(duration_secs: Option<i64>) -> Option<String> {
    let seconds = duration_secs?;
    if seconds < 0 {
        return None;
    }
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        Some(format!("{hours}:{minutes:02}:{seconds:02}"))
    } else {
        Some(format!("{minutes}:{seconds:02}"))
    }
}

fn optional_trimmed(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn sanitize_announcement(message: &str) -> String {
    message
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(700)
        .collect()
}

pub fn run_native(data_dir: PathBuf, db_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    wxdragon::main(move |app| {
        app.set_app_name("feedferry");
        app.set_app_display_name("FeedFerry");
        build_ui(data_dir, db_path);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Item;

    fn fixture_item() -> ItemWithFeed {
        ItemWithFeed {
            item: Item {
                id: 7,
                feed_id: 1,
                guid: "episode-7".to_string(),
                url: Some("https://example.com/posts/7".to_string()),
                title: "Episode Seven".to_string(),
                author: Some("Host".to_string()),
                summary: Some("A useful summary.".to_string()),
                content: Some("A useful summary.".to_string()),
                published: Some(1_735_689_600),
                updated: None,
                media_url: Some("https://example.com/episode-7.mp3".to_string()),
                media_type: Some("audio/mpeg".to_string()),
                media_length: Some(2048),
                duration_secs: Some(3671),
                is_read: false,
                is_favorite: true,
                downloaded_path: None,
                added_at: 1_735_689_600,
                last_position_secs: 61,
            },
            feed_title: "Example Podcast".to_string(),
            feed_url: "https://example.com/feed.xml".to_string(),
            feed_kind: FeedKind::Podcast,
        }
    }

    #[test]
    fn item_labels_are_verbose_for_screen_readers() {
        let label = readable_item_label(&fixture_item());
        assert!(label.contains("Episode Seven"));
        assert!(label.contains("unread"));
        assert!(label.contains("favorite"));
        assert!(label.contains("podcast episode"));
        assert!(label.contains("duration 1:01:11"));
    }

    #[test]
    fn detail_text_contains_article_and_media_urls() {
        let detail = item_detail_text(&fixture_item());
        assert!(detail.contains("Article URL: https://example.com/posts/7"));
        assert!(detail.contains("Media URL: https://example.com/episode-7.mp3"));
        assert!(detail.contains("Saved playback position: 1:01"));
    }

    #[test]
    fn announcement_sanitizer_removes_control_noise() {
        assert_eq!(
            sanitize_announcement("Hello\n\tworld\u{0007}"),
            "Hello world"
        );
    }

    #[test]
    fn update_summary_reports_success_counts() {
        let summary = UpdateSummary {
            fetched: 3,
            new_or_updated_items: 12,
            errors: Vec::new(),
        };

        assert_eq!(
            summary.message(),
            "Updated 3 feed(s); 12 new or changed item(s)."
        );
    }

    #[test]
    fn update_summary_reports_feed_errors() {
        let summary = UpdateSummary {
            fetched: 1,
            new_or_updated_items: 2,
            errors: vec!["first error".to_string(), "second error".to_string()],
        };

        assert_eq!(
            summary.message(),
            "Updated 1 feed(s); 2 new or changed item(s); 2 error(s): first error; second error."
        );
    }
}
