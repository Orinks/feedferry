# FeedFerry

FeedFerry is a Rust desktop podcatcher and RSS/Atom/JSON Feed news reader with a graphical interface and screen-reader-oriented workflows.

The default binary is now a GUI app. The original command-line interface is still included as `feedferry-cli` for scripting and automation.

## Features

- Native wxWidgets/wxDragon feed library with feed list, item list, reader pane, podcast queue, and status output.
- RSS, Atom, and JSON Feed parsing through `feed-rs`.
- Podcast enclosure detection, queueing, downloading, playback launching, and saved playback position.
- SQLite local database for feeds, items, read/unread state, favorites, queue, downloaded file paths, and feed cache metadata.
- OPML import and export.
- Screen-reader-oriented verbose item labels, explicit status output, and a plain selectable reader text pane.
- Optional speech-command bridge for tools such as `spd-say`, `say`, or a custom script.
- Keyboard shortcuts for common actions.

## Accessibility model

FeedFerry uses wxDragon/wxWidgets native controls instead of a custom-painted immediate-mode UI. The reader pane is intentionally plain text and selectable, and the item list uses verbose labels such as title, read state, favorite state, feed, date, item id, and podcast duration.

Status changes are shown in native text/status controls. FeedFerry still preserves the screen-reader and speech-command preferences for compatibility with existing settings, and the CLI companion remains the best surface for fully linear output.

Examples:

```text
spd-say
say
```

## Build requirements

Install Rust using `rustup`, then update to a recent stable toolchain:

```bash
rustup update stable
```

This project targets Rust 1.92 or newer. The GUI uses `wxdragon`, which builds wxWidgets through CMake.

Windows GUI builds require CMake, Ninja, LLVM/libclang, and Visual Studio Build Tools with the Windows SDK. The wxDragon build downloads and builds wxWidgets during the first compile.

Linux GUI builds require wxWidgets/GTK build prerequisites. On Debian/Ubuntu, start with:

```bash
sudo apt install build-essential cmake ninja-build libclang-dev pkg-config libgtk-3-dev libpng-dev libjpeg-dev libgl1-mesa-dev libglu1-mesa-dev libxkbcommon-dev libexpat1-dev libtiff-dev
```

For podcast playback, install a media player such as VLC or mpv and set the player command in Settings.

## Build and run the GUI

```bash
cargo test --all-targets
cargo run --release
```

To store data somewhere specific:

```bash
FEEDFERRY_DATA_DIR="$PWD/demo-data" cargo run --release
```

## First-use walkthrough

1. Launch the app with `cargo run --release`.
2. Choose **Add feed**.
3. Paste a feed URL.
4. Select an item in the center list.
5. Read it in the right pane, or use **Open**.
6. For podcast episodes, use **Queue**, **Download**, or **Play**.

## OPML

Use the toolbar buttons:

- **Import OPML** to import subscriptions.
- **Export OPML** to save a subscription backup.

A sample file is included at:

```text
examples/subscriptions.opml
```

## Command-line companion

The GUI is the main app, but the CLI is still available:

```bash
cargo run --bin feedferry-cli -- --help
cargo run --bin feedferry-cli -- add https://blog.rust-lang.org/feed.xml --kind news
cargo run --bin feedferry-cli -- update
cargo run --bin feedferry-cli -- items --unread
```

CLI screen-reader mode is still available:

```bash
cargo run --bin feedferry-cli -- --screen-reader items --unread
```

## Tests

The test suite covers:

- RSS podcast enclosure parsing.
- Atom/news parsing.
- SQLite feed/item/queue flow.
- OPML import/export round-tripping.
- Screen-reader-style GUI item labels and detail text.
- Update report formatting.
- Downloader filename sanitization.
- Accessibility flag behavior.

Run:

```bash
cargo test --all-targets
```

## Project layout

```text
src/main.rs                 GUI entry point
src/gui.rs                  wxDragon/wxWidgets desktop interface
src/bin/feedferry-cli.rs    CLI companion
src/database.rs             SQLite store
src/feed.rs                 RSS/Atom/JSON Feed parsing and fetching
src/downloader.rs           Podcast download logic
src/player.rs               Open/play integration
src/opml.rs                 OPML import/export
src/sync.rs                 Feed refresh orchestration
tests/library_flow.rs       Integration tests
```

## Notes

The project is intentionally local-first. It does not include telemetry, account sync, or a remote service.
