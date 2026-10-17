//! Sync: devices push changes and pull everything stored after a revision.
//! One change log holds every kind; `/hosts` (see `hosts.rs`) reads and writes
//! its hosts for apps from before the other kinds.

use axum::Json;
use axum::extract::State;
use hostly_protocol::{
    Item, ItemChange, ItemChanges, ItemPushRequest, ItemPushResponse, ItemRecord, Kind,
};
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

use crate::accounts::Authenticated;
use crate::error::{ApiFailure, ApiJson, ApiQuery};

#[derive(sqlx::FromRow)]
pub(crate) struct Row {
    kind: String,
    pub id: Uuid,
    pub data: Option<Jsonb<serde_json::Value>>,
    pub modified_at: i64,
    pub revision: i64,
}

impl Row {
    /// `None` for a kind this server version doesn't know.
    fn into_record(self) -> Option<ItemRecord> {
        Some(ItemRecord {
            kind: Kind::parse(&self.kind)?,
            id: self.id,
            data: self.data.map(|data| data.0),
            modified_at: self.modified_at,
            revision: self.revision,
        })
    }
}

#[derive(Deserialize)]
pub struct Since {
    #[serde(default)]
    pub since: i64,
}

/// The account's records after `since` in revision order, optionally of one
/// kind, and the latest revision to ask from next time.
pub(crate) async fn changes_since(
    pool: &PgPool,
    account_id: Uuid,
    since: i64,
    kind: Option<Kind>,
) -> Result<(Vec<Row>, i64), ApiFailure> {
    let kind = kind.map(Kind::as_str);
    // One snapshot for both queries, so `revision` can't move past a change
    // that committed between them.
    let mut tx = pool.begin().await?;
    sqlx::query("set transaction isolation level repeatable read, read only")
        .execute(&mut *tx)
        .await?;
    let rows: Vec<Row> = sqlx::query_as(
        "select kind, id, data, modified_at, revision from items
         where account_id = $1 and revision > $2 and ($3::text is null or kind = $3)
         order by revision",
    )
    .bind(account_id)
    .bind(since)
    .bind(kind)
    .fetch_all(&mut *tx)
    .await?;
    let latest: Option<i64> = sqlx::query_scalar(
        "select max(revision) from items where account_id = $1 and ($2::text is null or kind = $2)",
    )
    .bind(account_id)
    .bind(kind)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((rows, latest.unwrap_or(since)))
}

/// Validates every change, then stores each one unless a newer change is
/// already stored. Answers with the resulting row per change, in order.
pub(crate) async fn store(
    pool: &PgPool,
    account_id: Uuid,
    changes: Vec<ItemChange>,
) -> Result<Vec<Row>, ApiFailure> {
    // Validate everything first, so a bad change stores nothing.
    for change in &changes {
        if let Some(data) = &change.data {
            Item::decode(change.kind, change.id, data.clone()).map_err(ApiFailure::invalid)?;
        }
    }

    let mut tx = pool.begin().await?;
    // Serializes this account's pushes, so its revisions commit in increasing
    // order and a pull's `since` cursor never skips one.
    sqlx::query("select 1 from accounts where id = $1 for update")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;

    let mut rows = Vec::with_capacity(changes.len());
    for change in changes {
        let kind = change.kind.as_str();
        let stored: Option<Row> = sqlx::query_as(
            "select kind, id, data, modified_at, revision from items
             where account_id = $1 and kind = $2 and id = $3",
        )
        .bind(account_id)
        .bind(kind)
        .bind(change.id)
        .fetch_optional(&mut *tx)
        .await?;
        let row = match stored {
            // The newest change wins; on a tie the incoming change does.
            Some(row) if row.modified_at > change.modified_at => row,
            _ => {
                sqlx::query_as(
                    "insert into items (account_id, kind, id, data, modified_at, revision)
                     values ($1, $2, $3, $4, $5, nextval('item_revisions'))
                     on conflict (account_id, kind, id) do update set
                         data = excluded.data,
                         modified_at = excluded.modified_at,
                         revision = excluded.revision,
                         updated_at = now()
                     returning kind, id, data, modified_at, revision",
                )
                .bind(account_id)
                .bind(kind)
                .bind(change.id)
                .bind(change.data.map(Jsonb))
                .bind(change.modified_at)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        rows.push(row);
    }
    tx.commit().await?;
    Ok(rows)
}

/// `GET /items?since=N`
pub async fn pull(
    State(pool): State<PgPool>,
    auth: Authenticated,
    ApiQuery(Since { since }): ApiQuery<Since>,
) -> Result<Json<ItemChanges>, ApiFailure> {
    let (rows, revision) = changes_since(&pool, auth.account_id, since, None).await?;
    Ok(Json(ItemChanges {
        records: rows.into_iter().filter_map(Row::into_record).collect(),
        revision,
    }))
}

/// `POST /items`
pub async fn push(
    State(pool): State<PgPool>,
    auth: Authenticated,
    ApiJson(request): ApiJson<ItemPushRequest>,
) -> Result<Json<ItemPushResponse>, ApiFailure> {
    let rows = store(&pool, auth.account_id, request.changes).await?;
    Ok(Json(ItemPushResponse {
        records: rows.into_iter().filter_map(Row::into_record).collect(),
    }))
}
