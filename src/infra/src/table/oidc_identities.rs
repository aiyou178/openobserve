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

use config::{
    ider,
    meta::user::{UserRole, UserType},
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait, QueryFilter,
    Schema, Set, TransactionTrait,
    sea_query::{Expr, Func, OnConflict},
};
use tokio::sync::OnceCell;

use super::{
    entity::{
        oidc_identities::{ActiveModel, Column, Entity},
        org_users, organizations, users,
    },
    users::UserRecord,
};
use crate::db::get_orm_client_rw;

static SCHEMA: OnceCell<()> = OnceCell::const_new();

const DISABLED_PASSWORD_HASH: &str = "!";

pub struct Profile {
    pub email: String,
    pub first_name: String,
    pub last_name: String,
}

pub async fn initialize() -> Result<(), DbErr> {
    SCHEMA
        .get_or_try_init(|| async { create_schema(get_orm_client_rw().await).await })
        .await?;
    Ok(())
}

pub async fn get(issuer: &str, subject: &str) -> Result<(String, UserRecord), DbErr> {
    initialize().await?;
    get_on(get_orm_client_rw().await, issuer, subject).await
}

pub async fn provision(
    issuer: &str,
    subject: &str,
    profile: &Profile,
    organizations: &[String],
) -> Result<(String, UserRecord), DbErr> {
    initialize().await?;
    provision_on(
        get_orm_client_rw().await,
        issuer,
        subject,
        profile,
        organizations,
    )
    .await
}

async fn create_schema(client: &DatabaseConnection) -> Result<(), DbErr> {
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
    Ok(())
}

async fn get_on<C: ConnectionTrait>(
    client: &C,
    issuer: &str,
    subject: &str,
) -> Result<(String, UserRecord), DbErr> {
    let mapping = Entity::find_by_id((issuer.to_owned(), subject.to_owned()))
        .one(client)
        .await?
        .ok_or_else(|| DbErr::Custom("OIDC identity not found".into()))?;
    // Retaining a deleted user's mapping prevents a later login from resurrecting access.
    let user = users::Entity::find_by_id(mapping.user_id)
        .one(client)
        .await?
        .ok_or_else(|| DbErr::Custom("OIDC user not found".into()))?;
    if user.is_root || user.user_type != i16::from(UserType::External) {
        return Err(DbErr::Custom("Invalid OIDC user".into()));
    }
    Ok((user.id.clone(), user.into()))
}

async fn provision_on(
    client: &DatabaseConnection,
    issuer: &str,
    subject: &str,
    profile: &Profile,
    default_organizations: &[String],
) -> Result<(String, UserRecord), DbErr> {
    let transaction = client.begin().await?;
    let candidate_id = ider::uuid();
    // The upsert serializes first logins before reads on SQLite and PostgreSQL.
    Entity::insert(ActiveModel {
        issuer: Set(issuer.to_owned()),
        subject: Set(subject.to_owned()),
        user_id: Set(candidate_id.clone()),
    })
    .on_conflict(
        OnConflict::columns([Column::Issuer, Column::Subject])
            .update_column(Column::Subject)
            .to_owned(),
    )
    .exec_without_returning(&transaction)
    .await?;
    let mapping = Entity::find_by_id((issuer.to_owned(), subject.to_owned()))
        .one(&transaction)
        .await?
        .ok_or_else(|| DbErr::Custom("OIDC identity not found".into()))?;
    if mapping.user_id == candidate_id {
        insert_user(&transaction, candidate_id, profile, default_organizations).await?;
    }
    let result = get_on(&transaction, issuer, subject).await?;
    transaction.commit().await?;
    Ok(result)
}

