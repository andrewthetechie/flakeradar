#!/usr/bin/env bash
# Brings up the PostgreSQL server the Rust suite connects to, and makes the role
# and database of FLAKERADAR_TEST_DATABASE_URL exist on it.
#
# .fabro/test.toml runs this as the Test environment's prepare step, so every
# consumer of `fabro-io test-env` — .fabro/ci.sh, run_tests, task_check — gets a
# server. It needs no network: the server is either the image's own, or one
# .fabro/setup.sh installed. It is idempotent, and it never fights the sandbox
# image: the image's own helper wins wherever it exists, and a server that
# already answers the URL is left exactly as it is.
set -euo pipefail

fail() { printf 'ensure-postgres: FAIL: %s\n' "$*" >&2; exit 1; }
note() { printf 'ensure-postgres: %s\n' "$*"; }

url=${FLAKERADAR_TEST_DATABASE_URL:-}
[ -n "$url" ] || fail "FLAKERADAR_TEST_DATABASE_URL is unset; .fabro/test.toml's [env] declares it"

# postgresql://role:password@host:port/database, the spelling the suite reads.
if [[ ! "$url" =~ ^postgres(ql)?://([^:@/]+)(:([^@]*))?@([^:/]+)(:([0-9]+))?/([^/?]+) ]]; then
  fail "$url is not a postgresql://role:password@host:port/database URL"
fi
role=${BASH_REMATCH[2]}
password=${BASH_REMATCH[4]}
host=${BASH_REMATCH[5]}
port=${BASH_REMATCH[7]:-5432}
database=${BASH_REMATCH[8]}

# A name that would break out of the quoting below is a broken URL rather than a
# quoting problem, so it is refused instead of escaped.
for name in "$role" "$database"; do
  [[ "$name" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || fail "$name is not a plain SQL identifier"
done
[[ "$password" =~ ^[A-Za-z0-9_]*$ ]] || fail "the URL's password is not a plain SQL literal"

# Whether the suite can already reach its own database as its own role, which is
# exactly what a test connection does. Probing `postgres` instead would call a
# server that is missing the database ready, and the suite would then fail on
# every test that connects.
answers() {
  command -v psql >/dev/null 2>&1 \
    && PGPASSWORD=$password psql "postgresql://$role@$host:$port/$database" -tAc 'SELECT 1' >/dev/null 2>&1
}

# Whether anything listens on the host and port, provisioned or not.
port_is_open() {
  (exec 3<>"/dev/tcp/$host/$port") >/dev/null 2>&1
}

# The installed cluster that serves this port, as "<version> <name>", or nothing.
local_cluster() {
  command -v pg_lsclusters >/dev/null 2>&1 || return 0
  pg_lsclusters -h 2>/dev/null | awk -v port="$port" '$3 == port {print $1, $2; exit}'
}

# The image's own provider, used unchanged wherever the image carries it.
if command -v fabro-pg-ensure >/dev/null 2>&1; then
  exec fabro-pg-ensure
fi

if answers; then
  note "$host:$port already serves $database for $role"
  exit 0
fi

case "$host" in
localhost | 127.0.0.1 | ::1) ;;
*) fail "$host:$port does not serve $database for $role, and it is not a local server this script could provision" ;;
esac

if ! port_is_open; then
  cluster=$(local_cluster)
  [ -n "$cluster" ] || fail "nothing listens on $host:$port and no PostgreSQL serves it: run ./.fabro/setup.sh, or point FLAKERADAR_TEST_DATABASE_URL at a server"
  # `|| true`: an already-running cluster answers 2 here, and the wait below is
  # what actually decides whether the port came up.
  # shellcheck disable=SC2086 # $cluster is exactly "<version> <name>".
  pg_ctlcluster $cluster start || true
  deadline=$((SECONDS + 60))
  until port_is_open; do
    [ "$SECONDS" -lt "$deadline" ] \
      || fail "cluster $cluster did not open $host:$port within 60s; see /var/log/postgresql"
    sleep 0.5
  done
fi

[ "$(id -u)" = 0 ] \
  || fail "$host:$port does not serve $database for $role: run as root to provision the local server"
id postgres >/dev/null 2>&1 \
  || fail "$host:$port does not serve $database for $role: the server has no postgres system user"

# One statement as the server's own superuser, over the local socket. Everything
# below is idempotent, so a second run changes nothing it did not already create.
runsql() {
  su postgres -c "psql -p $port -v ON_ERROR_STOP=1 -c \"$1\"" >/dev/null
}
# Whether a catalog query returns a row.
exists() {
  [ -n "$(su postgres -c "psql -p $port -tAc \"$1\"")" ]
}

# SUPERUSER because every test clones its own database, drops it, and ends the
# pool's connections from the server side. The password is re-set on every run,
# because the URL is what the suite authenticates with; a URL without one leaves
# the role's password alone rather than clearing it.
exists "SELECT 1 FROM pg_roles WHERE rolname = '$role'" \
  || runsql "CREATE ROLE \"$role\" LOGIN SUPERUSER CREATEDB"
if [ -n "$password" ]; then
  runsql "ALTER ROLE \"$role\" LOGIN SUPERUSER CREATEDB PASSWORD '$password'"
else
  runsql "ALTER ROLE \"$role\" LOGIN SUPERUSER CREATEDB"
fi
exists "SELECT 1 FROM pg_database WHERE datname = '$database'" \
  || runsql "CREATE DATABASE \"$database\" OWNER \"$role\""

answers || fail "$host:$port is up but still refuses $role"
note "provisioned $database for $role on $host:$port"
