#!/bin/sh
set -eu

: "${SOLUM_DB_APP_PASSWORD:?SOLUM_DB_APP_PASSWORD is required}"

psql --set=ON_ERROR_STOP=1 \
  --username "$POSTGRES_USER" \
  --dbname "$POSTGRES_DB" \
  --set=app_password="$SOLUM_DB_APP_PASSWORD" <<'SQL'
select 'create role solum_api nologin nosuperuser nocreatedb nocreaterole noinherit'
where not exists (select 1 from pg_roles where rolname = 'solum_api') \gexec

select format(
  'create role solum_runtime login password %L nosuperuser nocreatedb nocreaterole inherit',
  :'app_password'
)
where not exists (select 1 from pg_roles where rolname = 'solum_runtime') \gexec

select format('alter role solum_runtime password %L', :'app_password') \gexec
grant solum_api to solum_runtime;
alter role solum_runtime set statement_timeout = '30s';
alter role solum_runtime set idle_in_transaction_session_timeout = '15s';
SQL
