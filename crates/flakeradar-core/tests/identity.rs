//! What this module promises, asserted from outside it.

// Rust guideline compliant 2026-07-21

#![expect(
    clippy::unwrap_used,
    reason = "a test fails loudly when the rule under test is wrong"
)]

use flakeradar_core::{
    identity::DEFAULT_PROJECT, identity::IdentityError, identity::normalize_project,
    identity::normalize_provider, identity::normalize_repo, identity::normalize_root,
};

#[test]
fn normalize_repo_lowercases_and_trims() {
    assert_eq!(
        normalize_repo("  AndrewTheTechie/Writers-App ").unwrap(),
        "andrewthetechie/writers-app"
    );
    for bad in ["", "writers-app", "a/b/c", "a b/c", "owner/"] {
        assert!(
            matches!(normalize_repo(bad), Err(IdentityError::Repo { .. })),
            "{bad:?} must be rejected"
        );
    }
}

#[test]
fn normalize_repo_rejects_more_than_255_characters() {
    let long = format!("{}/{}", "o".repeat(250), "n".repeat(10));
    let error = normalize_repo(&long).err();
    assert!(matches!(error, Some(IdentityError::Repo { .. })));
}

#[test]
fn normalize_project_defaults_and_lowercases() {
    assert_eq!(normalize_project(None).unwrap(), DEFAULT_PROJECT);
    assert_eq!(normalize_project(Some("")).unwrap(), DEFAULT_PROJECT);
    assert_eq!(normalize_project(Some(" Backend ")).unwrap(), "backend");
    let long_project = "x".repeat(101);
    for bad in ["front end", "a/b", long_project.as_str()] {
        assert!(matches!(
            normalize_project(Some(bad)),
            Err(IdentityError::Project { .. })
        ));
    }
}

#[test]
fn normalize_root_strips_dot_slash_and_slashes() {
    assert_eq!(normalize_root(None).unwrap(), None);
    assert_eq!(normalize_root(Some("")).unwrap(), Some(String::new()));
    assert_eq!(
        normalize_root(Some("./frontend/")).unwrap(),
        Some("frontend".to_owned())
    );
    assert_eq!(
        normalize_root(Some("/apps/web")).unwrap(),
        Some("apps/web".to_owned())
    );
    assert!(matches!(
        normalize_root(Some("../secrets")),
        Err(IdentityError::Root { .. })
    ));
}

#[test]
fn normalize_root_keeps_inner_dot_segments() {
    // A single dot is not a `..` segment, so it stays.
    assert_eq!(
        normalize_root(Some("a/./b")).unwrap(),
        Some("a/./b".to_owned())
    );
}

#[test]
fn normalize_provider_lowercases_and_validates() {
    assert_eq!(normalize_provider(" GitHub ").unwrap(), "github");
    assert_eq!(normalize_provider("GITLAB").unwrap(), "gitlab");
    let long_provider = "x".repeat(33);
    for bad in ["", "a/b", "has space", long_provider.as_str()] {
        assert!(matches!(
            normalize_provider(bad),
            Err(IdentityError::Provider { .. })
        ));
    }
}

#[test]
fn errors_name_the_raw_value() {
    assert_eq!(
        normalize_repo("a b/c").unwrap_err().to_string(),
        "repo must look like 'owner/name', got 'a b/c'"
    );
    assert_eq!(
        normalize_project(Some("front end"))
            .unwrap_err()
            .to_string(),
        "project must be 1-100 chars of a-z, 0-9, '.', '_' or '-', got 'front end'"
    );
    assert_eq!(
        normalize_root(Some("../secrets")).unwrap_err().to_string(),
        "root must not contain '..', got '../secrets'"
    );
    assert_eq!(
        normalize_provider("has space").unwrap_err().to_string(),
        "provider must be 1-32 chars of a-z, 0-9, '_' or '-', got 'has space'"
    );
}
