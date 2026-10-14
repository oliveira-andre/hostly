-- Failed log-ins since the last successful one. Every `account_failures` of them
-- (see `LoginLimits`) lock the account until `locked_until`.
alter table accounts
    add column failed_logins integer not null default 0,
    add column locked_until timestamptz;
