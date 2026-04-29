use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use feedferry::accessibility::{accessibility_help, Accessibility};
use feedferry::config;
use feedferry::database::Store;
use feedferry::downloader::Downloader;
use feedferry::feed::{parse_feed_bytes, FeedFetcher, FetchOutcome};
use feedferry::models::{Feed, FeedKind, FeedUpdateReport, ItemFilter};
use feedferry::opml::{export_opml, parse_opml};
use feedferry::output::{
    render_feeds, render_item_detail, render_items, render_queue, render_stats, OutputFormat,
};
use feedferry::player::{open_item_url, play_item};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Accessible command-line podcatcher and RSS/Atom/JSON Feed reader"
)]
struct Cli {
    #[arg(long, global = true, env = "FEEDFERRY_DATA_DIR")]
    data_dir: Option<PathBuf>,

    #[arg(long, global = true, env = "FEEDFERRY_SCREEN_READER")]
    screen_reader: bool,

    #[arg(long, global = true, env = "FEEDFERRY_SPEECH_COMMAND")]
    speech_command: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Subscribe to a feed URL.
    Add {
        url: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_enum, default_value = "mixed")]
        kind: KindArg,
        #[arg(long)]
        folder: Option<String>,
        #[arg(long)]
        no_update: bool,
    },

    /// Remove a subscription by id, URL, or exact title.
    Remove { selector: String },

    /// List subscriptions.
    Feeds {
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Refresh all feeds, or selected feeds by id, URL, or exact title.
    Update { selectors: Vec<String> },

    /// List stored articles and podcast episodes.
    Items {
        #[arg(long)]
        feed: Option<String>,
        #[arg(long)]
        unread: bool,
        #[arg(long)]
        podcasts: bool,
        #[arg(long)]
        news: bool,
        #[arg(long)]
        favorites: bool,
        #[arg(long)]
        search: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Print an item and mark it read unless --no-mark is used.
    Read {
        id: i64,
        #[arg(long)]
        no_mark: bool,
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Open an item URL in the operating system browser.
    Open { id: i64 },

    /// Mark one item, all items, or all items in a feed as read.
    MarkRead {
        target: String,
        #[arg(long)]
        feed: Option<String>,
    },

    /// Mark one item as unread.
    MarkUnread { id: i64 },

    /// Mark an item as a favorite.
    Star { id: i64 },

    /// Remove an item from favorites.
    Unstar { id: i64 },

    /// Add a podcast episode to the queue.
    Queue { id: i64 },

    /// List queued podcast episodes.
    QueueList {
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Remove an episode from the queue.
    Dequeue { id: i64 },

    /// Download a podcast episode by id, or the whole queue with target "queue".
    Download {
        #[arg(default_value = "queue")]
        target: String,
        #[arg(long)]
        dir: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },

    /// Play a downloaded episode or stream its media URL.
    Play {
        id: i64,
        #[arg(long)]
        player: Option<String>,
    },

    /// Import subscriptions from an OPML file.
    ImportOpml {
        path: PathBuf,
        #[arg(long)]
        update: bool,
    },

    /// Export subscriptions to an OPML file.
    ExportOpml { path: PathBuf },

    /// Search stored item titles, summaries, and content.
    Search {
        query: String,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Save playback position in seconds for an item.
    SetPosition { id: i64, seconds: i64 },

    /// Print library statistics.
    Stats {
        #[arg(long, value_enum, default_value = "text")]
        format: OutputFormat,
    },

    /// Print accessibility usage guidance.
    Accessibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum KindArg {
    News,
    Podcast,
    Mixed,
}

impl From<KindArg> for FeedKind {
    fn from(value: KindArg) -> Self {
        match value {
            KindArg::News => FeedKind::News,
            KindArg::Podcast => FeedKind::Podcast,
            KindArg::Mixed => FeedKind::Mixed,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    run(cli)
}

fn run(cli: Cli) -> Result<()> {
    let ax = Accessibility::new(cli.screen_reader, cli.speech_command.clone());

    if matches!(&cli.command, Commands::Accessibility) {
        println!("{}", accessibility_help());
        return Ok(());
    }

    let data_dir = config::data_dir(cli.data_dir.as_deref())?;
    let store = Store::open(config::database_path(&data_dir))?;

    match cli.command {
        Commands::Add {
            url,
            title,
            kind,
            folder,
            no_update,
        } => {
            let id = store.add_feed(&url, title.as_deref(), kind.into(), folder.as_deref())?;
            ax.status(format!("Subscribed feed {id}."))?;
            if !no_update {
                let fetcher = FeedFetcher::new()?;
                let feed = store
                    .feed_by_id(id)?
                    .context("feed disappeared after insert")?;
                let report = update_one_feed(&store, &fetcher, &feed, &ax)?;
                print_update_reports(&[report]);
            }
        }
        Commands::Remove { selector } => {
            let rows = store.remove_feed(&selector)?;
            println!("Removed {rows} feed(s).");
        }
        Commands::Feeds { format } => {
            let feeds = store.list_feeds()?;
            print!(
                "{}",
                render_feeds(&feeds, format, ax.should_use_linear_output())?
            );
        }
        Commands::Update { selectors } => {
            let reports = update_selected_feeds(&store, &selectors, &ax)?;
            print_update_reports(&reports);
        }
        Commands::Items {
            feed,
            unread,
            podcasts,
            news,
            favorites,
            search,
            limit,
            format,
        } => {
            let feed_id = match feed {
                Some(selector) => Some(store.require_feed(&selector)?.id),
                None => None,
            };
            let items = store.list_items(&ItemFilter {
                feed_id,
                unread_only: unread,
                podcasts_only: podcasts,
                news_only: news,
                favorites_only: favorites,
                search,
                limit,
            })?;
            print!(
                "{}",
                render_items(&items, format, ax.should_use_linear_output())?
            );
        }
        Commands::Read {
            id,
            no_mark,
            format,
        } => {
            if !no_mark {
                store.mark_item_read(id, true)?;
            }
            let item = store.require_item(id)?;
            print!("{}", render_item_detail(&item, format)?);
        }
        Commands::Open { id } => {
            let item = store.require_item(id)?;
            open_item_url(&item)?;
            ax.status(format!("Opened item {id}."))?;
        }
        Commands::MarkRead { target, feed } => {
            let rows = if target.eq_ignore_ascii_case("all") {
                let feed_id = match feed {
                    Some(selector) => Some(store.require_feed(&selector)?.id),
                    None => None,
                };
                store.mark_all_read(feed_id)?
            } else {
                let id = target.parse::<i64>().with_context(|| {
                    format!("target must be an item id or `all`, got `{target}`")
                })?;
                store.mark_item_read(id, true)?
            };
            println!("Marked {rows} item(s) read.");
        }
        Commands::MarkUnread { id } => {
            let rows = store.mark_item_read(id, false)?;
            println!("Marked {rows} item(s) unread.");
        }
        Commands::Star { id } => {
            let rows = store.set_favorite(id, true)?;
            println!("Starred {rows} item(s).");
        }
        Commands::Unstar { id } => {
            let rows = store.set_favorite(id, false)?;
            println!("Unstarred {rows} item(s).");
        }
        Commands::Queue { id } => {
            let item = store.require_item(id)?;
            if !item.item.is_podcast_episode() {
                return Err(anyhow!("item {id} has no media enclosure to queue"));
            }
            store.enqueue_item(id)?;
            println!("Queued item {id}.");
        }
        Commands::QueueList { format } => {
            let queue = store.list_queue()?;
            print!(
                "{}",
                render_queue(&queue, format, ax.should_use_linear_output())?
            );
        }
        Commands::Dequeue { id } => {
            let rows = store.remove_from_queue(id)?;
            println!("Removed {rows} queued item(s).");
        }
        Commands::Download { target, dir, force } => {
            let download_dir = match dir {
                Some(path) => path,
                None => config::default_download_dir(&data_dir)?,
            };
            let downloader = Downloader::new()?;
            if target.eq_ignore_ascii_case("queue") {
                let queue = store.list_queue()?;
                if queue.is_empty() {
                    println!("Queue is empty.");
                }
                for entry in queue {
                    let id = entry.item.item.id;
                    ax.status(format!("Downloading item {id}: {}", entry.item.item.title))?;
                    let path = downloader.download_item(&entry.item, &download_dir, force)?;
                    let path_string = path.to_string_lossy().to_string();
                    store.set_downloaded_path(id, &path_string)?;
                    println!("Downloaded {id}: {}", path.display());
                }
            } else {
                let id = target.parse::<i64>().with_context(|| {
                    format!("download target must be an item id or `queue`, got `{target}`")
                })?;
                let item = store.require_item(id)?;
                let path = downloader.download_item(&item, &download_dir, force)?;
                let path_string = path.to_string_lossy().to_string();
                store.set_downloaded_path(id, &path_string)?;
                println!("Downloaded {id}: {}", path.display());
            }
        }
        Commands::Play { id, player } => {
            let item = store.require_item(id)?;
            play_item(&item, player.as_deref())?;
            ax.status(format!("Started playback for item {id}."))?;
        }
        Commands::ImportOpml { path, update } => {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("could not read OPML file {}", path.display()))?;
            let subscriptions = parse_opml(&text)?;
            let mut ids = Vec::new();
            for sub in subscriptions {
                let id = store.add_feed(
                    &sub.xml_url,
                    Some(&sub.title),
                    sub.kind,
                    sub.folder.as_deref(),
                )?;
                ids.push(id);
            }
            println!("Imported {} subscription(s).", ids.len());
            if update {
                let fetcher = FeedFetcher::new()?;
                let mut reports = Vec::new();
                for id in ids {
                    if let Some(feed) = store.feed_by_id(id)? {
                        reports.push(update_one_feed(&store, &fetcher, &feed, &ax)?);
                    }
                }
                print_update_reports(&reports);
            }
        }
        Commands::ExportOpml { path } => {
            let feeds = store.list_feeds()?;
            let xml = export_opml(&feeds, "FeedFerry subscriptions");
            fs::write(&path, xml)
                .with_context(|| format!("could not write OPML file {}", path.display()))?;
            println!(
                "Exported {} subscription(s) to {}.",
                feeds.len(),
                path.display()
            );
        }
        Commands::Search {
            query,
            limit,
            format,
        } => {
            let items = store.search_items(&query, limit)?;
            print!(
                "{}",
                render_items(&items, format, ax.should_use_linear_output())?
            );
        }
        Commands::SetPosition { id, seconds } => {
            let rows = store.set_position(id, seconds)?;
            println!("Updated playback position for {rows} item(s).");
        }
        Commands::Stats { format } => {
            let stats = store.stats()?;
            print!("{}", render_stats(&stats, format)?);
        }
        Commands::Accessibility => unreachable!(),
    }

    Ok(())
}

fn update_selected_feeds(
    store: &Store,
    selectors: &[String],
    ax: &Accessibility,
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
    let mut reports = Vec::new();
    for feed in feeds {
        reports.push(update_one_feed(store, &fetcher, &feed, ax)?);
    }
    Ok(reports)
}

fn update_one_feed(
    store: &Store,
    fetcher: &FeedFetcher,
    feed: &Feed,
    ax: &Accessibility,
) -> Result<FeedUpdateReport> {
    ax.status(format!("Updating {}.", feed.title))?;
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
    match fetcher.fetch(
        &feed.url,
        feed.etag.as_deref(),
        feed.last_modified.as_deref(),
    )? {
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

fn print_update_reports(reports: &[FeedUpdateReport]) {
    if reports.is_empty() {
        println!("No feeds to update.");
        return;
    }
    for report in reports {
        if let Some(error) = &report.error {
            println!("{}: error: {}", report.title, error);
        } else if report.fetched {
            println!(
                "{}: fetched; {} item(s) inserted or updated.",
                report.title, report.new_or_updated_items
            );
        } else {
            println!("{}: not modified.", report.title);
        }
    }
}
