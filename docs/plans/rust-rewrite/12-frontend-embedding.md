# 12 — Frontend embedding

Read `00-shared-context.md` first. Tasks 01–11 have landed.

Python reference: `mount_frontend` in `backend/app/main.py`, and its tests in `test_app_startup.py`.

## Goal

The binary serves the React app from memory, with no files on disk. `frontend/` stays a normal, separate Vite project with its own dev server and tests. **Do not change `frontend/src`.**

## Contracts

### Build

- Cargo never runs npm. The order is `npm run build` in `frontend/`, then `cargo build`.
- `crates/flakeradar/build.rs`: if `frontend/dist/index.html` exists, set `cfg(has_frontend)`. Otherwise warn and build an API-only binary, matching today's "serve it only when it exists". Emit `cargo:rerun-if-changed` for `frontend/dist`, and declare the cfg with `check-cfg`.
- Use `rust-embed` for the `has_frontend` case, with `debug-embed` so debug builds embed too.

### Serving

Define a small `AssetSource` trait (`get(path) -> Option<Asset>`), so routing is tested with an in-memory source. This means test results and coverage **do not depend on whether `frontend/dist` exists**. A local `dist` must not change coverage totals.

Serving happens in the fallback from task 05, ahead of its 404 step, for `GET` and `HEAD` only:

- `/` serves `index.html`. A directory path serves its `index.html`, matching `StaticFiles(html=True)`.
- Reject path traversal.
- `Content-Type` from the extension (`mime_guess`), including `.woff2`, `.js`, `.css`, `.svg`.
- An `ETag` from the embedded hash, honouring `If-None-Match` with 304.
- Files under `/assets/` get `Cache-Control: public, max-age=31536000, immutable`, because Vite hashes those names. `index.html` gets `Cache-Control: no-cache`.
- An unknown path is a 404, with no SPA fallback, because the UI keeps its state in the query string. Paths under `/api/`, `/mcp` and `/metrics` never reach assets. API 404s stay JSON.
- `POST` to a static path is 405, as Starlette's static mount does.

Dev mode needs nothing new: Vite proxies `/api` to port 8000.

## Tests to port

The `mount_frontend` tests from `test_app_startup`.

Add tests, using the in-memory source, for each rule above: both `Cache-Control` values, ETag and 304, HEAD, traversal, the 405, and API routes winning over assets.

Add one `#[cfg(has_frontend)]` test for the real embedded `index.html`. CI's smoke test in task 13 also proves the real embed, so that test may stay out of the coverage job.

## Done when

- [ ] All verification in `00-shared-context.md` passes.
- [ ] With `dist` built, the binary run from an empty temp directory serves the dashboard. Confirm it in a browser with the Playwright MCP tools, including fonts.
- [ ] Without `dist`, the build succeeds with a warning, and `/` is 404.
- [ ] Manifest rows for this task are not `pending`, and gaps are logged.
- [ ] `STATUS.md` is updated, and the branch is pushed.
