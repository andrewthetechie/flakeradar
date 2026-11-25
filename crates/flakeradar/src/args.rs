//! The argument rules both front doors ask, and the refusals they answer with.
//!
//! The REST routes and the MCP tools take the same five questions from a caller:
//! is this a sort order, a Failure category, a limit or a page in range, a
//! `new_within_days` window, a Repo/Project scope. Both doors read one database,
//! so one answer per rule is the point of this module: each rule states its
//! bound or its spelling list once, reading the shared constant behind it, and
//! answers a refusal that names the field and carries why it was refused. The
//! door then places that refusal in the error shape it already publishes, which
//! is why the refusals are per-reason structs rather than one enum: a door maps
//! only the refusal its checker can actually receive.

// Rust guideline compliant 2026-07-21

use std::fmt;
use std::ops::RangeInclusive;

use flakeradar_core::classify::FailureCategory;
use flakeradar_core::identity::{normalize_project, normalize_repo};
use flakeradar_core::limits::{LIST_LIMIT_MAX, NEW_WITHIN_DAYS_MAX};
use flakeradar_db::reads::{MAX_EXECUTIONS_LIMIT, Scope, SortKey};

/// The largest `page` whose `OFFSET` fits an `i64` at [`LIST_LIMIT_MAX`].
///
/// Only the bottom of the range was bounded before, so a page past this one
/// wrapped in release builds and panicked in development ones. A page real
/// clients ask for is a handful of digits.
pub const PAGE_MAX: i64 = i64::MAX / LIST_LIMIT_MAX;

/// A number outside the inclusive range its rule allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeRefusal {
    /// The argument the rule refused, as the caller spelled it.
    pub field: &'static str,
    /// The number the caller sent.
    pub value: i64,
    /// The smallest number the rule takes.
    pub min: i64,
    /// The largest number the rule takes.
    pub max: i64,
}

/// A name outside the spellings its rule accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRefusal {
    /// The argument the rule refused, as the caller spelled it.
    pub field: &'static str,
    /// The name the caller sent.
    pub value: String,
    /// Every accepted spelling, in the order the doors list them.
    pub options: Vec<&'static str>,
}

/// A Repo/Project pair that cannot build a [`Scope`].
///
/// A door prints a [`ScopeRefusal`] as it prints the identity or scope error it
/// refuses today, because the rule answers with that very sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRefusal {
    /// The argument the rule refused: `repo` or `project`.
    pub field: &'static str,
    /// The rule's own sentence, which is what both doors publish.
    pub message: String,
}

impl fmt::Display for ScopeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The page a leaderboard or listing asks for.
///
/// # Errors
///
/// Returns [`RangeRefusal`] when the page is below 1 or past [`PAGE_MAX`].
pub fn page(page: i64) -> Result<(), RangeRefusal> {
    accepted("page", page, 1..=PAGE_MAX)
}

/// The number of rows one dashboard page carries.
///
/// # Errors
///
/// Returns [`RangeRefusal`] when the size is below 1 or past [`LIST_LIMIT_MAX`].
pub fn page_size(size: i64) -> Result<(), RangeRefusal> {
    accepted("page_size", size, 1..=LIST_LIMIT_MAX)
}

/// The number of rows an MCP listing tool returns.
///
/// The same bound as [`page_size`], because a listing either surface serves has
/// to be readable in one answer: an MCP tool pages by refusing rather than
/// cutting a page short.
///
/// # Errors
///
/// Returns [`RangeRefusal`] when the limit is below 1 or past [`LIST_LIMIT_MAX`].
pub fn listing_limit(limit: i64) -> Result<(), RangeRefusal> {
    accepted("limit", limit, 1..=LIST_LIMIT_MAX)
}

/// The number of Executions one detail answer carries.
///
/// Deliberately a wider bound than a listing's: a detail route returns a run of
/// history whose rows each carry a message and details, and only that route asks
/// for one.
///
/// # Errors
///
/// Returns [`RangeRefusal`] when the limit is below 1 or past
/// [`MAX_EXECUTIONS_LIMIT`].
pub fn executions_limit(limit: i64) -> Result<(), RangeRefusal> {
    accepted("limit", limit, 1..=MAX_EXECUTIONS_LIMIT)
}

