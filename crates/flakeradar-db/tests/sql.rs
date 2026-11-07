//! The placeholder numbering a built statement shares with the ones a caller spells.
//!
//! No database: a bind list, a `VALUES` row and a placeholder run are strings.

use sea_orm::Value;

use flakeradar_db::sql::{Binds, placeholder_list, value_row};

#[test]
fn a_condition_suffix_is_empty_without_conditions() {
    assert_eq!(Binds::new().suffix(), "");
}

#[test]
fn every_bound_value_comes_with_its_own_placeholder() {
    let mut binds = Binds::new();
    binds.eq("r.name", Value::from("acme/app"));
    binds.raw("t.flakiness_score > 0");
    let placeholder = binds.bind(Value::from(0.3));
    binds.raw(format!("t.flakiness_score >= {placeholder}"));
    assert_eq!(
        binds.suffix(),
        " WHERE r.name = $1 AND t.flakiness_score > 0 AND t.flakiness_score >= $2"
    );
    assert_eq!(binds.values().len(), 2);
}

#[test]
fn a_builder_that_starts_later_keeps_the_numbers_a_statement_reserved() {
    let mut binds = Binds::after(3);
    assert_eq!(binds.next(), "$3");
    binds.eq("p.name", Value::from("backend"));
    assert_eq!(binds.suffix(), " WHERE p.name = $3");
    assert_eq!(binds.into_values().len(), 1);
}

#[test]
fn a_value_row_numbers_every_column_from_its_start() {
    assert_eq!(value_row(1, 3), "($1, $2, $3)");
    assert_eq!(value_row(7, 2), "($7, $8)");
    assert_eq!(placeholder_list(4, 2), "$4, $5");
    assert_eq!(placeholder_list(3, 0), "");
}
