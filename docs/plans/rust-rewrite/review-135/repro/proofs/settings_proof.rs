//! Proofs for H4 and J2, rewritten after the fix: a `.env` line that does not
//! parse never puts its text in the error, and a setting whose value its field
//! could not mean refuses to start.
//!
//! At `f9330fd` the error quoted the whole line (token included), and a NaN decay,
//! a negative threshold, window or poll all started the server.

use flakeradar::settings::{Settings, SettingsError};
use std::collections::BTreeMap;

#[test]
fn a_malformed_env_line_never_reaches_the_error_text() {
    let dir = std::env::temp_dir().join(format!("pr135-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the directory is created");
    std::fs::write(
        dir.join(".env"),
        "FLAKERADAR_SCORE_WINDOW=7\nFLAKERADAR_API_TOKEN=\"s3cret-token\n",
    )
    .expect("the file is written");
    std::env::set_current_dir(&dir).expect("the directory is entered");

    let error = Settings::from_env().expect_err("the second line does not parse");
    let text = error.to_string();
    println!("{text}");
    assert!(
        !text.contains("s3cret-token"),
        "the error names the line, never the line's text"
    );
    assert!(text.contains(".env"), "the error names the file");
    assert!(text.contains("line 2"), "the error names the line number");
}

#[test]
fn a_setting_outside_its_fields_meaning_refuses_to_start() {
    for (key, value) in [
        ("FLAKERADAR_SCORE_DECAY", "NaN"),
        ("FLAKERADAR_FLAKE_THRESHOLD", "-3"),
        ("FLAKERADAR_SCORE_WINDOW", "-1"),
        ("FLAKERADAR_WORKER_POLL_SECONDS", "-1"),
    ] {
        let mut values = BTreeMap::new();
        values.insert(key.to_owned(), value.to_owned());
        let reading = Settings::from_map(&values);
        println!("{key}={value} -> {reading:?}");
        // Either refusal is the same outcome for an operator: the process does
        // not start, and the message names the variable. `SCORE_WINDOW` is an
        // unsigned field, so `-1` fails to parse rather than failing the range.
        match reading {
            Err(
                SettingsError::OutOfRange { variable, .. }
                | SettingsError::InvalidValue { variable, .. },
            ) => assert_eq!(variable, key),
            other => panic!("{key}={value} must be refused, not {other:?}"),
        }
    }
}
