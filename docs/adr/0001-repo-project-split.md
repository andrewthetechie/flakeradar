# Split Repo from Project, and require `repo` on ingest

In upstream FlakeRadar, one `project` string stood for both a repository and a test suite. Our own data showed the problem: names like `writers-app-backend` joined the two, and they had no owner. We now treat a **Repo** (lowercase `owner/name`) and a **Project** (a suite inside a Repo, `default` if omitted) as two separate things, and a Test is unique within (Repo, Project). Ingest rejects any upload that has no `repo`. This breaks upstream's API, and we accept that: silently filing ambiguous data is how the old names got mixed up. We wiped the existing data instead of migrating it, because 13 Runs gave almost no scoring signal.

## Considered Options

- Accept `project` alone under a placeholder repo: rejected, because it keeps the ambiguity.
- Guess the repo by splitting the project string: rejected, because the guesses have no owner and can be wrong.
