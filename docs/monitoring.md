# Monitoring

FlakeRadar has two health endpoints and a Prometheus endpoint. All three have
no authentication, like the other read APIs. Put the dashboard on a private
network.

## Health checks

- `GET /api/health` is a liveness check.
- `GET /api/health/ready` checks the database. When `SELECT 1` succeeds, it
  answers `200` with
  `{database, processor_leader, pending_reports, oldest_pending_age_seconds}`.
  When the database is not available, it answers `503` with
  `{"database": false}`. A missing report processor leader shows in the body,
  not as `503`.

## Metrics

`GET /metrics` serves the Prometheus text format. It has six gauge families.
FlakeRadar reads each value from the database, so each replica returns the same
values.

| Gauge | Value |
|---|---|
| `flakeradar_reports{status}` | The number of reports for each status: `pending`, `processed` or `failed`. |
| `flakeradar_oldest_pending_report_age_seconds` | The age in seconds of the oldest pending report. It is 0 when the queue is empty. |
| `flakeradar_processor_leader` | 1 when a process holds the advisory lock of the report processor. |
| `flakeradar_tests{tier}` | The number of tests in each tier: `flaky`, `suspect` or `stable`. |
| `flakeradar_jobs{tier}` | The number of jobs in each tier. |
| `flakeradar_last_report_processed_timestamp_seconds` | The Unix time of the last processed report that FlakeRadar still keeps. |

The last gauge is absent until FlakeRadar processes the first report. It can
move backwards, and it can disappear again, when retention deletes processed
reports. `FLAKERADAR_REPORT_RETENTION_DAYS` sets that retention, and the
default is 7 days.

`flakeradar_processor_leader 0` during a redeploy is normal. The lock moves
between processes, so this value alone is not a reason for an alert.

Two settings change the `/metrics` output:

- `FLAKERADAR_FLAKE_THRESHOLD` sets the tier of each test and job in the
  `flakeradar_tests{tier}` and `flakeradar_jobs{tier}` gauges.
- `FLAKERADAR_REPORT_RETENTION_DAYS` sets when FlakeRadar deletes the processed
  reports that give the last-processed timestamp.

A Prometheus scrape configuration:

```yaml
scrape_configs:
  - job_name: flakeradar
    static_configs:
      - targets: ["localhost:8000"]
```

An alert for a queue that stops draining:

```yaml
- alert: FlakeRadarQueueStuck
  expr: flakeradar_oldest_pending_report_age_seconds > 600
```
