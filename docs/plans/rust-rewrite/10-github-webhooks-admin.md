# 10 — Issue links, GitHub, webhooks, repo admin

Read `00-shared-context.md` first. Tasks 01–09 have landed.

Python reference: `backend/app/issue_links.py`, `github_integration.py` (read its module docstring in full, because it states the error policy), `worker.py` (`GitHubWorker`, `WebhookWorker`), `notify.py`, `routers/repos.py`, and the issue routes in `routers/tests.py` and `routers/jobs.py`. ADRs 0008 and 0009.

## Goal

Everything that writes outward, plus Issue links and admin deletes. This is the largest test port (175 tests). Port it file by file and commit often.

Repo admin delete is in this task, rather than task 09, because it calls the GitHub issue closer.

## Contracts

### Issue links

Port the `IssueLinkTarget` descriptor and engines:

- `PUT /api/tests/{id}/issue?replace=`, `DELETE /api/tests/{id}/issue`, and the same for Jobs, all token-protected.
- `FOR UPDATE` row lock on the entity.
- Same-number link is a 200 with no change. A different number is 409 unless `replace=true`, with the same text.
- `url` is optional, and must equal the derived URL after `strip`, trailing `/` removal and lowercasing.
- Jobs only on `github` Pipelines.
- Changing or clearing a link clears `github_issue_filed`, `github_issue_closed`, `github_issue_fixed_noted_at` and `checked_at` exactly as the engine does.
- Errors are a type with a status (404, 409, 422), so MCP can reuse the text in task 11.

### GitHub client

`reqwest` with base URL `https://api.github.com` **injectable** (for `wiremock`), `Accept: application/vnd.github+json`, `X-GitHub-Api-Version: 2022-11-28`, bearer token and a 15-second timeout. Set a generic `User-Agent: flakeradar/<version>`; GitHub requires one, and `reqwest` sets none.

Port these with the same behaviour, batch order and text. Issue bodies and comments must match byte for byte:

- `_issue_body`, `_job_issue_body`, `_run_hint`, `_fixed_comment`, `_CLOSE_COMMENT`
- the filing gate (`_FilingGate`), including the fixed-streak rule
- `file_issues_for`, `file_job_issues_for`
- `sync_closed_issues` with the rotation (`linked_for_check`, `github_sync_batch`, `checked_at NULLS FIRST`)
- `_clear_if_closed`
- `close_issues_for_deleted_repo`
- `note_fixed_issues_for`, `note_fixed_job_issues_for`
- `on_report_processed`

**Error policy:** follow the module docstring case by case, because each case has tests: rate-limit detection (`_rate_limited`), a permission 403, 401, 404, 410, the row vanishing between POST and commit (StaleDataError: comment on and close the new issue, best effort), and "never raises". In Rust, a vanished row is an `UPDATE` with `rows_affected == 0`.

**Without a token:** a silent no-op.

### `GitHubWorker`

A `LeaderTask`. Unbounded queue, with `enqueue` never waiting. Its one task loops: if a sync is due, sync; otherwise wait for an outcome until the next sync is due, then file. Errors are logged, and the task stays alive. Queue contents are lost if the process dies, which is accepted. The first sync runs at start.

### `WebhookWorker`

A `LeaderTask`, started only when URLs are configured. Queue of at most 1000 batches, dropping the **oldest** when full, with a logged count. Outcomes with no Tier change are not queued. Each batch goes to every URL concurrently. Per URL:

- 3 attempts, delays `0.5` and `1.0` seconds, 5-second timeout.
- 2xx is success.
- A 4xx other than 408 and 429 is permanent.
- A numeric `retry-after` replaces the delay, capped at 10 seconds. An HTTP-date is ignored.
- A failure never raises.
- **Never log a Webhook URL.** Logs name its host only, and `reqwest` errors must have their URL removed with `without_url()` before being logged. Test this by capturing logs.

### Repo admin

Port `routers/repos.py`: `DELETE /api/repos/{owner}/{name}` and `DELETE /api/repos/{owner}/{name}/projects/{project}`, with the token, the `FOR UPDATE` lock, deleted counts, the cascade, closing filed issues after commit, and the retryable 503 on a deadlock with the processor.

### Wiring

`run_server` builds both workers. `on_processed` hands outcomes to the GitHub task, and the webhook hook runs after it. Both stop before unlock (task 08).

## Tests to port

`test_github` (95), `test_github_worker` (14), `test_issue_links_api` (16), `test_issue_links_lifecycle` (10), `test_webhook` (11), `test_webhook_worker` (14), `test_repos_api` (15).

Use `wiremock`, and assert request bodies and headers, not only call counts. Enable the remaining parity scenarios.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] All REST parity scenarios pass.
- [ ] A log-capture test proves no Webhook URL or GitHub token appears in logs at any level.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
