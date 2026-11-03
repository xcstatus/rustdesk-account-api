#!/bin/sh
# Runs only on a new PostgreSQL cluster. Never echo secret values.
set -eu
RD_INIT_APP_PASSWORD=$(cat /run/secrets/app_password)
test -n "$RD_INIT_APP_PASSWORD"
export RD_INIT_APP_PASSWORD
psql --set=ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<'SQL'
\getenv app_password RD_INIT_APP_PASSWORD
CREATE ROLE rustdesk LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD :'app_password';
ALTER DATABASE rustdesk OWNER TO rustdesk;
SQL
unset RD_INIT_APP_PASSWORD
