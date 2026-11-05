use flakeradar::Settings;
use flakeradar_core::{ExecutionStatus::{Failed, Passed}, combined_score};

#[test]
fn a_malformed_env_line_with_a_token_reaches_the_error_text() {
    let dir = std::env::temp_dir().join(format!("pr135-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".env"), "FLAKERADAR_API_TOKEN=\"s3cret-token\n").unwrap();
    std::env::set_current_dir(&dir).unwrap();
    let error = Settings::from_env().expect_err("the line does not parse");
    let text = format!("{:?}", anyhow_like(&error));
    println!("{text}");
    assert!(text.contains("s3cret-token"));
}

fn anyhow_like(error: &flakeradar::SettingsError) -> String {
    error.to_string()
}

#[test]
fn nan_decay_scores_every_history_one() {
    let (score, _) = combined_score(&[Passed, Passed, Passed, Passed], &[], f64::NAN, 50);
    println!("all-passing history with decay NaN scores {score}");
    assert_eq!(score, 1.0);
    let (score, _) = combined_score(&[Passed, Failed, Passed, Failed], &[], f64::INFINITY, 50);
    println!("decay inf scores {score}");
}

#[test]
fn negative_decay_and_threshold_parse() {
    let mut values = std::collections::BTreeMap::new();
    for (key, value) in [
        ("FLAKERADAR_SCORE_DECAY", "NaN"),
        ("FLAKERADAR_FLAKE_THRESHOLD", "-3"),
        ("FLAKERADAR_SCORE_WINDOW", "-1"),
        ("FLAKERADAR_WORKER_POLL_SECONDS", "-1"),
    ] {
        values.insert(key.to_owned(), value.to_owned());
    }
    let settings = Settings::from_map(&values).expect("nothing checks the ranges");
    println!(
        "decay={} threshold={} window={} poll={:?}",
        settings.score_decay,
        settings.flake_threshold,
        settings.processing().score_window,
        settings.worker().poll
    );
}
