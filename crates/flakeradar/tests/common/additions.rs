//! What this server answers that the recorded corpus never did, and the form the
//! list of those additions must take.
//!
//! `tests/parity/additions.json` lists the database columns, response fields,
//! routes and query parameters added after the goldens were recorded. The replay
//! reads it through [`Additions`] and ignores exactly what it lists; every
//! recorded value is still compared. [`Additions::problems`] reports each entry
//! that does not match the form its list documents.

// Rust guideline compliant 2026-07-21

use serde::Deserialize;

use super::allowances::root;

/// Every addition the parity replay is told to ignore.
///
/// The four lists are the four kinds of addition, each spelled in the form
/// `tests/parity/README.md` documents: `table.column`, `Schema.field`,
/// `METHOD /path` and `METHOD /path name`.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Additions {
    /// Added database columns, each spelled `table.column`.
    pub(crate) columns: Vec<String>,
    /// Added response fields, each spelled `Schema.field` with the OpenAPI
    /// component schema name.
    pub(crate) fields: Vec<String>,
    /// Added routes, each spelled `METHOD /path` with the OpenAPI path template.
    pub(crate) paths: Vec<String>,
    /// Added query parameters, each spelled `METHOD /path name`.
    pub(crate) query_params: Vec<String>,
}

impl Additions {
    /// Reads `tests/parity/additions.json`.
    ///
    /// # Panics
    ///
    /// Panics when the fixture cannot be read or is not the documented shape: a
    /// test that cannot read its own contract has failed.
    pub(crate) fn load() -> Self {
        let path = root().join("additions.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()))
    }

    /// Whether `table.column` names an added database column.
    pub(crate) fn column(&self, table: &str, column: &str) -> bool {
        names_dotted(&self.columns, table, column)
    }

    /// Whether `schema.field` names an added response field.
    pub(crate) fn field(&self, schema: &str, field: &str) -> bool {
        names_dotted(&self.fields, schema, field)
    }

    /// Whether `name` is the field part of any listed `Schema.field`.
    ///
    /// The replay sees untyped JSON, so it matches the bare field name against
    /// the last part of each listed `Schema.field`, not against a schema.
    pub(crate) fn field_name(&self, name: &str) -> bool {
        let dotted = format!(".{name}");
        self.fields.iter().any(|entry| entry.ends_with(&dotted))
    }

    /// Whether `METHOD /path` names an added route, the method compared without
    /// regard to case.
    pub(crate) fn path(&self, method: &str, path: &str) -> bool {
        self.paths.iter().any(|entry| {
            two_parts(entry).is_some_and(|(listed, listed_path)| {
                listed.eq_ignore_ascii_case(method) && listed_path == path
            })
        })
    }

    /// Whether `METHOD /path name` names an added query parameter, the method
    /// compared without regard to case.
    pub(crate) fn query_param(&self, method: &str, path: &str, name: &str) -> bool {
        self.query_params.iter().any(|entry| {
            three_parts(entry).is_some_and(|(listed, listed_path, listed_name)| {
                listed.eq_ignore_ascii_case(method) && listed_path == path && listed_name == name
            })
        })
    }

    /// Every entry that does not match the form of its list, each naming the
    /// entry so a reader knows which line of the fixture to fix.
    pub(crate) fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        report(
            &mut problems,
            "columns",
            &self.columns,
            is_dotted,
            "`table.column`",
        );
        report(
            &mut problems,
            "fields",
            &self.fields,
            is_dotted,
            "`Schema.field`",
        );
        report(
            &mut problems,
            "paths",
            &self.paths,
            is_route,
            "`METHOD /path`",
        );
        report(
            &mut problems,
            "query_params",
            &self.query_params,
            is_route_and_name,
            "`METHOD /path name`",
        );
        problems
    }
}

/// Adds a problem for each entry of `list` that does not match `form`.
fn report(
    problems: &mut Vec<String>,
    list: &str,
    entries: &[String],
    matches_form: fn(&str) -> bool,
    form: &str,
) {
    problems.extend(
        entries
            .iter()
            .filter(|entry| !matches_form(entry))
            .map(|entry| format!("{list}: {entry} is not a {form} entry")),
    );
}

