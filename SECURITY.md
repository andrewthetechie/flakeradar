# Security Policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately via GitHub Security Advisories
(the "Report a vulnerability" button on the Security tab) rather than a public
issue. We aim to acknowledge reports within a few days.

## Trust model

FlakeRadar's security posture is deliberate and documented:

- **Write path is token-gated.** `POST /api/ingest`, `POST /api/ingest/pipeline`,
  `GET /api/quarantine`, the Issue link endpoints, the admin Repo/Project
  delete and the MCP server at `/mcp/` require the token — `X-API-Key`, or
  `Authorization: Bearer` for MCP — compared in constant time. These are the
  endpoints external CI and agents call. An empty `FLAKERADAR_API_TOKEN` makes
  the comparison match an absent header, so set a real one.
- **Read/dashboard APIs are open by design.** They are expected to sit on a
  private network or behind an authenticating reverse proxy. Do not expose the
  dashboard directly to the public internet.
- **The quarantine toggle (`POST /api/tests/{id}/quarantine`) is an
  intentionally open *write*.** It is a dashboard action (internal origin), so
  it shares the dashboard's trust boundary rather than the token-gated CI path.
  Anyone who can reach the dashboard can quarantine/un-quarantine a test — the
  same trust level under which they can already read every result. Gate it at
  the reverse proxy if your dashboard is not on a trusted network.
- **Secrets** (`FLAKERADAR_API_TOKEN`, `FLAKERADAR_GITHUB_TOKEN`,
  `FLAKERADAR_WEBHOOK_URLS`) live only in `.env`, which is git-ignored. Never
  commit them. Rotate the API token if it is ever exposed. The binary
  **refuses to start** on the shipped default token (`changeme`) unless
  `FLAKERADAR_ALLOW_INSECURE=1` is set, and it never logs a token or a Webhook
  URL: those settings print redacted.
- **The database is trusted.** The binary creates its schema and refuses a
  database another tool made; it needs a role that can create tables on first
  start.
