#!/bin/bash
# usage: mut.sh label filter
S=/tmp/sp2
cd /Users/andrew/Documents/code/flakeradar
echo "=== $1"
CARGO_INCREMENTAL=0 cargo nextest run --config-file $S/nt.toml -p flakeradar --test github_worker --test github --test webhooks -E "$2" --no-tests=warn 2>&1 | grep -E "^\s+(PASS|FAIL|TIMEOUT|SIGABRT|SIGKILL|TERMINATING)|Summary|^error(\[|:)" | cut -c1-120 | sort -u
