// Copyright 2026 OpenObserve Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter, Schema, Set,
    TransactionTrait,
    sea_query::{Expr, OnConflict},
};
use tokio::sync::OnceCell;

use super::entity::oidc_records::{ActiveModel, Column, Entity, Model};
use crate::db::get_orm_client_rw;

static SCHEMA: OnceCell<()> = OnceCell::const_new();

pub async fn initialize() -> Result<(), sea_orm::DbErr> {
    SCHEMA
        .get_or_try_init(|| async {
            let client = get_orm_client_rw().await;
            let backend = client.get_database_backend();
            client
                .execute(
                    backend.build(
                        &Schema::new(backend)
                            .create_table_from_entity(Entity)
                            .if_not_exists()
                            .take(),
                    ),
                )
                .await?;
            Ok::<(), sea_orm::DbErr>(())
        })
        .await?;
    Ok(())
}

pub async fn insert(record: Model) -> Result<(), sea_orm::DbErr> {
    initialize().await?;
    let client = get_orm_client_rw().await;
    if record.kind == "transaction" {
        let transaction = client.begin().await?;
        reserve_transaction(&transaction, &record).await?;
        transaction.commit().await?;
    } else {
        insert_on(client, record).await?;
    }
    Ok(())
}

pub async fn get(handle: &str, kind: &str) -> Result<Option<Model>, sea_orm::DbErr> {
    initialize().await?;
    Entity::find_by_id(handle)
        .filter(Column::Kind.eq(kind))
        .one(get_orm_client_rw().await)
        .await
}

pub async fn consume(handle: &str, browser: &str, now: i64) -> Result<bool, sea_orm::DbErr> {
    consume_on(get_orm_client_rw().await, handle, browser, now).await
}

pub async fn touch(handle: &str, now: i64, idle: i64) -> Result<bool, sea_orm::DbErr> {
    touch_on(get_orm_client_rw().await, handle, now, idle).await
}

pub async fn revoke(handle: &str) -> Result<(), sea_orm::DbErr> {
    initialize().await?;
    Entity::delete_by_id(handle)
        .exec(get_orm_client_rw().await)
        .await?;
    Ok(())
}

pub async fn capacity(browser: &str, now: i64) -> Result<bool, sea_orm::DbErr> {
    initialize().await?;
    let client = get_orm_client_rw().await;
    Entity::delete_many()
        .filter(Column::ExpiresAt.lte(now))
        .exec(client)
        .await?;
    let count = Entity::find()
        .filter(Column::Kind.eq("transaction"))
        .filter(Column::Browser.eq(browser))
        .filter(Column::Consumed.eq(false))
        .count(client)
        .await?;
    Ok(count < 8)
}

async fn insert_on<C: ConnectionTrait>(client: &C, record: Model) -> Result<(), sea_orm::DbErr> {
    Entity::insert(ActiveModel {
        handle: Set(record.handle),
        kind: Set(record.kind),
        browser: Set(record.browser),
        payload: Set(record.payload),
        expires_at: Set(record.expires_at),
        touched_at: Set(record.touched_at),
        consumed: Set(false),
    })
    .exec(client)
    .await?;
    Ok(())
}

async fn reserve_transaction(
    client: &sea_orm::DatabaseTransaction,
    record: &Model,
) -> Result<(), sea_orm::DbErr> {
    // Writing the browser guard serializes admission across PostgreSQL replicas and SQLite writers.
    Entity::insert(ActiveModel {
        handle: Set(format!("guard:{}", record.browser)),
        kind: Set("guard".into()),
        browser: Set(record.browser.clone()),
        payload: Set(String::new()),
        expires_at: Set(record.expires_at),
        touched_at: Set(record.touched_at),
        consumed: Set(false),
    })
    .on_conflict(
        OnConflict::column(Column::Handle)
            .update_columns([Column::TouchedAt, Column::ExpiresAt])
            .to_owned(),
    )
    .exec_without_returning(client)
    .await?;
    let count = Entity::find()
        .filter(Column::Kind.eq("transaction"))
        .filter(Column::Browser.eq(&record.browser))
        .filter(Column::Consumed.eq(false))
        .filter(Column::ExpiresAt.gt(record.touched_at))
        .count(client)
        .await?;
    if count >= 8 {
        return Err(sea_orm::DbErr::Custom("OIDC transaction capacity".into()));
    }
    insert_on(client, record.clone()).await
}

async fn consume_on(
    client: &sea_orm::DatabaseConnection,
    handle: &str,
    browser: &str,
    now: i64,
) -> Result<bool, sea_orm::DbErr> {
    let result = Entity::update_many()
        .col_expr(Column::Consumed, Expr::value(true))
        .filter(Column::Handle.eq(handle))
        .filter(Column::Kind.eq("transaction"))
        .filter(Column::Browser.eq(browser))
        .filter(Column::Consumed.eq(false))
        .filter(Column::ExpiresAt.gt(now))
        .exec(client)
        .await?;
    Ok(result.rows_affected == 1)
}

