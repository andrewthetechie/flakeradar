# Accept-then-process ingest on a Postgres queue, with one processor

Upstream processed each JUnit upload inside the request, in an `async` handler, with synchronous database work. A 2,500-test report blocked the event loop for about 0.5 s, and concurrent uploads could time out on SQLite's writer lock, so their Runs were lost. Now the request only checks that the XML parses, stores the raw report, and returns `202`. A background task claims pending reports in the order they arrived. Only the task that holds a Postgres advisory lock processes reports, and the tasks in other web workers stay on standby. There is only one processor because Flakiness scores depend on the order of Executions: if two processors rescored the same Test in parallel, one could read data the other had not committed yet. A report that fails processing is kept with its error and can be retried.

## Considered Options

- Process synchronously, with batched queries and client retries: rejected, because a busy server still fails the upload.
- External queue (Redis with arq or Celery): rejected, because it adds a service to run and gives no benefit at our scale (about 4 repos).
- A separate worker container: possible later. We chose the advisory lock so that we deploy only one image and one service.

## Consequences

- The ingest response no longer includes pass/fail counts. Clients get them from `GET /api/reports/{id}`.
