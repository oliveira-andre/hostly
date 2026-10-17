//! Upgrading a database that already holds hosts: they move into the item log
//! with their revisions, so cursors apps already hold stay valid.
//!
//! Runs in a throwaway schema of `TEST_DATABASE_URL`; without it the test
//! passes after a skip notice.

use std::str::FromStr;

use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
async fn hosts_move_into_the_item_log_with_their_revisions() {
    let _ = dotenvy::dotenv();
    let Some(url) = std::env::var("TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("skipping: TEST_DATABASE_URL is not set");
        return;
    };
    let schema = format!("migration_{}", Uuid::new_v4().simple());
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    // The schema name is generated here, never user input.
    sqlx::query(AssertSqlSafe(format!("create schema {schema}")))
        .execute(&admin)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let run = async |sql: &'static str| sqlx::raw_sql(sql).execute(&pool).await.unwrap();

    run(include_str!("../migrations/0001_init.sql")).await;
    run(include_str!("../migrations/0002_login_limits.sql")).await;
    let account = Uuid::new_v4();
    let (kept, deleted) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("insert into accounts (id, email, password_hash) values ($1, 'a@b.c', 'hash')")
        .bind(account)
        .execute(&pool)
        .await
        .unwrap();
    for (id, data) in [
        (
            kept,
            Some(r#"{"id":"x","hostname":"db","username":"me","auth":{"type":"agent"}}"#),
        ),
        (deleted, None),
    ] {
        sqlx::query(
            "insert into hosts (account_id, id, data, modified_at, revision)
             values ($1, $2, $3::jsonb, 7, nextval('host_revisions'))",
        )
        .bind(account)
        .bind(id)
        .bind(data)
        .execute(&pool)
        .await
        .unwrap();
    }

    run(include_str!("../migrations/0003_items_and_vault.sql")).await;

    let items: Vec<(String, Uuid, Option<serde_json::Value>, i64, i64)> = sqlx::query_as(
        "select kind, id, data, modified_at, revision from items
         where account_id = $1 order by revision",
    )
    .bind(account)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!((items[0].0.as_str(), items[0].1), ("host", kept));
    assert_eq!(items[0].2.as_ref().unwrap()["hostname"], "db");
    assert_eq!((items[0].3, items[0].4), (7, 1));
    assert_eq!(
        (items[1].1, items[1].2.as_ref(), items[1].4),
        (deleted, None, 2)
    );

    // New changes continue the same revision sequence.
    let next: i64 = sqlx::query_scalar("select nextval('item_revisions')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(next, 3);
    let (key_version, vault_key): (i16, Option<String>) =
        sqlx::query_as("select key_version, vault_key from accounts where id = $1")
            .bind(account)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((key_version, vault_key), (0, None));

    pool.close().await;
    sqlx::query(AssertSqlSafe(format!("drop schema {schema} cascade")))
        .execute(&admin)
        .await
        .unwrap();
}
