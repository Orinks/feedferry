use std::path::PathBuf;

use feedferry::{config, gui};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data_dir_override = std::env::var_os("FEEDFERRY_DATA_DIR").map(PathBuf::from);
    let data_dir = config::data_dir(data_dir_override.as_deref()).unwrap_or_else(|error| {
        eprintln!("FeedFerry could not prepare its data directory: {error}");
        std::process::exit(1);
    });
    let db_path = config::database_path(&data_dir);
    gui::run_native(data_dir, db_path)
}
