//! The Test/Job axis stated once: the scored table's name, its alias, its
//! `SELECT` list, its joins, the Score-history table and owner column, and the
//! per-entity words a read statement splices into its own SQL.
//!
//! [`EntityKind`] has two variants, so [`entity`] answers each with one
//! descriptor. A third entity means one new `entity()` arm plus its descriptor:
//! the statements that read it need no new arm, because they ask the descriptor
//! for every per-entity word they name.
//!
//! Every string is the text the statements serve, copied as it stood, so no
//! query changes shape by being built here. Two fields hold a join because one
//! string cannot be both spellings: `from` carries the `FROM` keyword, which the
//! leaderboard splices whole, while `tally_from` omits it, because the count
//! statements write `FROM` before interpolating it.

// Rust guideline compliant 2026-07-21

use flakeradar_core::model::EntityKind;

/// What the read statements need to know about one scored entity.
pub(crate) struct Entity {
    /// The entity the descriptor is about.
    pub(crate) kind: EntityKind,
    /// The scored table's name, as the migration creates it.
    pub(crate) table: &'static str,
    /// The scored table's alias in the statements that read it.
    pub(crate) alias: &'static str,
    /// The row shape: the entity's columns, then the names of its parents.
    pub(crate) select: &'static str,
    /// The scored table joined to its parents, `FROM` keyword included.
    pub(crate) from: &'static str,
    /// The same join without the `FROM` keyword, for the headline counts.
    pub(crate) tally_from: &'static str,
    /// The Issue-link columns `locked_entity_row` adds, with their leading
    /// comma and the space that precedes the `FROM` that follows them.
    pub(crate) locked_select: &'static str,
    /// The Score history joined to the scored table and its parents.
    pub(crate) history_from: &'static str,
    /// The Score-history table that holds the entity's own history.
    pub(crate) history_table: &'static str,
    /// The column naming the owner: the Test's or the Job's id.
    pub(crate) owner_column: &'static str,
    /// The entity's GitHub issue column.
    pub(crate) issue_column: &'static str,
    /// The search condition, given the pattern bind's placeholder.
    pub(crate) search: fn(&str) -> String,
    /// The column an exact-name lookup narrows by: a Test's classname, a Job's
    /// Pipeline name.
    pub(crate) lookup_column: &'static str,
    /// How the entity's queries are named in the error a statement that
    /// answered nothing gives: nothing for a Test, `job ` for a Job.
    pub(crate) prefix: &'static str,
    /// Whether only a GitHub Pipeline's entity can hold an Issue link (ADR 0008).
    pub(crate) provider_only: bool,
}

/// The case-insensitive literal substring search over a Test's identity.
///
/// One pattern bind covers every column, so `pattern` is repeated.
fn test_search(pattern: &str) -> String {
    format!(
        "(t.name ILIKE {pattern} ESCAPE '\\' \
         OR t.classname ILIKE {pattern} ESCAPE '\\' \
         OR t.file ILIKE {pattern} ESCAPE '\\')"
    )
}

/// The case-insensitive literal substring search over a Job's name and Pipeline.
fn job_search(pattern: &str) -> String {
    format!("(j.name ILIKE {pattern} ESCAPE '\\' OR p.name ILIKE {pattern} ESCAPE '\\')")
}

/// The Tests leaderboard's and headline counts' descriptor.
const TEST: Entity = Entity {
    kind: EntityKind::Test,
    table: "test_cases",
    alias: "t",
    select: "SELECT t.*, p.name AS project_name, r.name AS repo_name",
    from: "FROM test_cases t \
     JOIN projects p ON p.id = t.project_id \
     JOIN repos r ON r.id = p.repo_id",
    tally_from: "test_cases t \
        JOIN projects p ON p.id = t.project_id \
        JOIN repos r ON r.id = p.repo_id",
    locked_select: ", t.github_issue_checked_at AS checked_at, \
        t.github_issue_closed AS closed, \
        t.github_issue_fixed_noted_at AS fixed_noted_at ",
    history_from: "test_score_history h \
        JOIN test_cases t ON t.id = h.test_case_id \
        JOIN projects p ON p.id = t.project_id \
        JOIN repos r ON r.id = p.repo_id",
    history_table: "test_score_history",
    owner_column: "test_case_id",
    issue_column: "t.github_issue_number",
    search: test_search,
    lookup_column: "t.classname",
    prefix: "",
    provider_only: false,
};

