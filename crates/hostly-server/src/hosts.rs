//! Host sync: devices push changes and pull everything stored after a revision.

use axum::Json;
use axum::extract::State;
use hostly_protocol::{Changes, Host, HostRecord, PushRequest, PushResponse};
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

use crate::accounts::Authenticated;
use crate::error::{ApiFailure, ApiJson, ApiQuery};

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    data: Option<Jsonb<Host>>,
    modified_at: i64,
    revision: i64,
}

impl From<Row> for HostRecord {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            host: row.data.map(|data| data.0),
            modified_at: row.modified_at,
            revision: row.revision,
        }
    }
}

#[derive(Deserialize)]
pub struct Since {
    #[serde(default)]
    since: i64,
}

/// `GET /hosts?since=N`
pub async fn pull(
    State(pool): State<PgPool>,
    auth: Authenticated,
    ApiQuery(Since { since }): ApiQuery<Since>,
) -> Result<Json<Changes>, ApiFailure> {
    // One snapshot for both queries, so `revision` can't move past a change
    // that committed between them.
    let mut tx = pool.begin().await?;
    sqlx::query("set transaction isolation level repeatable read, read only")
        .execute(&mut *tx)
        .await?;
    let rows: Vec<Row> = sqlx::query_as(
        "select id, data, modified_at, revision from hosts
         where account_id = $1 and revision > $2 order by revision",
    )
    .bind(auth.account_id)
    .bind(since)
    .fetch_all(&mut *tx)
    .await?;
    let latest: Option<i64> =
        sqlx::query_scalar("select max(revision) from hosts where account_id = $1")
            .bind(auth.account_id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(Json(Changes {
        records: rows.into_iter().map(HostRecord::from).collect(),
        revision: latest.unwrap_or(since),
    }))
}

/// `POST /hosts`: stores each change unless a newer one is already stored, and
/// answers with the resulting record for every change, in request order.
pub async fn push(
    State(pool): State<PgPool>,
    auth: Authenticated,
    ApiJson(request): ApiJson<PushRequest>,
) -> Result<Json<PushResponse>, ApiFailure> {
    // Validate everything first, so a bad change stores nothing.
    for change in &request.changes {
        if let Some(host) = &change.host {
            if host.id != change.id {
                return Err(ApiFailure::invalid(format!(
                    "host id {} does not match change id {}",
                    host.id, change.id
                )));
            }
            host.validate()
                .map_err(|reason| ApiFailure::invalid(format!("host {}: {reason}", change.id)))?;
        }
    }

    let mut tx = pool.begin().await?;
    // Serializes this account's pushes, so its revisions commit in increasing
    // order and a pull's `since` cursor never skips one.
    sqlx::query("select 1 from accounts where id = $1 for update")
        .bind(auth.account_id)
        .execute(&mut *tx)
        .await?;

    let mut records = Vec::with_capacity(request.changes.len());
    for change in request.changes {
        let stored: Option<Row> = sqlx::query_as(
            "select id, data, modified_at, revision from hosts where account_id = $1 and id = $2",
        )
        .bind(auth.account_id)
        .bind(change.id)
        .fetch_optional(&mut *tx)
        .await?;
        let row = match stored {
            // The newest change wins; on a tie the incoming change does.
            Some(row) if row.modified_at > change.modified_at => row,
            _ => {
                sqlx::query_as(
                    "insert into hosts (account_id, id, data, modified_at, revision)
                     values ($1, $2, $3, $4, nextval('host_revisions'))
                     on conflict (account_id, id) do update set
                         data = excluded.data,
                         modified_at = excluded.modified_at,
                         revision = excluded.revision,
                         updated_at = now()
                     returning id, data, modified_at, revision",
                )
                .bind(auth.account_id)
                .bind(change.id)
                .bind(change.host.map(Jsonb))
                .bind(change.modified_at)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        records.push(row.into());
    }
    tx.commit().await?;
    Ok(Json(PushResponse { records }))
}
