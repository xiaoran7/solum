\set ON_ERROR_STOP on

create extension if not exists pgcrypto;

revoke all on schema public from public;

create schema if not exists auth;
create schema if not exists sync;
create schema if not exists vault;
create schema if not exists ai;

create table if not exists auth.users (
  id uuid primary key default gen_random_uuid(),
  username text not null unique,
  password_hash text not null,
  password_salt text not null,
  password_version smallint not null default 2 constraint users_password_version_check
    check (password_version in (1, 2)),
  status text not null default 'active' check (status in ('active', 'disabled')),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

alter table auth.users add column if not exists password_version smallint;
update auth.users set password_version = 1 where password_version is null;
alter table auth.users alter column password_version set default 2;
alter table auth.users alter column password_version set not null;
do $$
begin
  if not exists (select 1 from pg_constraint
                 where conname = 'users_password_version_check'
                   and conrelid = 'auth.users'::regclass) then
    alter table auth.users add constraint users_password_version_check
      check (password_version in (1, 2)) not valid;
  end if;
end
$$;
alter table auth.users validate constraint users_password_version_check;

create table if not exists auth.refresh_tokens (
  token_hash bytea primary key,
  user_id uuid not null references auth.users(id) on delete cascade,
  session_id uuid not null default gen_random_uuid(),
  previous_token_hash bytea,
  legacy_token_hash bytea,
  rotated_at timestamptz,
  expires_at timestamptz not null,
  created_at timestamptz not null default now()
);

alter table auth.refresh_tokens add column if not exists session_id uuid;
alter table auth.refresh_tokens add column if not exists previous_token_hash bytea;
do $$
begin
  if not exists (select 1 from information_schema.columns
                 where table_schema = 'auth' and table_name = 'refresh_tokens'
                   and column_name = 'legacy_token_hash') then
    alter table auth.refresh_tokens add column legacy_token_hash bytea;
    update auth.refresh_tokens set legacy_token_hash = token_hash;
  end if;
  if not exists (select 1 from information_schema.columns
                 where table_schema = 'auth' and table_name = 'refresh_tokens'
                   and column_name = 'rotated_at') then
    alter table auth.refresh_tokens add column rotated_at timestamptz;
  end if;
end
$$;
update auth.refresh_tokens set session_id = gen_random_uuid() where session_id is null;
alter table auth.refresh_tokens alter column session_id set default gen_random_uuid();
alter table auth.refresh_tokens alter column session_id set not null;
delete from auth.refresh_tokens where expires_at <= now();
with ranked as (
  select token_hash,
         row_number() over(partition by user_id order by created_at desc, session_id desc) as rank
  from auth.refresh_tokens
)
delete from auth.refresh_tokens target using ranked
where target.token_hash = ranked.token_hash and ranked.rank > 32;

create index if not exists refresh_tokens_user_expiry_idx
  on auth.refresh_tokens (user_id, expires_at);
create index if not exists refresh_tokens_expiry_idx
  on auth.refresh_tokens (expires_at);
create unique index if not exists refresh_tokens_session_idx
  on auth.refresh_tokens (session_id);
create index if not exists refresh_tokens_previous_idx
  on auth.refresh_tokens (previous_token_hash) where previous_token_hash is not null;
create index if not exists refresh_tokens_legacy_idx
  on auth.refresh_tokens (legacy_token_hash) where legacy_token_hash is not null;

create table if not exists ai.entitlements (
  tenant_id uuid primary key references auth.users(id) on delete cascade,
  plan text not null default 'standard' check (char_length(plan) between 1 and 64),
  monthly_request_limit integer not null default 1000 check (monthly_request_limit > 0),
  custom_models_enabled boolean not null default true,
  updated_at timestamptz not null default now()
);

create table if not exists ai.usage_monthly (
  tenant_id uuid not null references auth.users(id) on delete cascade,
  period_start date not null,
  request_count integer not null default 0 check (request_count >= 0),
  success_count integer not null default 0 check (success_count >= 0),
  failure_count integer not null default 0 check (failure_count >= 0),
  fallback_count integer not null default 0 check (fallback_count >= 0),
  prompt_tokens bigint not null default 0 check (prompt_tokens >= 0),
  completion_tokens bigint not null default 0 check (completion_tokens >= 0),
  total_tokens bigint not null default 0 check (total_tokens >= 0),
  updated_at timestamptz not null default now(),
  primary key (tenant_id, period_start)
);

create table if not exists ai.usage_events (
  request_id uuid primary key,
  tenant_id uuid not null references auth.users(id) on delete cascade,
  period_start date not null,
  requested_model text not null check (char_length(requested_model) between 1 and 100),
  effective_model text not null check (char_length(effective_model) between 1 and 100),
  status text not null default 'pending' check (status in ('pending', 'succeeded', 'failed', 'interrupted')),
  fallback_occurred boolean not null default false,
  fallback_reason text check (fallback_reason is null or char_length(fallback_reason) between 1 and 128),
  upstream_latency_ms bigint check (upstream_latency_ms is null or upstream_latency_ms >= 0),
  prompt_tokens integer check (prompt_tokens is null or prompt_tokens >= 0),
  completion_tokens integer check (completion_tokens is null or completion_tokens >= 0),
  total_tokens integer check (total_tokens is null or total_tokens >= 0),
  created_at timestamptz not null default now(),
  completed_at timestamptz
);

create index if not exists usage_events_tenant_created_idx
  on ai.usage_events (tenant_id, created_at desc);

create or replace function sync.current_tenant_id()
returns uuid
language sql
stable
set search_path = ''
as $$
  select nullif(current_setting('app.current_tenant_id', true), '')::uuid
$$;

create table if not exists sync.devices (
  tenant_id uuid not null references auth.users(id) on delete cascade,
  device_id text not null check (char_length(device_id) between 1 and 128),
  public_key bytea,
  protocol_version smallint not null default 1 check (protocol_version between 1 and 32767),
  last_seen_at timestamptz not null default now(),
  revoked_at timestamptz,
  primary key (tenant_id, device_id)
);

create table if not exists sync.blobs (
  seq bigint generated always as identity primary key,
  tenant_id uuid not null references auth.users(id) on delete cascade,
  device_id text not null check (char_length(device_id) between 1 and 128),
  ciphertext bytea not null check (octet_length(ciphertext) between 1 and 8388608),
  protocol_version smallint not null default 1 check (protocol_version between 1 and 32767),
  received_at timestamptz not null default now()
);

create index if not exists blobs_tenant_seq_idx
  on sync.blobs (tenant_id, seq);
create index if not exists blobs_tenant_device_seq_idx
  on sync.blobs (tenant_id, device_id, seq);
create index if not exists blobs_received_at_idx
  on sync.blobs (received_at);

create table if not exists sync.preferences (
  tenant_id uuid not null references auth.users(id) on delete cascade,
  name text not null check (char_length(name) between 1 and 128),
  ciphertext bytea not null check (octet_length(ciphertext) between 1 and 1048576),
  version bigint not null default 1 check (version > 0),
  updated_at timestamptz not null default now(),
  primary key (tenant_id, name)
);

create table if not exists sync.alerts (
  seq bigint generated always as identity primary key,
  tenant_id uuid not null references auth.users(id) on delete cascade,
  event_id text not null check (char_length(event_id) between 1 and 256),
  source text not null check (char_length(source) between 1 and 64),
  monitor_id text,
  name text,
  status text not null check (char_length(status) between 1 and 64),
  latency_ms bigint constraint alerts_latency_ms_range
    check (latency_ms is null or latency_ms between 0 and 86400000),
  ping_latency_ms bigint constraint alerts_ping_latency_ms_range
    check (ping_latency_ms is null or ping_latency_ms between 0 and 86400000),
  availability_7d numeric(7, 4) constraint alerts_availability_7d_range
    check (availability_7d is null or availability_7d between 0 and 100),
  checked_at text,
  detail_url text constraint alerts_detail_url_scheme
    check (detail_url is null or
      (char_length(detail_url) <= 2048 and detail_url ~* '^https?://' and
       detail_url !~ E'[\\r\\n]')),
  received_at timestamptz not null default now(),
  unique (tenant_id, event_id)
);

do $$
begin
  if not exists (select 1 from pg_constraint
                 where conname = 'alerts_latency_ms_range' and conrelid = 'sync.alerts'::regclass) then
    alter table sync.alerts add constraint alerts_latency_ms_range
      check (latency_ms is null or latency_ms between 0 and 86400000) not valid;
  end if;
  if not exists (select 1 from pg_constraint
                 where conname = 'alerts_ping_latency_ms_range' and conrelid = 'sync.alerts'::regclass) then
    alter table sync.alerts add constraint alerts_ping_latency_ms_range
      check (ping_latency_ms is null or ping_latency_ms between 0 and 86400000) not valid;
  end if;
  if not exists (select 1 from pg_constraint
                 where conname = 'alerts_availability_7d_range' and conrelid = 'sync.alerts'::regclass) then
    alter table sync.alerts add constraint alerts_availability_7d_range
      check (availability_7d is null or availability_7d between 0 and 100) not valid;
  end if;
  if not exists (select 1 from pg_constraint
                 where conname = 'alerts_detail_url_scheme' and conrelid = 'sync.alerts'::regclass) then
    alter table sync.alerts add constraint alerts_detail_url_scheme
      check (detail_url is null or
        (char_length(detail_url) <= 2048 and detail_url ~* '^https?://' and
         detail_url !~ E'[\\r\\n]')) not valid;
  end if;
end
$$;

do $$
begin
  if exists (select 1 from pg_constraint
             where conrelid = 'sync.alerts'::regclass and not convalidated
               and conname = any(array[
                 'alerts_latency_ms_range', 'alerts_ping_latency_ms_range',
                 'alerts_availability_7d_range', 'alerts_detail_url_scheme'
               ])) then
    update sync.alerts set
      latency_ms = case
        when latency_ms is not null and latency_ms not between 0 and 86400000 then null
        else latency_ms
      end,
      ping_latency_ms = case
        when ping_latency_ms is not null and ping_latency_ms not between 0 and 86400000 then null
        else ping_latency_ms
      end,
      availability_7d = case
        when availability_7d is not null and
          (availability_7d not between 0 and 100 or availability_7d::text = 'NaN') then null
        else availability_7d
      end,
      detail_url = case
        when detail_url is not null and
          (char_length(detail_url) > 2048 or detail_url !~* '^https?://' or
           detail_url ~ E'[\\r\\n]') then null
        else detail_url
      end
    where (latency_ms is not null and latency_ms not between 0 and 86400000)
       or (ping_latency_ms is not null and ping_latency_ms not between 0 and 86400000)
       or (availability_7d is not null and
           (availability_7d not between 0 and 100 or availability_7d::text = 'NaN'))
       or (detail_url is not null and
           (char_length(detail_url) > 2048 or detail_url !~* '^https?://' or
            detail_url ~ E'[\\r\\n]'));

    alter table sync.alerts validate constraint alerts_latency_ms_range;
    alter table sync.alerts validate constraint alerts_ping_latency_ms_range;
    alter table sync.alerts validate constraint alerts_availability_7d_range;
    alter table sync.alerts validate constraint alerts_detail_url_scheme;
  end if;
end
$$;

create index if not exists alerts_tenant_source_seq_idx
  on sync.alerts (tenant_id, source, seq);
create index if not exists alerts_received_at_idx
  on sync.alerts (received_at);
create index if not exists alerts_tenant_received_at_idx
  on sync.alerts (tenant_id, received_at);

create table if not exists vault.key_envelopes (
  id uuid primary key default gen_random_uuid(),
  tenant_id uuid not null references auth.users(id) on delete cascade,
  recipient_device_id text not null check (char_length(recipient_device_id) between 1 and 128),
  key_version integer not null check (key_version > 0),
  algorithm text not null check (algorithm in ('x25519-xchacha20poly1305', 'recovery-xchacha20poly1305')),
  envelope bytea not null check (octet_length(envelope) between 1 and 65536),
  created_at timestamptz not null default now(),
  revoked_at timestamptz,
  unique (tenant_id, recipient_device_id, key_version, algorithm)
);

create index if not exists key_envelopes_tenant_device_idx
  on vault.key_envelopes (tenant_id, recipient_device_id, key_version desc);

-- One row per tenant: the user-chosen recovery code envelope. The envelope
-- wraps the same E2EE master key as the password envelope but under an
-- independent key domain; verify_hash is a client-computed digest of the
-- derived wrap key (never of the code itself) used only for online
-- verification. The plaintext code never reaches the server.
create table if not exists vault.recovery_codes (
  tenant_id uuid primary key references auth.users(id) on delete cascade,
  key_version integer not null check (key_version > 0),
  algorithm text not null check (algorithm = 'recovery-code-xchacha20poly1305'),
  envelope bytea not null check (octet_length(envelope) between 56 and 65536),
  verify_hash text not null check (verify_hash ~ '^[0-9a-f]{64}$'),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table if not exists vault.backup_state (
  tenant_id uuid primary key references auth.users(id) on delete cascade,
  active_generation bigint not null default 1 check (active_generation between 1 and 9007199254740991),
  last_takeover_id uuid,
  writer_id uuid,
  updated_at timestamptz not null default now()
);

alter table vault.backup_state add column if not exists writer_id uuid;

create table if not exists vault.backups (
  tenant_id uuid not null references auth.users(id) on delete cascade,
  backup_id uuid not null,
  generation bigint not null check (generation between 1 and 9007199254740991),
  protocol_version smallint not null check (protocol_version = 1),
  client_created_at timestamptz not null,
  status text not null default 'pending' check (status in ('pending', 'complete')),
  plan_sha256 bytea not null check (octet_length(plan_sha256) = 32),
  total_bytes bigint not null check (total_bytes between 1 and 2147483648),
  object_count smallint not null check (object_count between 1 and 64),
  created_at timestamptz not null default now(),
  committed_at timestamptz,
  primary key (tenant_id, backup_id),
  check ((status = 'pending' and committed_at is null) or
         (status = 'complete' and committed_at is not null))
);

create table if not exists vault.backup_objects (
  tenant_id uuid not null,
  backup_id uuid not null,
  object_id uuid not null,
  kind text not null check (kind in ('main', 'attachment')),
  declared_bytes bigint not null check (declared_bytes between 1 and 1073741824),
  declared_sha256 bytea not null check (octet_length(declared_sha256) = 32),
  ciphertext bytea,
  uploaded_at timestamptz,
  primary key (tenant_id, backup_id, object_id),
  foreign key (tenant_id, backup_id) references vault.backups(tenant_id, backup_id)
    on delete cascade,
  check ((ciphertext is null and uploaded_at is null) or
         (ciphertext is not null and uploaded_at is not null and
          octet_length(ciphertext) = declared_bytes))
);

create index if not exists backups_tenant_complete_idx
  on vault.backups (tenant_id, committed_at desc, backup_id desc)
  where status = 'complete';
create index if not exists backups_tenant_pending_idx
  on vault.backups (tenant_id, created_at)
  where status = 'pending';
create index if not exists backup_objects_tenant_backup_idx
  on vault.backup_objects (tenant_id, backup_id, object_id);

alter table sync.devices enable row level security;
alter table sync.devices force row level security;
alter table sync.blobs enable row level security;
alter table sync.blobs force row level security;
alter table sync.preferences enable row level security;
alter table sync.preferences force row level security;
alter table sync.alerts enable row level security;
alter table sync.alerts force row level security;
alter table vault.key_envelopes enable row level security;
alter table vault.key_envelopes force row level security;
alter table vault.recovery_codes enable row level security;
alter table vault.recovery_codes force row level security;
alter table vault.backup_state enable row level security;
alter table vault.backup_state force row level security;
alter table vault.backups enable row level security;
alter table vault.backups force row level security;
alter table vault.backup_objects enable row level security;
alter table vault.backup_objects force row level security;
alter table ai.entitlements enable row level security;
alter table ai.entitlements force row level security;
alter table ai.usage_monthly enable row level security;
alter table ai.usage_monthly force row level security;
alter table ai.usage_events enable row level security;
alter table ai.usage_events force row level security;

drop policy if exists devices_tenant_policy on sync.devices;
create policy devices_tenant_policy on sync.devices
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists blobs_tenant_policy on sync.blobs;
create policy blobs_tenant_policy on sync.blobs
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists preferences_tenant_policy on sync.preferences;
create policy preferences_tenant_policy on sync.preferences
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists alerts_tenant_policy on sync.alerts;
create policy alerts_tenant_policy on sync.alerts
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists key_envelopes_tenant_policy on vault.key_envelopes;
create policy key_envelopes_tenant_policy on vault.key_envelopes
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists recovery_codes_tenant_policy on vault.recovery_codes;
create policy recovery_codes_tenant_policy on vault.recovery_codes
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists backup_state_tenant_policy on vault.backup_state;
create policy backup_state_tenant_policy on vault.backup_state
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists backups_tenant_policy on vault.backups;
create policy backups_tenant_policy on vault.backups
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists backup_objects_tenant_policy on vault.backup_objects;
create policy backup_objects_tenant_policy on vault.backup_objects
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists ai_entitlements_tenant_policy on ai.entitlements;
create policy ai_entitlements_tenant_policy on ai.entitlements
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists ai_usage_monthly_tenant_policy on ai.usage_monthly;
create policy ai_usage_monthly_tenant_policy on ai.usage_monthly
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));
drop policy if exists ai_usage_events_tenant_policy on ai.usage_events;
create policy ai_usage_events_tenant_policy on ai.usage_events
  for all to solum_api
  using (tenant_id = (select sync.current_tenant_id()))
  with check (tenant_id = (select sync.current_tenant_id()));