/// The Jobs leaderboard's and headline counts' descriptor.
const JOB: Entity = Entity {
    kind: EntityKind::Job,
    table: "jobs",
    alias: "j",
    select: "SELECT j.*, p.name AS pipeline_name, p.provider AS provider, \
     p.repo_id AS repo_id, r.name AS repo_name",
    from: "FROM jobs j \
     JOIN pipelines p ON p.id = j.pipeline_id \
     JOIN repos r ON r.id = p.repo_id",
    tally_from: "jobs j \
        JOIN pipelines p ON p.id = j.pipeline_id \
        JOIN repos r ON r.id = p.repo_id",
    locked_select: ", j.github_issue_checked_at AS checked_at, \
        j.github_issue_closed AS closed, \
        j.github_issue_fixed_noted_at AS fixed_noted_at ",
    history_from: "job_score_history h \
        JOIN jobs j ON j.id = h.job_id \
        JOIN pipelines p ON p.id = j.pipeline_id \
        JOIN repos r ON r.id = p.repo_id",
    history_table: "job_score_history",
    owner_column: "job_id",
    issue_column: "j.github_issue_number",
    search: job_search,
    lookup_column: "p.name",
    prefix: "job ",
    provider_only: true,
};

/// The descriptor `kind` is scored by.
#[must_use]
pub(crate) const fn entity(kind: EntityKind) -> &'static Entity {
    match kind {
        EntityKind::Test => &TEST,
        EntityKind::Job => &JOB,
    }
}

#[cfg(test)]
mod tests {
    //! The descriptor's own spellings, without a database.

    use super::{EntityKind, entity};

    /// The two `FROM` fields are one join spelled two ways, and the join text
    /// is the single-spaced sentence the statements serve.
    #[test]
    fn the_join_fields_are_one_spelling_with_the_keyword_and_one_without() {
        let tests = entity(EntityKind::Test);
        assert_eq!(
            tests.from,
            "FROM test_cases t JOIN projects p ON p.id = t.project_id \
             JOIN repos r ON r.id = p.repo_id"
        );
        assert!(tests.from.starts_with("FROM test_cases t "));
        assert_eq!(
            tests.tally_from,
            "test_cases t JOIN projects p ON p.id = t.project_id \
             JOIN repos r ON r.id = p.repo_id"
        );
        assert!(tests.tally_from.starts_with("test_cases t"));
        assert!(!tests.tally_from.contains("FROM"));
    }

    /// The Job twin names the Job table, its Pipeline and its Repo.
    #[test]
    fn the_job_arm_names_the_job_table_and_its_parents() {
        let jobs = entity(EntityKind::Job);
        assert_eq!(jobs.table, "jobs");
        assert_eq!(
            jobs.from,
            "FROM jobs j JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos r ON r.id = p.repo_id"
        );
        assert_eq!(
            jobs.tally_from,
            "jobs j JOIN pipelines p ON p.id = j.pipeline_id \
             JOIN repos r ON r.id = p.repo_id"
        );
    }

    /// The two descriptors name the scored tables exactly as the migration
    /// creates them, which is what the impact lateral's Job arm interpolates.
    #[test]
    fn each_descriptor_names_its_own_scored_table() {
        assert_eq!(entity(EntityKind::Test).table, "test_cases");
        assert_eq!(entity(EntityKind::Job).table, "jobs");
    }

    /// The names the acknowledge, link and Score-history modules read: the
    /// history tables and owner columns exactly as the migrations create them,
    /// the narrowing column the exact-match lookups bind, and the one per-target
    /// rule the link engine answers from the descriptor.
    #[test]
    fn each_descriptor_names_its_history_table_owner_and_provider_rule() {
        assert_eq!(entity(EntityKind::Test).history_table, "test_score_history");
        assert_eq!(entity(EntityKind::Job).history_table, "job_score_history");
        assert_eq!(entity(EntityKind::Test).owner_column, "test_case_id");
        assert_eq!(entity(EntityKind::Job).owner_column, "job_id");
        assert_eq!(entity(EntityKind::Test).lookup_column, "t.classname");
        assert_eq!(entity(EntityKind::Job).lookup_column, "p.name");
        assert!(entity(EntityKind::Job).provider_only);
        assert!(!entity(EntityKind::Test).provider_only);
    }
}
