# Quarantine samples

A quarantine consumer reads the quarantine list for its repo and project, then
skips the tests on that list in its test runner. Each consumer is a small,
copyable sample that runs in Docker and tests itself.

Every consumer follows the contract below. It keeps the run green when
FlakeRadar is down, and it skips only the tests a person quarantined.

## Contract

### Env

The consumer reads four environment variables. `FLAKERADAR_URL` is the server
URL, with one trailing `/` removed. `FLAKERADAR_TOKEN` is the API token.
`FLAKERADAR_REPO` is the repo as `owner/name`; when it is empty, the consumer
uses `GITHUB_REPOSITORY`. `FLAKERADAR_PROJECT` is the project name. An empty
variable counts as unset.

### Off

When the URL or the repo is unset, the consumer fetches nothing, writes
nothing to stderr, and skips nothing. A snippet whose stdout feeds a command
prints its neutral value on stdout.

### Request

The consumer sends `GET <url>/api/quarantine?repo=<repo>&project=<project>`.

The language's standard encoder percent-encodes the values, so `acme/app` is
sent as `acme%2Fapp`. `project` is left out when unset; the server then uses
the `default` Project. `X-API-Key: <token>` is sent only when a token is set.
Without one, the server answers 401 and the fail-open rule applies. The whole
request has a 10-second timeout.

### Valid body

A valid body is a JSON array whose every element is an object with a string
`classname` and a non-empty string `name`. Other keys are ignored. One invalid
element makes the whole body invalid.

### Fail open

On a connection error, a timeout, a non-2xx status or an invalid body, the
consumer writes exactly one line to stderr:

```text
flakeradar-quarantine: <reason>; skipping nothing
```

The pytest plugin writes that line through pytest's terminal reporter instead.
The consumer then skips nothing, and it never fails the run because of
FlakeRadar.

### Tests

An HTTP test server compares the decoded query, so `repo` is `acme/app`, not
the raw string. A curl shim compares the raw URL, because jq's `@uri` output is
fixed. The default command of a sample's Docker image exits 0 only when every
case passed.

### Fixture

Each `quarantine.json` uses this exact shape. Only `suite`, `classname` and
`name` change, and `fingerprint` ends in the item's 1-based index. The whole
fingerprint is the 40 lowercase hex characters the endpoint writes. The
timestamp form is the one the server writes, `%Y-%m-%dT%H:%M:%S%.6fZ`.

```json
[
  {"suite": "...", "classname": "...", "name": "...",
   "fingerprint": "0000000000000000000000000000000000000001", "file": null, "line": null,
   "quarantined_at": "2026-11-01T12:00:00.000000Z", "reason": "flaky in CI",
   "expires_at": "2026-12-01T12:00:00.000000Z"},
  {"suite": "...", "classname": "...", "name": "...",
   "fingerprint": "0000000000000000000000000000000000000002", "file": null, "line": null,
   "quarantined_at": "2026-11-01T12:00:00.000000Z", "reason": null, "expires_at": null}
]
```

### Docs formats

Add a row to the `## Consumers` table below:

```text
| <Runner> | [<path>](<path>) | <how it skips> |
```

Add an item to the list under `### Skip quarantined tests in your runner` in
`docs/concepts.md`:

```text
- <Runner>: [<file name>](../samples/quarantine/<path>)
```

## Sample layout

A sample lives in `samples/quarantine/<runner>/` and holds:

- its consumer;
- at least one `quarantine.json`, anywhere under the dir, that uses the names
  the runner's JUnit reporter writes;
- its test;
- a `Dockerfile` whose default command runs that test.

The Docker build context is `samples/quarantine/`, so a sample can copy shared
files.

## Run the samples

```bash
samples/quarantine/test.sh [runner...]
```

With no arguments, the script builds and runs every sample dir that has a
`Dockerfile`. With arguments, it runs only the named dirs.

## Consumers

| Runner | Consumer | How it skips |
|---|---|---|
