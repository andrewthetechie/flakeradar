# Vendored assets

## `redoc.standalone.js`

The Redoc browser bundle that `/docs` loads. It is committed rather than fetched
at build or run time, so viewing the API documentation needs no network beyond
the instance itself and a release binary is reproducible.

| | |
|---|---|
| Package | [`redoc`](https://www.npmjs.com/package/redoc) |
| Version | 2.5.0 (pinned) |
| Source URL | <https://unpkg.com/redoc@2.5.0/bundles/redoc.standalone.js> |
| Size | 910994 bytes |
| SHA-256 | `0ec05be285ac885a330289b02f470e1bdbd2b6b3223a9fa213f24bf805a851d1` |
| Licence | MIT, Copyright (c) 2015-present Rebilly, Inc. See `NOTICE` |

To upgrade, download the new pinned URL, check the SHA-256 against the release,
replace the file and update this table in the same commit. The served path does
not change: `flakeradar::docs::REDOC_SCRIPT_PATH`.