/// Whether `entries` holds exactly `{first}.{second}`.
fn names_dotted(entries: &[String], first: &str, second: &str) -> bool {
    entries
        .iter()
        .any(|entry| entry.split_once('.') == Some((first, second)))
}

/// The two space-separated parts of `entry`, when it holds exactly two.
fn two_parts(entry: &str) -> Option<(&str, &str)> {
    let mut parts = entry.split(' ');
    let first = parts.next()?;
    let second = parts.next()?;
    parts.next().is_none().then_some((first, second))
}

/// The three space-separated parts of `entry`, when it holds exactly three.
fn three_parts(entry: &str) -> Option<(&str, &str, &str)> {
    let mut parts = entry.split(' ');
    let first = parts.next()?;
    let second = parts.next()?;
    let third = parts.next()?;
    parts.next().is_none().then_some((first, second, third))
}

/// Whether `text` is a non-empty, all-uppercase ASCII alphabetic HTTP method.
fn is_method(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_uppercase())
}

/// Whether `entry` is one route in the form `paths` documents.
fn is_route(entry: &str) -> bool {
    two_parts(entry).is_some_and(|(method, path)| is_method(method) && path.starts_with('/'))
}

/// Whether `entry` is one route and parameter in the form `query_params`
/// documents.
fn is_route_and_name(entry: &str) -> bool {
    three_parts(entry).is_some_and(|(method, path, name)| {
        is_method(method) && path.starts_with('/') && !name.is_empty()
    })
}

/// Whether `entry` is two non-empty parts separated by one dot.
fn is_dotted(entry: &str) -> bool {
    entry.split_once('.').is_some_and(|(first, second)| {
        !first.is_empty() && !second.is_empty() && !second.contains('.')
    })
}

#[cfg(test)]
mod form {
    //! The shipped list, its form check, and the predicates the replay calls.

    use super::Additions;
    use crate::common::allowances::root;

    /// A fixture whose `columns` holds only `entry`.
    fn columns(entry: &str) -> Additions {
        Additions {
            columns: vec![entry.to_owned()],
            ..Default::default()
        }
    }

