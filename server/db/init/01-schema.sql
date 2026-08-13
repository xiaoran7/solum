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
  status text not null default 'active' check (status in ('active', 'disabled')),
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table if not exists auth.refresh_tokens (
  token_hash bytea primary key,
  user_id uuid not null references auth.users(id) on delete cascade,
  expires_at timestamptz not null,
  created_at timestamptz not null default now()
);

create index if not exists refresh_tokens_user_expiry_idx
  on auth.refresh_tokens (user_id, expires_at);
create index if not exists refresh_tokens_expiry_idx
  on auth.refresh_tokens (expires_at);

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
  latency_ms bigint,
  ping_latency_ms bigint,
  availability_7d numeric(7, 4),
  checked_at text,
  detail_url text,
  received_at timestamptz not null default now(),
  unique (tenant_id, event_id)
);

create index if not exists alerts_tenant_source_seq_idx
  on sync.alerts (tenant_id, source, seq);
create index if not exists alerts_received_at_idx
  on sync.alerts (received_at);

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

grant usage on schema auth, sync, vault, ai to solum_api;
grant select, insert, update on auth.users to solum_api;
grant select, insert, delete on auth.refresh_tokens to solum_api;
grant select, insert, update on sync.devices to solum_api;
grant select, insert, delete on sync.blobs to solum_api;
grant select, insert, update on sync.preferences to solum_api;
grant select, insert on sync.alerts to solum_api;
grant select, insert, update on vault.key_envelopes to solum_api;
revoke update, delete on ai.entitlements from solum_api;
grant select, insert on ai.entitlements to solum_api;
grant select, insert, update on ai.usage_monthly to solum_api;
grant select, insert, update on ai.usage_events to solum_api;
grant usage, select on all sequences in schema sync to solum_api;
grant execute on function sync.current_tenant_id() to solum_api;

alter default privileges in schema auth, sync, vault, ai revoke all on tables from public;
alter default privileges in schema auth, sync, vault, ai revoke all on sequences from public;
