# FlakeRadar

Self-hosted flaky-test detection: CI uploads JUnit reports, and FlakeRadar scores how often each test's outcome flips between pass and fail.

## Language

### Where tests live

**Repo**:
A source repository, identified by its lowercase `owner/name` (for example `andrewthetechie/writers-app`). Unique across the instance.
_Avoid_: Repository slug, project (for a repository)

**Project**:
A named test suite inside a Repo, such as `frontend`, `backend` or `e2e`. Unique within its Repo, and the same name can exist in other Repos. A Repo with a single suite uses the project `default`.
_Avoid_: Suite (that is a JUnit concept), component, package

**Test**:
One logical test case, identified by its suite, classname and name within one Repo and Project. It is the same Test across runs, branches and CI providers.
_Avoid_: Test case (in user-facing text), spec

**Project root**:
The directory of a Project inside its Repo, such as `frontend`. File paths that a Project reports are relative to it.

**Location**:
The file (relative to the Project root) and line where a Test is defined, as the latest report gave them. A Location is not part of a Test's identity, and many runners report none.

### What CI reports

**Report**:
One JUnit XML file that CI uploads for a Repo and Project. It is pending until it is processed. Then it has either become a Run, or it has failed and is kept with its error.
_Avoid_: Upload, payload

**Run**:
The processed result of one Report: the Executions for one Repo and Project at a specific commit SHA.
_Avoid_: Build, job

**Execution**:
One Test's outcome (passed, failed, error or skipped) within one Run.
_Avoid_: Result, attempt

**Failure message**:
The short, one-line reason that a failing Execution gives, such as `AssertionError: expected 3, got 4`.

**Failure details**:
The full output that a failing Execution gives: the traceback and any captured stdout or stderr.
_Avoid_: Stack trace (details can hold more than a trace)

### How flakiness is judged

**Flakiness score**:
A number from 0 to 1 that measures how often a Test's outcome flips between pass and fail, with recent flips weighted more. A Test that always fails scores 0: it is broken, not flaky.

**Proven flake**:
A commit SHA on which the same Test both passed and failed. This proves nondeterminism and puts a floor under the Flakiness score.
_Avoid_: Confirmed flake, same-SHA flip (in user-facing text)

**Flake threshold**:
The Flakiness score at or above which a Test counts as a Flaky test.

**Flaky test**:
A Test whose Flakiness score is at or above the Flake threshold.

**Suspect test**:
A Test whose Flakiness score is above 0 but below the Flake threshold.

**Stable test**:
A Test whose Flakiness score is 0.

**Quarantine**:
A reversible human decision that marks a Test as one the test runner may skip. FlakeRadar never quarantines a Test on its own, and agents cannot quarantine through the MCP server.