/// The "became flaky recently" window, in days.
///
/// Both doors read the same Score history, so the window is one rule: a wider
/// window on one side would be a different question about the same rows.
///
/// # Errors
///
/// Returns [`RangeRefusal`] when the window is below 1 or past
/// [`NEW_WITHIN_DAYS_MAX`].
pub fn new_within_days(days: i64) -> Result<(), RangeRefusal> {
    accepted("new_within_days", days, 1..=NEW_WITHIN_DAYS_MAX)
}

/// The sort order a `sort` argument names.
///
/// # Errors
///
/// Returns [`NameRefusal`] naming every order [`SortKey`] accepts.
pub fn sort(sort: &str) -> Result<SortKey, NameRefusal> {
    SortKey::parse(sort).ok_or_else(|| refused_name("sort", sort, SortKey::SPELLINGS.to_vec()))
}

/// The Failure category a `category` argument names.
///
/// # Errors
///
/// Returns [`NameRefusal`] naming every category [`FailureCategory`] reports.
pub fn category(name: &str) -> Result<FailureCategory, NameRefusal> {
    FailureCategory::from_wire(name).ok_or_else(|| refused_name("category", name, category_names()))
}

/// The scope a named Repo and a named Project build, normalizing both.
///
/// Either name may be absent, which is how a REST route asks for every Repo while
/// an MCP tool always names one.
///
/// # Errors
///
/// Returns [`ScopeRefusal`] when a name is not a valid Repo or Project, or when a
/// Project is named without the Repo it is unique within.
pub fn scope(repo: Option<&str>, project: Option<&str>) -> Result<Scope, ScopeRefusal> {
    let repo = match repo {
        Some(raw) => Some(normalize_repo(raw).map_err(|error| refused_scope("repo", &error))?),
        None => None,
    };
    let project = match project {
        Some(raw) => {
            Some(normalize_project(Some(raw)).map_err(|error| refused_scope("project", &error))?)
        }
        None => None,
    };
    Scope::new(repo, project).map_err(|error| refused_scope("project", &error))
}

/// `value` when it falls inside `bounds`, refusing it by name otherwise.
fn accepted(
    field: &'static str,
    value: i64,
    bounds: RangeInclusive<i64>,
) -> Result<(), RangeRefusal> {
    if bounds.contains(&value) {
        Ok(())
    } else {
        Err(RangeRefusal {
            field,
            value,
            min: *bounds.start(),
            max: *bounds.end(),
        })
    }
}

/// A refusal naming the accepted spellings of `field`.
fn refused_name(field: &'static str, value: &str, options: Vec<&'static str>) -> NameRefusal {
    NameRefusal {
        field,
        value: value.to_owned(),
        options,
    }
}

/// The categories, in the order both doors list them.
fn category_names() -> Vec<&'static str> {
    FailureCategory::ALL
        .iter()
        .map(|category| category.as_str())
        .collect()
}