async fn touch_on(
    client: &sea_orm::DatabaseConnection,
    handle: &str,
    now: i64,
    idle: i64,
) -> Result<bool, sea_orm::DbErr> {
    let result = Entity::update_many()
        .col_expr(Column::TouchedAt, Expr::value(now))
        .filter(Column::Handle.eq(handle))
        .filter(Column::Kind.eq("session"))
        .filter(Column::ExpiresAt.gt(now))
        .filter(Column::TouchedAt.gt(now - idle))
        .filter(Column::Consumed.eq(false))
        .exec(client)
        .await?;
    Ok(result.rows_affected == 1)
}

#[cfg(test)]
mod tests {
    use sea_orm::{Database, DatabaseConnection};

    use super::*;

    async fn database() -> DatabaseConnection {
        let client = Database::connect("sqlite::memory:").await.unwrap();
        let backend = client.get_database_backend();
        client
            .execute(backend.build(&Schema::new(backend).create_table_from_entity(Entity)))
            .await
            .unwrap();
        client
    }

    async fn record(
        client: &DatabaseConnection,
        handle: &str,
        kind: &str,
        expires: i64,
        touched: i64,
    ) {
        Entity::insert(ActiveModel {
            handle: Set(handle.into()),
            kind: Set(kind.into()),
            browser: Set("browser-a".into()),
            payload: Set("{}".into()),
            expires_at: Set(expires),
            touched_at: Set(touched),
            consumed: Set(false),
        })
        .exec(client)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn admission_reserves_at_most_eight_pending_transactions_per_browser() {
        let client = database().await;
        for slot in 0..8 {
            let transaction = client.begin().await.unwrap();
            reserve_transaction(
                &transaction,
                &Model {
                    handle: format!("state-{slot}"),
                    kind: "transaction".into(),
                    browser: "browser-a".into(),
                    payload: "{}".into(),
                    expires_at: 400,
                    touched_at: 100,
                    consumed: false,
                },
            )
            .await
            .unwrap();
            transaction.commit().await.unwrap();
        }
        let transaction = client.begin().await.unwrap();
        assert!(
            reserve_transaction(
                &transaction,
                &Model {
                    handle: "ninth".into(),
                    kind: "transaction".into(),
                    browser: "browser-a".into(),
                    payload: "{}".into(),
                    expires_at: 400,
                    touched_at: 100,
                    consumed: false
                }
            )
            .await
            .is_err()
        );
        transaction.rollback().await.unwrap();
        assert!(
            consume_on(&client, "state-0", "browser-a", 101)
                .await
                .unwrap()
        );
        let transaction = client.begin().await.unwrap();
        reserve_transaction(
            &transaction,
            &Model {
                handle: "replacement".into(),
                kind: "transaction".into(),
                browser: "browser-a".into(),
                payload: "{}".into(),
                expires_at: 400,
                touched_at: 101,
                consumed: false,
            },
        )
        .await
        .unwrap();
        transaction.commit().await.unwrap();
    }

    #[tokio::test]
    async fn replay_authority_and_revocation_survive_reconnect_and_a_second_connection() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("session.sqlite").display()
        );
        let first = Database::connect(&url).await.unwrap();
        let backend = first.get_database_backend();
        first
            .execute(backend.build(&Schema::new(backend).create_table_from_entity(Entity)))
            .await
            .unwrap();
        record(&first, "state", "transaction", 400, 100).await;
        record(&first, "session", "session", 400, 100).await;
        first.close().await.unwrap();
        let restarted = Database::connect(&url).await.unwrap();
        let replica = Database::connect(&url).await.unwrap();
        let (a, b) = tokio::join!(
            consume_on(&restarted, "state", "browser-a", 100),
            consume_on(&replica, "state", "browser-a", 100)
        );
        assert_ne!(a.unwrap(), b.unwrap());
        assert!(
            !consume_on(&replica, "state", "browser-a", 101)
                .await
                .unwrap()
        );
        assert!(touch_on(&restarted, "session", 150, 60).await.unwrap());
        Entity::delete_by_id("session")
            .exec(&replica)
            .await
            .unwrap();
        assert!(!touch_on(&restarted, "session", 151, 60).await.unwrap());
        restarted.close().await.unwrap();
        replica.close().await.unwrap();
    }

    #[tokio::test]
    async fn transaction_has_one_winner_across_concurrent_consumers() {
        let client = database().await;
        record(&client, "state", "transaction", 400, 100).await;
        assert!(
            !consume_on(&client, "state", "browser-b", 100)
                .await
                .unwrap()
        );
        let (a, b) = tokio::join!(
            consume_on(&client, "state", "browser-a", 100),
            consume_on(&client, "state", "browser-a", 100)
        );
        assert_ne!(a.unwrap(), b.unwrap());
        assert!(
            !consume_on(&client, "state", "browser-a", 101)
                .await
                .unwrap()
        );
        record(&client, "expired", "transaction", 100, 100).await;
        assert!(
            !consume_on(&client, "expired", "browser-a", 100)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn idle_absolute_expiry_and_revocation_cannot_resurrect_a_session() {
        let client = database().await;
        record(&client, "session", "session", 400, 100).await;
        assert!(touch_on(&client, "session", 150, 60).await.unwrap());
        assert!(!touch_on(&client, "session", 210, 60).await.unwrap());
        record(&client, "absolute", "session", 200, 190).await;
        assert!(!touch_on(&client, "absolute", 200, 60).await.unwrap());
        Entity::delete_by_id("session").exec(&client).await.unwrap();
        assert!(!touch_on(&client, "session", 151, 60).await.unwrap());
    }
}
