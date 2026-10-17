//! `/hosts`: the hosts in the change log, for apps from before snippets, port
//! forwards and synced secrets. They see and change only hosts.

use axum::Json;
use axum::extract::State;
use hostly_protocol::{Changes, HostRecord, ItemChange, Kind, PushRequest, PushResponse};
use sqlx::PgPool;

use crate::accounts::Authenticated;
use crate::error::{ApiFailure, ApiJson, ApiQuery};
use crate::items::{self, Row, Since};

impl TryFrom<Row> for HostRecord {
    type Error = ApiFailure;

    fn try_from(row: Row) -> Result<Self, ApiFailure> {
        Ok(Self {
            id: row.id,
            host: row
                .data
                .map(|data| serde_json::from_value(data.0))
                .transpose()
                .map_err(ApiFailure::internal)?,
            modified_at: row.modified_at,
            revision: row.revision,
        })
    }
}

/// `GET /hosts?since=N`
pub async fn pull(
    State(pool): State<PgPool>,
    auth: Authenticated,
    ApiQuery(Since { since }): ApiQuery<Since>,
) -> Result<Json<Changes>, ApiFailure> {
    let (rows, revision) =
        items::changes_since(&pool, auth.account_id, since, Some(Kind::Host)).await?;
    Ok(Json(Changes {
        records: rows
            .into_iter()
            .map(HostRecord::try_from)
            .collect::<Result<_, _>>()?,
        revision,
    }))
}

/// `POST /hosts`: stores each change unless a newer one is already stored, and
/// answers with the resulting record for every change, in request order.
pub async fn push(
    State(pool): State<PgPool>,
    auth: Authenticated,
    ApiJson(request): ApiJson<PushRequest>,
) -> Result<Json<PushResponse>, ApiFailure> {
    let changes = request
        .changes
        .into_iter()
        .map(|change| ItemChange {
            kind: Kind::Host,
            id: change.id,
            data: change
                .host
                .map(|host| serde_json::to_value(host).expect("hosts always serialize")),
            modified_at: change.modified_at,
        })
        .collect();
    let rows = items::store(&pool, auth.account_id, changes).await?;
    Ok(Json(PushResponse {
        records: rows
            .into_iter()
            .map(HostRecord::try_from)
            .collect::<Result<_, _>>()?,
    }))
}
