//! The bounds the REST routes and the MCP tools both enforce.
//!
//! Both front doors read the same Score history and the same leaderboards, so a
//! window or a page size one accepts and the other refuses would answer two
//! clients differently about one database. Each bound below is stated once and
//! read by both; a bound only one surface has stays with that surface.

/// The largest page, or listing, either surface serves: REST `page_size` and
/// the MCP listing tools' `limit`.
///
/// One page has to be readable in either front door, and 100 rows is the point
/// where a leaderboard answer stays a small body: every row carries the identity,
/// the score, the tier and the trend, and a page is read once per request. Both
/// surfaces enforce it, so the same question about one database cannot be answered
/// a hundred rows differently; the bound is in the published 422 and in the MCP
/// tools' refusal, and `tests/parity` replays both. Raising it grows the largest
/// answer either surface builds; lowering it refuses a page the frontend or an agent
/// already asks for. `routes/reads.rs` bounds `page_size` and the `page` offset with
/// it, `mcp` refuses a larger `limit` with it, and `defaults::DEFAULT_PAGE_SIZE`
/// is what a request that names none gets.
pub const LIST_LIMIT_MAX: i64 = 100;

/// The widest "became flaky recently" window either surface filters on, in days.
///
/// Both read the same Score history, so a wider window on one side would be a
/// different question about the same rows.
pub const NEW_WITHIN_DAYS_MAX: i64 = 90;
