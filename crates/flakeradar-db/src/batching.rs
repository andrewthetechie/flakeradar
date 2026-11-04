//! Chunking for bulk statements and `IN (...)` lists.
//!
//! PostgreSQL accepts at most 32,767 bind parameters in one statement, so every
//! bulk insert, update and `IN` list is split into chunks of [`CHUNK`] rows.

// Rust guideline compliant 2026-07-21

/// Number of rows in one chunk of a bulk statement.
///
/// 1000 rows stay far below PostgreSQL's 32,767-bind-parameter limit for the
/// widest row in the schema, while keeping each statement large enough that the
/// round-trip cost stays small. It is the frozen Python server's `CHUNK`, so a
/// bulk statement issues the same number of round trips.
pub const CHUNK: usize = 1000;

/// Splits `items` into chunks of at most `CHUNK` items.
///
/// # Examples
///
/// ```
/// # use flakeradar_db::chunks;
/// assert_eq!(chunks(&[1, 2, 3]).count(), 1);
/// ```
pub fn chunks<T>(items: &[T]) -> impl Iterator<Item = &[T]> {
    items.chunks(CHUNK)
}
