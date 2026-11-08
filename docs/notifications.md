# Notifications

FlakeRadar can post a message when a test or a job becomes flaky, and when it
recovers. This is for teams who work in Slack and do not want an issue for each
flaky test.

## Set up

Set `FLAKERADAR_WEBHOOK_URLS` to a comma-separated list of URLs, and choose a
format:

```bash
FLAKERADAR_WEBHOOK_URLS=https://hooks.slack.com/services/T000/B000/xxxx
FLAKERADAR_WEBHOOK_FORMAT=slack
FLAKERADAR_PUBLIC_URL=https://flakeradar.example.com
```

## Events

FlakeRadar sends a message only when a test or job crosses the flake
threshold:

| Event | When |
|---|---|
| `test.flaky` | The score of a test reached the threshold. |
| `test.recovered` | The score of a test fell below the threshold again. |
| `job.flaky` | The score of a job reached the threshold. |
| `job.recovered` | The score of a job fell below the threshold again. |

FlakeRadar sends nothing in these cases:

- The score moves, but the test or job stays on the same side of the
  threshold.
- A report changes the default branch. That change scores the whole repo
  again, so the report sends no events at all, also for the tests and jobs
  that it changed. The log records how many events FlakeRadar dropped.
- Retention deletes old executions, and a score falls as a result. The log
  records how many events FlakeRadar dropped.
- A test that is now [broken](concepts.md#broken-tests) leaves the flaky tier.
  A broken test has not recovered, so there is no closing event. Thus
  `test.flaky` can be the last event that you see for that test. When someone
  fixes the test later, you get an event only if the score crosses the
  threshold again. The first pass can send a new `test.flaky`, and a later pass
  can send `test.recovered`.

## Formats

`json` (the default) posts a list of event objects, one for each test or job
that the report changed. One report is one request:

```json
[
  {
    "event": "test.flaky",
    "repo": "acme/app",
    "project": "backend",
    "test": {
      "id": 42,
      "classname": "tests.test_auth",
      "name": "test_login",
      "flakiness_score": 0.42,
      "tier": "flaky",
      "confirmed_flake_count": 2,
      "failure_category": "timing"
    },
    "url": "https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42"
  }
]
```

A job event has `"project": null` and a `job` object with `id`, `pipeline`,
`name`, `flakiness_score`, `tier` and `confirmed_flake_count`. Its `url` opens
the jobs view. FlakeRadar leaves out `url` when `FLAKERADAR_PUBLIC_URL` is
empty.

`slack` posts `{"text": ...}` with one line for each event:

```
FlakeRadar: acme/app / backend — test "test_login" became flaky (score 0.42) https://flakeradar.example.com/?repo=acme%2Fapp&project=backend&test=42
```

## Delivery

Delivery is best effort:

- Each URL gets three attempts of five seconds each, with a short wait between
  them.
- FlakeRadar obeys the `retry-after` of a rate limit, up to ten seconds.
- FlakeRadar retries a timeout, a `5xx`, a `408` and a `429`. Any other `4xx`
  is a configuration error, and FlakeRadar does not retry it.

FlakeRadar keeps no queue on disk. A message is lost when FlakeRadar cannot
deliver it, or when it is still in the queue when the process stops. The queue
holds the messages of 1000 reports. When it is full, FlakeRadar drops the
oldest messages, so a long endpoint outage also loses messages.

The dashboard still shows the current tier, and the next report that changes
the tier sends a new message.
