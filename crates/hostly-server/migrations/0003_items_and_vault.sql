-- One change log for every synced kind: hosts, snippets, port forwards and
-- sealed secrets. Existing hosts move over with their revisions, so cursors
-- that apps already hold stay valid.
create table items (
    account_id uuid not null references accounts (id) on delete cascade,
    -- 'host', 'snippet', 'forward' or 'secret' (see hostly_protocol::Kind).
    kind text not null,
    id uuid not null,
    -- The item as the app sends it; null once deleted (a tombstone). Secrets
    -- are sealed on the device, so the server can't read them.
    data jsonb,
    -- When the change was made, by the device's clock (ms since the Unix epoch).
    modified_at bigint not null,
    revision bigint not null,
    updated_at timestamptz not null default now(),
    primary key (account_id, kind, id)
);

create index items_account_revision on items (account_id, revision);

insert into items (account_id, kind, id, data, modified_at, revision, updated_at)
    select account_id, 'host', id, data, modified_at, revision, updated_at from hosts;

drop table hosts;

alter sequence host_revisions rename to item_revisions;

-- End-to-end encryption. key_version 0: password_hash is of the password as
-- typed. 1: of a login key derived from it on the device, and vault_key holds
-- the account's vault key wrapped with another key the server never sees.
alter table accounts
    add column key_version smallint not null default 0,
    add column vault_key text;
