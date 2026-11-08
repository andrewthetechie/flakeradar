//! Brings one database up to the baseline schema.
//!
//! This is the migration path without the server: CI uses it to check that the
//! committed entities still match what `sea-orm-cli` generates from a migrated
//! database. It takes the URL as its only argument.
//!
//! ```text
//! cargo run -p flakeradar-db --example migrate -- postgresql://user:pass@host/db
//! ```

// Rust guideline compliant 2026-07-21

use std::error::Error;

use flakeradar_db::{DbSettings, connect, run_migrations};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = std::env::args()
        .nth(1)
        .ok_or("usage: migrate <database-url>")?;
    let db = connect(&url, DbSettings::default()).await?;
    run_migrations(&db).await?;
    Ok(())
}
