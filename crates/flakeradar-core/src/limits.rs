//! The bounds the REST routes and the MCP tools both enforce.
//!
//! Both front doors read the same Score history and the same leaderboards, so a
//! window or a page size one accepts and the other refuses would answer two
//! clients differently about one database. Each bound below is stated once and
//! read by both; a bound only one surface has stays with that surface.

/// The largest page, or listing, either surface serves: REST `page_size` and
/// the MCP listing tools' `limit`.
pub const LIST_LIMIT_MAX: i64 = 100;

/// The widest "became flaky recently" window either surface filters on, in days.
///
/// Both read the same Score history, so a wider window on one side would be a
/// different question about the same rows.
pub const NEW_WITHIN_DAYS_MAX: i64 = 90;

#[cfg(test)]
mod tests {
    //! The bounds are the values the two surfaces' documents state.

    use super::{LIST_LIMIT_MAX, NEW_WITHIN_DAYS_MAX};

    #[test]
    fn the_shared_bounds_are_the_documented_numbers() {
        assert_eq!(LIST_LIMIT_MAX, 100);
        assert_eq!(NEW_WITHIN_DAYS_MAX, 90);
    }
}