async fn insert_user(
    client: &DatabaseTransaction,
    user_id: String,
    profile: &Profile,
    default_organizations: &[String],
) -> Result<(), DbErr> {
    if default_organizations.is_empty()
        || users::Entity::find()
            .filter(
                Expr::expr(Func::lower(Expr::col(users::Column::Email)))
                    .eq(profile.email.to_lowercase()),
            )
            .one(client)
            .await?
            .is_some()
    {
        return Err(DbErr::Custom("OIDC provisioning denied".into()));
    }
    let now = chrono::Utc::now().timestamp_micros();
    users::Entity::insert(users::ActiveModel {
        id: Set(user_id),
        email: Set(profile.email.clone()),
        first_name: Set(profile.first_name.clone()),
        last_name: Set(profile.last_name.clone()),
        password: Set(DISABLED_PASSWORD_HASH.into()),
        salt: Set(ider::uuid()),
        password_ext: Set(None),
        is_root: Set(false),
        user_type: Set(UserType::External.into()),
        created_at: Set(now),
        updated_at: Set(now),
        must_reset_password: Set(false),
        password_reset_reason: Set(None),
        flagged_at: Set(None),
        password_updated_at: Set(None),
    })
    .exec(client)
    .await?;
    for org in default_organizations {
        let organization = organizations::Entity::find_by_id(org)
            .one(client)
            .await?
            .ok_or_else(|| DbErr::Custom("OIDC organization not found".into()))?;
        if org == config::META_ORG_ID
            || organization.status != "active"
            || organization.deleted_at.is_some()
        {
            return Err(DbErr::Custom("OIDC organization unavailable".into()));
        }
        org_users::Entity::insert(org_users::ActiveModel {
            id: Set(ider::uuid()),
            email: Set(profile.email.clone()),
            org_id: Set(org.clone()),
            role: Set(UserRole::Admin.into()),
            token: Set(String::new()),
            rum_token: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            allow_static_token: Set(false),
        })
        .exec(client)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sea_orm::{ColumnTrait, ConnectOptions, Database, PaginatorTrait};

    use super::*;

    async fn connect(url: &str) -> DatabaseConnection {
        let mut options = ConnectOptions::new(url);
        options.max_connections(1).sqlx_logging(false);
        Database::connect(options).await.unwrap()
    }

    async fn database(url: &str) -> DatabaseConnection {
        let client = connect(url).await;
        let backend = client.get_database_backend();
        let schema = Schema::new(backend);
        for statement in [
            schema.create_table_from_entity(users::Entity),
            schema.create_table_from_entity(organizations::Entity),
            schema.create_table_from_entity(org_users::Entity),
        ] {
            client.execute(backend.build(&statement)).await.unwrap();
        }
        create_schema(&client).await.unwrap();
        for identifier in ["first", "second", config::META_ORG_ID] {
            organizations::Entity::insert(organizations::ActiveModel {
                identifier: Set(identifier.into()),
                org_name: Set(identifier.into()),
                org_type: Set(0),
                status: Set("active".into()),
                created_at: Set(1),
                updated_at: Set(1),
                deleted_at: Set(None),
                #[cfg(feature = "cloud")]
                trial_ends_at: Set(100),
            })
            .exec(&client)
            .await
            .unwrap();
        }
        client
    }

    fn profile(email: &str) -> Profile {
        Profile {
            email: email.into(),
            first_name: "OIDC".into(),
            last_name: "User".into(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_first_login_and_reconnect_reuse_the_durable_subject() {
        let directory = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("identities.sqlite").display()
        );
        let first = database(&url).await;
        let replica = connect(&url).await;
        let defaults = vec!["first".into(), "second".into()];
        let original = profile("original@example.com");
        let (a, b) = tokio::join!(
            provision_on(
                &first,
                "https://idp.example",
                "subject",
                &original,
                &defaults
            ),
            provision_on(
                &replica,
                "https://idp.example",
                "subject",
                &original,
                &defaults
            ),
        );
        let (id, user) = a.unwrap();
        assert_eq!(id, b.unwrap().0);
        assert!(user.user_type.is_external());
        assert!(!user.is_root);
        assert_eq!(users::Entity::find().count(&first).await.unwrap(), 1);
        assert_eq!(Entity::find().count(&first).await.unwrap(), 1);
        let memberships = org_users::Entity::find().all(&first).await.unwrap();
        assert_eq!(memberships.len(), 2);
        assert!(
            memberships
                .iter()
                .all(|org| org.role == i16::from(UserRole::Admin)
                    && !org.allow_static_token
                    && org.token.is_empty()
                    && org.rum_token.is_none())
        );
        first.close().await.unwrap();
        replica.close().await.unwrap();
        let restarted = connect(&url).await;
        let (again, unchanged) = provision_on(
            &restarted,
            "https://idp.example",
            "subject",
            &profile("changed@example.com"),
            &["missing-new-default".into()],
        )
        .await
        .unwrap();
        assert_eq!(again, id);
        assert_eq!(unchanged.email, "original@example.com");
        restarted.close().await.unwrap();
    }

    #[tokio::test]
    async fn defaults_are_not_restored_and_deleted_accounts_are_not_recreated() {
        let client = database("sqlite::memory:").await;
        let defaults = vec!["first".into()];
        let original = profile("removed@example.com");
        let (id, _) = provision_on(&client, "issuer", "subject", &original, &defaults)
            .await
            .unwrap();
        org_users::Entity::delete_many()
            .exec(&client)
            .await
            .unwrap();
        assert!(
            provision_on(&client, "issuer", "subject", &original, &defaults)
                .await
                .is_ok()
        );
        assert_eq!(org_users::Entity::find().count(&client).await.unwrap(), 0);
        users::Entity::delete_by_id(&id)
            .exec(&client)
            .await
            .unwrap();
        assert!(
            provision_on(&client, "issuer", "subject", &original, &defaults)
                .await
                .is_err()
        );
        assert_eq!(Entity::find().count(&client).await.unwrap(), 1);
        assert_eq!(users::Entity::find().count(&client).await.unwrap(), 0);
        let (replacement, _) =
            provision_on(&client, "issuer", "different-subject", &original, &defaults)
                .await
                .unwrap();
        assert_ne!(replacement, id);
        assert!(get_on(&client, "issuer", "subject").await.is_err());
    }

    #[tokio::test]
    async fn every_default_must_be_available_or_the_entire_creation_rolls_back() {
        let client = database("sqlite::memory:").await;
        organizations::Entity::update_many()
            .col_expr(organizations::Column::Status, Expr::value("deleting"))
            .filter(organizations::Column::Identifier.eq("second"))
            .exec(&client)
            .await
            .unwrap();
        for defaults in [
            vec![],
            vec!["first".into(), "missing".into()],
            vec!["first".into(), "second".into()],
            vec![config::META_ORG_ID.into()],
        ] {
            assert!(
                provision_on(
                    &client,
                    "issuer",
                    "subject",
                    &profile("new@example.com"),
                    &defaults
                )
                .await
                .is_err()
            );
            assert_eq!(users::Entity::find().count(&client).await.unwrap(), 0);
            assert_eq!(org_users::Entity::find().count(&client).await.unwrap(), 0);
            assert_eq!(Entity::find().count(&client).await.unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn email_collisions_never_link_or_overwrite_an_existing_account() {
        let client = database("sqlite::memory:").await;
        let defaults = vec!["first".into()];
        let original = profile("existing@example.com");
        let (id, _) = provision_on(&client, "issuer", "original", &original, &defaults)
            .await
            .unwrap();
        users::Entity::update_many()
            .col_expr(users::Column::IsRoot, Expr::value(true))
            .filter(users::Column::Id.eq(&id))
            .exec(&client)
            .await
            .unwrap();
        assert!(
            provision_on(
                &client,
                "issuer",
                "attacker",
                &profile("EXISTING@example.com"),
                &defaults
            )
            .await
            .is_err()
        );
        assert_eq!(Entity::find().count(&client).await.unwrap(), 1);
        assert_eq!(users::Entity::find().count(&client).await.unwrap(), 1);
        assert!(
            users::Entity::find_by_id(&id)
                .one(&client)
                .await
                .unwrap()
                .unwrap()
                .is_root
        );
        assert!(get_on(&client, "issuer", "original").await.is_err());
    }

    #[tokio::test]
    async fn issuer_and_subject_both_separate_accounts() {
        let client = database("sqlite::memory:").await;
        let defaults = vec!["first".into()];
        let (a, _) = provision_on(
            &client,
            "issuer-a",
            "subject",
            &profile("one@example.com"),
            &defaults,
        )
        .await
        .unwrap();
        let (b, _) = provision_on(
            &client,
            "issuer-b",
            "subject",
            &profile("two@example.com"),
            &defaults,
        )
        .await
        .unwrap();
        let (c, _) = provision_on(
            &client,
            "issuer-a",
            "other",
            &profile("three@example.com"),
            &defaults,
        )
        .await
        .unwrap();
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
        assert_eq!(Entity::find().count(&client).await.unwrap(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires an isolated PostgreSQL database in a local temporary socket directory"]
    async fn postgres_serializes_first_logins_without_overwriting_the_identity() {
        let url = std::env::var("OIDC_TEST_POSTGRES_URL").unwrap();
        let target = url::Url::parse(&url).unwrap();
        assert_eq!(target.host_str(), Some("localhost"));
        assert_eq!(target.username(), "oidc_test");
        assert_eq!(target.path(), "/oidc_provision_test");
        let socket = target
            .query_pairs()
            .find(|(key, _)| key == "host")
            .unwrap()
            .1;
        assert!(
            std::path::Path::new(socket.as_ref())
                .canonicalize()
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        let first = database(&url).await;
        let replica = connect(&url).await;
        let original = profile("postgres@example.com");
        let defaults = vec!["first".into(), "second".into()];
        let (a, b) = tokio::join!(
            provision_on(
                &first,
                "https://idp.example",
                "postgres",
                &original,
                &defaults
            ),
            provision_on(
                &replica,
                "https://idp.example",
                "postgres",
                &original,
                &defaults
            ),
        );
        let (id, _) = a.unwrap();
        assert_eq!(id, b.unwrap().0);
        assert_eq!(users::Entity::find().count(&first).await.unwrap(), 1);
        assert_eq!(Entity::find().count(&first).await.unwrap(), 1);
        assert_eq!(org_users::Entity::find().count(&first).await.unwrap(), 2);
        first.close().await.unwrap();
        replica.close().await.unwrap();
        let restarted = connect(&url).await;
        assert_eq!(
            get_on(&restarted, "https://idp.example", "postgres")
                .await
                .unwrap()
                .0,
            id
        );
        assert!(
            provision_on(
                &restarted,
                "https://other.example",
                "postgres",
                &original,
                &defaults
            )
            .await
            .is_err()
        );
        assert_eq!(Entity::find().count(&restarted).await.unwrap(), 1);
        restarted.close().await.unwrap();
    }
}