revoke all on all tables in schema auth, sync, vault, ai from public;
revoke all on all sequences in schema auth, sync, vault, ai from public;
revoke all on all functions in schema sync from public;

-- Reapplying the migration must also repair privileges left by older releases.
-- Granting a narrower list alone never removes an obsolete DELETE/UPDATE grant.
revoke all on schema auth, sync, vault, ai from solum_api, solum_runtime;
revoke all on all tables in schema auth, sync, vault, ai from solum_api, solum_runtime;
revoke all on all sequences in schema auth, sync, vault, ai from solum_api, solum_runtime;
revoke all on all functions in schema sync from solum_api, solum_runtime;

grant usage on schema auth, sync, vault, ai to solum_api;
grant select, insert on auth.users to solum_api;
-- Column-scoped: the explicit password change endpoint may only touch the
-- credential columns, never identity or status.
grant update (password_hash, password_salt, password_version) on auth.users to solum_api;
grant select, insert, update, delete on auth.refresh_tokens to solum_api;
grant select, insert, update on sync.devices to solum_api;
grant select, insert on sync.blobs to solum_api;
grant select, insert, delete on sync.alerts to solum_api;
grant select, insert on vault.key_envelopes to solum_api;
grant select, insert, update on vault.recovery_codes to solum_api;
grant select, insert, update on vault.backup_state to solum_api;
grant select, insert, update, delete on vault.backups to solum_api;
grant select, insert, update on vault.backup_objects to solum_api;
grant select, insert on ai.entitlements to solum_api;
grant select, insert, update on ai.usage_monthly to solum_api;
grant select, insert, update on ai.usage_events to solum_api;
grant usage, select on all sequences in schema sync to solum_api;
grant execute on function sync.current_tenant_id() to solum_api;

alter default privileges in schema auth, sync, vault, ai revoke all on tables from public;
alter default privileges in schema auth, sync, vault, ai revoke all on sequences from public;