/// A refusal carrying the sentence the identity or scope rule answers with, which
/// is what both doors publish for that rejection today.
fn refused_scope(field: &'static str, error: &impl fmt::Display) -> ScopeRefusal {
    ScopeRefusal {
        field,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    //! The rules answer about a number, a name or a pair of names, so no test
    //! here needs a database.

    use super::{
        PAGE_MAX, RangeRefusal, category, executions_limit, listing_limit, new_within_days, page,
        page_size, scope, sort,
    };
    use flakeradar_core::classify::FailureCategory;
    use flakeradar_core::limits::{LIST_LIMIT_MAX, NEW_WITHIN_DAYS_MAX};
    use flakeradar_db::reads::{MAX_EXECUTIONS_LIMIT, ScopeError, SortKey};

    #[test]
    fn the_window_rule_refuses_everything_outside_the_shared_bound() {
        assert_eq!(
            new_within_days(0),
            Err(RangeRefusal {
                field: "new_within_days",
                value: 0,
                min: 1,
                max: NEW_WITHIN_DAYS_MAX,
            })
        );
        assert_eq!(
            new_within_days(NEW_WITHIN_DAYS_MAX + 1).err(),
            Some(RangeRefusal {
                field: "new_within_days",
                value: NEW_WITHIN_DAYS_MAX + 1,
                min: 1,
                max: NEW_WITHIN_DAYS_MAX,
            }),
            "a day past the bound answers the same refusal as a day before it"
        );
        assert_eq!(new_within_days(1), Ok(()));
        assert_eq!(new_within_days(NEW_WITHIN_DAYS_MAX), Ok(()));
    }

    #[test]
    fn the_paging_rule_refuses_a_page_whose_offset_does_not_fit() {
        assert_eq!(
            page(0),
            Err(RangeRefusal {
                field: "page",
                value: 0,
                min: 1,
                max: PAGE_MAX,
            })
        );
        assert_eq!(page(PAGE_MAX), Ok(()));
        assert_eq!(
            page(PAGE_MAX + 1).err(),
            Some(RangeRefusal {
                field: "page",
                value: PAGE_MAX + 1,
                min: 1,
                max: PAGE_MAX,
            }),
            "an OFFSET past this page would overflow the i64 it multiplies into"
        );
    }

    #[test]
    fn the_page_size_rule_stops_at_the_shared_listing_bound() {
        assert_eq!(page_size(1), Ok(()));
        assert_eq!(page_size(LIST_LIMIT_MAX), Ok(()));
        assert_eq!(
            page_size(LIST_LIMIT_MAX + 1).err(),
            Some(RangeRefusal {
                field: "page_size",
                value: LIST_LIMIT_MAX + 1,
                min: 1,
                max: LIST_LIMIT_MAX,
            })
        );
    }

    #[test]
    fn the_two_limit_rules_keep_their_own_bounds() {
        assert_eq!(
            listing_limit(0),
            Err(RangeRefusal {
                field: "limit",
                value: 0,
                min: 1,
                max: LIST_LIMIT_MAX,
            })
        );
        assert_eq!(listing_limit(LIST_LIMIT_MAX), Ok(()));
        assert_eq!(
            executions_limit(LIST_LIMIT_MAX + 1),
            Ok(()),
            "a detail answer carries more rows than a listing's page"
        );
        assert_eq!(executions_limit(MAX_EXECUTIONS_LIMIT), Ok(()));
        assert_eq!(
            executions_limit(MAX_EXECUTIONS_LIMIT + 1).err(),
            Some(RangeRefusal {
                field: "limit",
                value: MAX_EXECUTIONS_LIMIT + 1,
                min: 1,
                max: MAX_EXECUTIONS_LIMIT,
            })
        );
    }

    #[test]
    fn a_name_rule_answers_the_spellings_it_publishes() {
        assert_eq!(sort("proven"), Ok(SortKey::Proven));
        let refusal = sort("bogus").expect_err("bogus is no sort order");
        assert_eq!(refusal.field, "sort");
        assert_eq!(refusal.value, "bogus");
        assert_eq!(
            refusal.options,
            SortKey::SPELLINGS.to_vec(),
            "the refusal carries every order, in the order the doors print them"
        );
        assert!(
            refusal
                .options
                .iter()
                .all(|spelling| sort(spelling).is_ok()),
            "and each one the refusal offers is one the rule takes"
        );

        assert_eq!(category("timing"), Ok(FailureCategory::Timing));
        let refusal = category("bogus").expect_err("bogus is no category");
        assert_eq!(refusal.field, "category");
        assert_eq!(refusal.value, "bogus");
        assert_eq!(
            refusal.options,
            FailureCategory::ALL
                .iter()
                .map(|category| category.as_str())
                .collect::<Vec<_>>(),
            "the refusal carries every category, in the order the 422 lists them"
        );
    }

    #[test]
    fn the_scope_rule_normalizes_both_names() {
        let built = scope(Some(" Acme/App "), Some(" Backend ")).expect("a valid pair builds");
        assert_eq!(
            (built.repo(), built.project()),
            (Some("acme/app"), Some("backend"))
        );
        let every_repo = scope(None, None).expect("no name is every Repo");
        assert_eq!(every_repo.project(), None);
    }

    #[test]
    fn a_refused_scope_names_the_rule_that_rejected_it() {
        let refusal = scope(None, Some("backend")).expect_err("a Project needs its Repo");
        assert_eq!(refusal.field, "project");
        assert_eq!(
            refusal.to_string(),
            ScopeError::ProjectWithoutRepo.to_string()
        );

        let refusal = scope(Some("acme"), None).expect_err("a Repo needs a slash");
        assert_eq!(refusal.field, "repo");
        assert_eq!(
            refusal.to_string(),
            "repo must look like 'owner/name', got 'acme'"
        );
    }
}
