# Configuration

FlakeRadar reads its settings from environment variables. Each variable has the
prefix `FLAKERADAR_`, for example `FLAKERADAR_API_TOKEN`. FlakeRadar also reads a
`.env` file from the working directory. A variable in the process environment
overrides the same variable in `.env`. A missing `.env` file is not an error.
[`.env.example`](../.env.example) lists the common settings with comments.

FlakeRadar checks each value at start-up. A value outside the meaning of its
setting stops the process with an error that names the variable. Examples are a
NaN or out-of-range score decay, a score window below 1, a negative retention
window, and a poll of 0. FlakeRadar does not score with a value that nobody
meant.

## Settings

The table gives each name without the `FLAKERADAR_` prefix.

| Variable | Default | Meaning |
|---|---|---|
| `API_TOKEN` | `changeme` | The token for CI uploads (`X-API-Key`) and for MCP (`Authorization: Bearer`). The server does not start with `changeme` unless `FLAKERADAR_ALLOW_INSECURE=1`. Generate a token with `openssl rand -base64 32`. |
| `ALLOW_INSECURE` | not set | `1` lets the server start with the default token, for a local demo only. FlakeRadar reads it from the process environment only, so a value in `.env` has no effect for the binary. Docker Compose passes `.env` to the container as its environment. |
| `DATABASE_URL` | `postgresql://flakeradar:flakeradar@localhost:5432/flakeradar` | PostgreSQL only. SQLx reads the libpq spellings. An old `postgresql+asyncpg://` URL must drop the `+asyncpg`, and `?ssl=` must become `?sslmode=`. Docker Compose builds this URL itself. |
| `HOST` / `PORT` | `127.0.0.1` / `8000` | The address that the binary binds. The container image sets `FLAKERADAR_HOST=0.0.0.0`. |
| `DB_COMMAND_TIMEOUT_SECONDS` | `30` | The time that one SQL statement can run before PostgreSQL cancels it. |
| `FLAKE_THRESHOLD` | `0.30` | The score at which a test or job becomes flaky. The `/metrics` gauges count the tier state (`flaky_since`), which `FLAKE_THRESHOLD` and `RECOVER_THRESHOLD` drive. |
| `RECOVER_THRESHOLD` | `0.20` | The score below which a flaky test or job recovers, clearing its flaky state. It must not be greater than `FLAKE_THRESHOLD`: the band between the two is what stops a score that hovers at the flake threshold from changing the tier on every report ([ADR 0012](adr/0012-tier-hysteresis.md)). |
| `SCORE_WINDOW` / `SCORE_DECAY` | `50` / `0.85` | The number of executions to score, and the weight for recent executions. |
| `BROKEN_STREAK` | `5` | The number of consecutive failures on the default branch at which a test is [broken](concepts.md#broken-tests). `0` disables broken detection. |
| `INFRA_FAIL_SHARE` | `0.8` | The share of a run's tests that must fail or error on their last attempt for the run to be [suspected infrastructure](concepts.md#suspected-infrastructure) and left out of scoring. A number above `0` and at most `1`. |
| `INFRA_MIN_TESTS` | `20` | The fewest distinct tests a run must hold for FlakeRadar to judge it at all. A smaller run is always scored, because a suite of 3 that fails entirely has a broken test, not a bad runner. |
| `GITHUB_TOKEN` | empty (disabled) | The token for [GitHub issue automation](github-issues.md). FlakeRadar also uses it to check linked issues, so it needs read access to the issues of those repos. |
| `GITHUB_ISSUE_LABEL` | `flakeradar` | The label on each issue that FlakeRadar opens. |
| `GITHUB_ISSUE_MIN_SCORE` / `GITHUB_ISSUE_MIN_PROVEN_FLAKES` / `GITHUB_ISSUE_MIN_FAILURES` | `0.30` / `0` / `0` | The [filing gate](github-issues.md#filing-gate). |
| `GITHUB_SYNC_BATCH` | `2000` | How many linked tests, and separately how many linked jobs, the maintenance sync checks in each run. The oldest checks run first. |
| `GITHUB_FIXED_STREAK` | `0` (disabled) | The clean streak at which FlakeRadar comments one time on the linked issue that the fix appears to hold. |
| `GITHUB_CLOSE_FIXED` | `false` | Also close the issue as completed when FlakeRadar posts that comment. |
| `WEBHOOK_URLS` | empty (disabled) | Comma-separated URLs that get [notifications](notifications.md). |
| `WEBHOOK_FORMAT` | `json` | `json` posts a list of event objects. `slack` posts `{"text": ...}` with one line for each event. |
| `PUBLIC_URL` | empty | The base URL of this instance. When it is set, fixed-note comments and notifications have links to the test, the job or the dashboard. |
| `WORKER_POLL_SECONDS` | `1.0` | How often the report processor checks an empty queue. |
| `MAX_REPORT_RETRIES` | `3` | How many times FlakeRadar processes a report again after a transient failure. |
| `REPORT_RETENTION_DAYS` | `7` | FlakeRadar deletes processed raw reports after this time. |
| `FAILED_REPORT_RETENTION_DAYS` | `0` | FlakeRadar keeps failed reports until a person retries or dismisses each one. Set this to delete failed reports older than this number of days. |
| `EXECUTION_RETENTION_DAYS` | `90` | FlakeRadar deletes older executions. |
| `SCORE_HISTORY_RETENTION_DAYS` | `365` | FlakeRadar deletes older daily score history rows. |
| `PRUNE_INTERVAL_SECONDS` | `3600` | How often retention runs. |
| `PRUNE_BATCH` | `5000` | How many rows one retention round deletes from each table. |
| `PRUNE_TIME_BUDGET_SECONDS` | `60` | The time limit of one retention run. The next run continues from where the last run stopped. |
| `CORS_ORIGINS` | `http://localhost:5173` | The origin of the frontend dev server. |

## Logs

FlakeRadar writes logs to stdout. The default level is `info`. `RUST_LOG`
overrides it with the filter syntax of `tracing`:

- `RUST_LOG=warn` gives fewer lines.
- `RUST_LOG=debug` adds detail.
- `RUST_LOG=info,sqlx=debug` adds the SQL statements and nothing else.

FlakeRadar logs SQL statements at `debug`, not at the `info` level of the
driver. At `info`, on a demo install with demo data, 1,546 of 1,605 log lines
were SQL statements. The other lines were difficult to find.
