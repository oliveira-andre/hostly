create table accounts (
    id uuid primary key,
    -- Stored trimmed and lowercased, so uniqueness ignores case.
    email text not null unique,
    password_hash text not null,
    created_at timestamptz not null default now()
);

create table sessions (
    -- SHA-256 of the bearer token; the token itself is never stored.
    token_hash bytea primary key,
    account_id uuid not null references accounts (id) on delete cascade,
    created_at timestamptz not null default now(),
    last_used_at timestamptz not null default now()
);

create index sessions_account_id on sessions (account_id);

-- Orders stored host changes; clients pull with `since = <last revision seen>`.
create sequence host_revisions;

create table hosts (
    account_id uuid not null references accounts (id) on delete cascade,
    id uuid not null,
    -- The host as the app sends it; null once deleted (a tombstone).
    data jsonb,
    -- When the change was made, by the device's clock (ms since the Unix epoch).
    modified_at bigint not null,
    revision bigint not null,
    updated_at timestamptz not null default now(),
    primary key (account_id, id)
);

create index hosts_account_revision on hosts (account_id, revision);
