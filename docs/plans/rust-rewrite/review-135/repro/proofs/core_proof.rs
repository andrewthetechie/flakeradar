use flakeradar_core::*;

fn one(time: &str) -> f64 {
    let xml = format!(r#"<testsuite name="s"><testcase classname="c" name="t" time="{time}"/></testsuite>"#);
    let cases = parse_junit_xml(xml.as_bytes(), 1).expect("parser accepted the report");
    cases[0].duration
}

#[test]
fn time_nan_and_overflow_are_accepted() {
    println!("time=NaN -> {:?}", one("NaN"));
    println!("time=1e999 -> {:?}", one("1e999"));
    println!("time=inf -> {:?}", one("inf"));
    println!("time=-5 -> {:?}", one("-5"));
    println!("serde_json of ExecutionOut duration NaN -> {}", serde_json::to_string(&f64::NAN).unwrap());
    assert!(one("NaN").is_nan());
    assert!(one("1e999").is_infinite());
}

#[test]
fn double_struck_digit_reads_as_nine() {
    // U+1D7D9 MATHEMATICAL DOUBLE-STRUCK DIGIT ONE. Python: float("\U0001D7D9") == 1.0
    let duration = one("\u{1D7D9}");
    println!("time=U+1D7D9 -> {duration}");
    assert_eq!(duration, 9.0, "decimal_value walks back through the adjacent Nd block");
}

#[test]
fn suite_name_amplifies_per_case() {
    let name = "A".repeat(50_000);
    let mut xml = format!(r#"<testsuite name="{name}">"#);
    for index in 0..5_000 {
        xml.push_str(&format!(r#"<testcase name="t{index}"/>"#));
    }
    xml.push_str("</testsuite>");
    let cases = parse_junit_xml(xml.as_bytes(), 1).expect("parsed");
    let held: usize = cases.iter().map(|case| case.suite.len()).sum();
    println!("body {} bytes -> {} cases -> {} bytes of suite names ({}x)", xml.len(), cases.len(), held, held / xml.len());
    assert!(held > 500 * xml.len());
}

#[test]
fn retry_children_amplify_the_test_name() {
    let name = "N".repeat(50_000);
    let mut xml = format!(r#"<testsuite name="s"><testcase name="{name}">"#);
    for _ in 0..5_000 {
        xml.push_str("<rerunFailure/>");
    }
    xml.push_str("</testcase></testsuite>");
    let cases = parse_junit_xml(xml.as_bytes(), 1).expect("parsed");
    let held: usize = cases.iter().map(|case| case.name.len()).sum();
    println!("body {} bytes -> {} cases -> {} bytes of names ({}x)", xml.len(), cases.len(), held, held / xml.len());
    assert!(held > 500 * xml.len());
}

#[test]
fn a_bad_runner_label_points_at_the_wrong_job() {
    let job = |id: &str, labels: Vec<String>| JobResultIn {
        ci_job_id: id.to_owned(),
        name: id.to_owned(),
        status: JobStatus::Passed,
        url: String::new(),
        runner_name: String::new(),
        runner_labels: labels,
        started_at: None,
        completed_at: None,
        summary: false,
    };
    let payload = PipelineReportIn {
        repo: "a/b".to_owned(),
        provider: "github".to_owned(),
        pipeline: "ci".to_owned(),
        commit_sha: "abc".to_owned(),
        branch: "main".to_owned(),
        default_branch: None,
        ci_run_id: "1".to_owned(),
        ci_run_attempt: 1,
        jobs: vec![job("j0", vec![]), job("j1", vec![]), job("j2", vec!["x".repeat(256)])],
    };
    let issues = payload.validate().expect_err("the label is too long");
    let loc = serde_json::to_string(&issues[0].loc).unwrap();
    println!("loc for jobs[2].runner_labels[0]: {loc}");
    assert_eq!(loc, r#"["body","jobs",0,"runner_labels"]"#);
}
