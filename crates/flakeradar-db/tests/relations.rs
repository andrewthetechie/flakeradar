//! The relations `sea-orm-cli` generated, resolved against the migrated schema.
//!
//! The port joins explicitly, so nothing else calls these; they are checked here
//! because they are generated from the schema's foreign keys, and a relation
//! that points at a table the baseline no longer has is a mistake the generator
//! would otherwise hide until someone used it.
//!
//! The test resolves every relation and then loads one through
//! `find_with_related`, so the generated `RelationDef` is exercised against the
//! real foreign key rather than just constructed.

#![expect(
    clippy::expect_used,
    reason = "a test fails loudly when the fixture or the generated relation is wrong"
)]

use flakeradar_db::entities::{
    job_executions, job_score_history, jobs, pipelines, projects, reports, repos, test_cases,
    test_executions, test_runs, test_score_history,
};
use flakeradar_test_support::{
    TestDb, make_execution, make_job, make_pipeline, make_project, make_run, make_test_case,
};
use sea_orm::{EntityTrait, ModelTrait as _, Related};

/// Every relation the generator emitted, as `(entity, related entity)`.
///
/// `Relation::X.def()` builds the relation definition from the entity's foreign
/// keys, so resolving one checks that the definition still matches the schema.
macro_rules! relations {
    ($($from:ty => $to:ty),* $(,)?) => {
        /// The relation definitions, each one resolved.
        fn resolve_every_relation() -> usize {
            let mut resolved = 0;
            $(
                let _: sea_orm::RelationDef = <$from as Related<$to>>::to();
                resolved += 1;
            )*
            resolved
        }
    };
}

relations! {
    repos::Entity => pipelines::Entity,
    repos::Entity => projects::Entity,
    repos::Entity => reports::Entity,
    projects::Entity => reports::Entity,
    projects::Entity => repos::Entity,
    projects::Entity => test_cases::Entity,
    projects::Entity => test_runs::Entity,
    pipelines::Entity => jobs::Entity,
    pipelines::Entity => repos::Entity,
    jobs::Entity => job_executions::Entity,
    jobs::Entity => job_score_history::Entity,
    jobs::Entity => pipelines::Entity,
    job_executions::Entity => jobs::Entity,
    job_score_history::Entity => jobs::Entity,
    reports::Entity => projects::Entity,
    reports::Entity => repos::Entity,
    reports::Entity => test_runs::Entity,
    test_cases::Entity => projects::Entity,
    test_cases::Entity => test_executions::Entity,
    test_cases::Entity => test_score_history::Entity,
    test_executions::Entity => test_cases::Entity,
    test_executions::Entity => test_runs::Entity,
    test_runs::Entity => projects::Entity,
    test_runs::Entity => reports::Entity,
    test_runs::Entity => test_executions::Entity,
    test_score_history::Entity => test_cases::Entity,
}

#[tokio::test]
async fn every_generated_relation_resolves() {
    // One per foreign key in the baseline schema, counted so a generation that
    // drops one is visible here rather than as a query that returns nothing.
    assert_eq!(resolve_every_relation(), 26);
}

#[tokio::test]
async fn a_relation_loads_the_rows_behind_it() {
    let db = TestDb::new().await.expect("a migrated test database");
    let project = make_project()
        .build(db.connection())
        .await
        .expect("a Project");
    let test_case = make_test_case(&project)
        .build(db.connection())
        .await
        .expect("a Test");
    let run = make_run(&project)
        .build(db.connection())
        .await
        .expect("a Run");
    make_execution(&test_case, &run)
        .build(db.connection())
        .await
        .expect("an Execution");

    let tests = projects::Entity::find_by_id(project.id)
        .one(db.connection())
        .await
        .expect("the Project is readable")
        .expect("the Project exists")
        .find_related(test_cases::Entity)
        .all(db.connection())
        .await
        .expect("the relation resolves");
    assert_eq!(tests.len(), 1);

    let executions = test_cases::Entity::find_by_id(test_case.id)
        .one(db.connection())
        .await
        .expect("the Test is readable")
        .expect("the Test exists")
        .find_related(test_executions::Entity)
        .all(db.connection())
        .await
        .expect("the relation resolves");
    assert_eq!(executions.len(), 1);

    let pipeline = make_pipeline()
        .build(db.connection())
        .await
        .expect("a Pipeline");
    make_job(&pipeline)
        .build(db.connection())
        .await
        .expect("a Job");
    let related = repos::Entity::find()
        .find_with_related(projects::Entity)
        .all(db.connection())
        .await
        .expect("the relation resolves");
    assert_eq!(related.iter().flat_map(|(_, projects)| projects).count(), 1);
}