    /// The single problem a fixture reports, checked to name `entry`.
    fn only_problem(additions: &Additions, entry: &str) {
        let problems = additions.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains(entry),
            "{} does not name {entry}",
            problems[0]
        );
    }

    /// The fixture as it ships: exactly four empty lists, and nothing to report.
    #[test]
    fn the_shipped_fixture_holds_four_empty_lists_and_reports_nothing() {
        let path = root().join("additions.json");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
        let document: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()));
        assert_eq!(
            document,
            serde_json::json!({"columns": [], "fields": [], "paths": [], "query_params": []}),
            "{}",
            path.display()
        );
        let problems = Additions::load().problems();
        assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    }

    /// The four JSON lists land in the four fields, in the forms the fixture
    /// documents.
    #[test]
    fn each_list_of_the_fixture_deserializes_into_its_field() {
        let document = r#"{"columns": ["test_runs.infra_suspect"],
            "fields": ["CiRunTestFailureOut.infra_suspect"],
            "paths": ["POST /api/x"],
            "query_params": ["GET /api/tests acknowledged"]}"#;
        let additions = match serde_json::from_str::<Additions>(document) {
            Ok(additions) => additions,
            Err(error) => panic!("the document is the fixture's shape: {error}"),
        };
        assert_eq!(additions.columns, ["test_runs.infra_suspect"]);
        assert_eq!(additions.fields, ["CiRunTestFailureOut.infra_suspect"]);
        assert_eq!(additions.paths, ["POST /api/x"]);
        assert_eq!(additions.query_params, ["GET /api/tests acknowledged"]);
        let problems = additions.problems();
        assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    }

    /// An entry that is not a `table.column` pair is reported, by name.
    #[test]
    fn a_column_that_is_not_a_dotted_pair_is_reported() {
        let additions = Additions {
            columns: vec!["no_dot".to_owned()],
            ..Default::default()
        };
        let problems = additions.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("no_dot"),
            "{} does not name no_dot",
            problems[0]
        );
    }

    /// Each list reports the shapes it cannot accept, and accepts the ones it can.
    #[test]
    fn every_list_reports_its_own_malformed_entries() {
        for entry in ["no_dot", "", ".tail", "head.", "a.b.c"] {
            only_problem(&columns(entry), entry);
        }
        for entry in ["no_dot", "a.", "a.b.c"] {
            only_problem(
                &Additions {
                    fields: vec![entry.to_owned()],
                    ..Default::default()
                },
                entry,
            );
        }
        for entry in [
            "get /api/x",
            "GET api/x",
            "GET",
            "GET /api/x extra",
            "GET  /api/x",
            "GET2 /api/x",
        ] {
            only_problem(
                &Additions {
                    paths: vec![entry.to_owned()],
                    ..Default::default()
                },
                entry,
            );
        }
        for entry in ["GET /api/x", "get /api/x name", "GET /api/x ", "GET x name"] {
            only_problem(
                &Additions {
                    query_params: vec![entry.to_owned()],
                    ..Default::default()
                },
                entry,
            );
        }

        for additions in [
            columns("test_runs.infra_suspect"),
            Additions {
                fields: vec!["CiRunTestFailureOut.infra_suspect".to_owned()],
                ..Default::default()
            },
            Additions {
                paths: vec!["POST /api/x".to_owned()],
                ..Default::default()
            },
            Additions {
                query_params: vec!["GET /api/tests acknowledged".to_owned()],
                ..Default::default()
            },
        ] {
            let problems = additions.problems();
            assert!(problems.is_empty(), "\n{}", problems.join("\n"));
        }
    }

    /// The dotted predicates match one whole entry, and nothing else.
    #[test]
    fn the_dotted_predicates_match_a_listed_entry_only() {
        let additions = Additions {
            columns: vec!["test_runs.infra_suspect".to_owned()],
            fields: vec!["CiRunTestFailureOut.infra_suspect".to_owned()],
            ..Default::default()
        };
        assert!(additions.column("test_runs", "infra_suspect"));
        assert!(!additions.column("test_runs", "other"));
        assert!(!additions.column("runs.infra", "suspect"));
        assert!(additions.field("CiRunTestFailureOut", "infra_suspect"));
        assert!(!additions.field("Other", "infra_suspect"));
        assert!(additions.field_name("infra_suspect"));
        assert!(!additions.field_name("other"));
        assert!(!additions.field_name("CiRunTestFailureOut"));
        assert!(
            !additions.field_name("ut.infra_suspect"),
            "the match is on the last dotted part, not on any suffix"
        );

        let empty = Additions::default();
        assert!(
            !empty.column("test_runs", "infra_suspect") && !empty.field_name("infra_suspect"),
            "an empty list ignores nothing"
        );
    }

    /// The route predicates compare the method case-insensitively and everything
    /// else exactly.
    #[test]
    fn the_route_predicates_ignore_only_the_method_case() {
        let additions = Additions {
            paths: vec!["POST /api/x".to_owned()],
            query_params: vec!["GET /api/tests acknowledged".to_owned()],
            ..Default::default()
        };
        assert!(additions.path("POST", "/api/x"));
        assert!(additions.path("post", "/api/x"));
        assert!(!additions.path("get", "/api/x"));
        assert!(!additions.path("post", "/api/y"));
        assert!(!additions.path("post", "/api/x/"));

        assert!(additions.query_param("GET", "/api/tests", "acknowledged"));
        assert!(additions.query_param("get", "/api/tests", "acknowledged"));
        assert!(!additions.query_param("post", "/api/tests", "acknowledged"));
        assert!(!additions.query_param("get", "/api/tests", "unacknowledged"));
        assert!(!additions.query_param("get", "/api/reports", "acknowledged"));
        assert!(!additions.query_param("get", "/api/tests", ""));

        let empty = Additions::default();
        assert!(
            !empty.path("post", "/api/x")
                && !empty.query_param("get", "/api/tests", "acknowledged"),
            "an empty list ignores nothing"
        );
    }
}
