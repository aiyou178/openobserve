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

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    Form, Json, Router,
    extract::State,
    http::{HeaderMap, header},
    routing::{get, post},
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use openobserve_api_common::auth::oidc::{self, Provisioning, Settings};
use serde_json::{Value, json};
use tokio::sync::Mutex;

#[derive(Clone)]
struct Provider {
    issuer: String,
    callback: String,
    login: Arc<Mutex<Value>>,
    email_scope: Arc<AtomicBool>,
}

async fn discovery(State(provider): State<Provider>) -> Json<Value> {
    let issuer = provider.issuer;
    let mut document = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "userinfo_endpoint": format!("{issuer}/userinfo"),
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "response_modes_supported": ["query"]
    });
    if provider.email_scope.load(Ordering::SeqCst) {
        document["scopes_supported"] = json!(["openid", "profile", "email"]);
    }
    Json(document)
}

async fn token(
    State(provider): State<Provider>,
    headers: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    let login = provider.login.lock().await;
    assert_eq!(
        headers[header::AUTHORIZATION],
        "Basic Y2xpZW50OnRlc3Qtc2VjcmV0"
    );
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["redirect_uri"], provider.callback);
    assert_eq!(oidc::digest(&form["code_verifier"]), login["challenge"]);
    assert_eq!(form["code"], "fixture-code");
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture".into());
    let claims = json!({
        "iss": provider.issuer, "aud": "client", "sub": login["subject"],
        "nonce": login["nonce"], "iat": oidc::now(), "exp": oidc::now() + 300
    });
    let token = encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(include_bytes!("../src/auth/fixtures/oidc-test-only.pem"))
            .unwrap(),
    )
    .unwrap();
    Json(json!({"access_token": "fixture-access-token", "token_type": "Bearer", "id_token": token}))
}

async fn userinfo(State(provider): State<Provider>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(
        headers[header::AUTHORIZATION],
        "Bearer fixture-access-token"
    );
    Json(provider.login.lock().await["userinfo"].clone())
}

async fn start_login(
    settings: &Settings,
    provider: &Provider,
    subject: &str,
    expected_scope: &str,
) -> (String, String) {
    let browser = oidc::random();
    let authorize = oidc::begin(settings, &browser, "/web/").await.unwrap();
    let url = url::Url::parse(&authorize).unwrap();
    let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["scope"], expected_scope);
    *provider.login.lock().await = json!({
        "subject": subject, "nonce": query["nonce"], "challenge": query["code_challenge"],
        "userinfo": {"sub": subject, "email": "first@example.com", "email_verified": true, "name": "First User"}
    });
    (browser, query["state"].clone())
}

#[test]
fn verified_callback_creates_a_usable_session_and_policy_changes_revoke_admission() {
    let directory = tempfile::tempdir().unwrap();
    let db_dir = directory.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    // Configure the isolated database before creating a runtime or reading global configuration.
    unsafe {
        std::env::set_var("ZO_DATA_DIR", directory.path());
        std::env::set_var("ZO_DATA_DB_DIR", &db_dir);
        std::env::set_var("ZO_META_STORE", "sqlite");
        std::env::set_var("ZO_LOCAL_MODE", "true");
        std::env::set_var("ZO_BASE_URI", "");
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let configured = std::path::Path::new(&config::get_config().common.data_db_dir)
                .canonicalize()
                .unwrap();
            assert_eq!(configured, db_dir.canonicalize().unwrap());
            infra::table::create_user_tables().await.unwrap();
            infra::table::organizations::add(
                "default",
                "OIDC test",
                config::meta::organization::OrganizationType::Default,
            )
            .await
            .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let issuer = format!("http://{}/oidc", listener.local_addr().unwrap());
            let secret = directory.path().join("client-secret");
            std::fs::write(&secret, "test-secret").unwrap();
            let mut settings = Settings {
                issuer: issuer.clone(),
                client_id: "client".into(),
                client_secret_file: secret.to_str().unwrap().into(),
                public_url: "http://127.0.0.1:5080".into(),
                allow_insecure_loopback: true,
                idle_seconds: 60,
                absolute_seconds: 300,
                bindings: vec![],
                auto_provision: Some(Provisioning {
                    default_organizations: vec!["default".into()],
                    allow_oss_admin: true,
                }),
            };
            settings.validate().unwrap();
            let provider = Provider {
                issuer,
                callback: settings.callback(),
                login: Arc::new(Mutex::new(Value::Null)),
                email_scope: Arc::new(AtomicBool::new(false)),
            };
            let router = Router::new()
                .route("/oidc/.well-known/openid-configuration", get(discovery))
                .route(
                    "/oidc/jwks",
                    get(|| async {
                        Json(
                            serde_json::from_str::<Value>(include_str!(
                                "../src/auth/fixtures/oidc-test-only.jwks.json"
                            ))
                            .unwrap(),
                        )
                    }),
                )
                .route("/oidc/token", post(token))
                .route("/oidc/userinfo", get(userinfo))
                .with_state(provider.clone());
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let (browser, state) = start_login(&settings, &provider, "first-subject", "openid profile").await;
            let (handle, returned) = oidc::complete(&settings, &browser, &state, "fixture-code")
                .await
                .unwrap();
            assert_eq!(returned, "/web/");
            let mut headers = HeaderMap::new();
            headers.insert(
                header::COOKIE,
                format!("{}={handle}", oidc::SESSION_COOKIE)
                    .parse()
                    .unwrap(),
            );
            let session = oidc::session(&settings, &headers).await.unwrap();
            assert!(session.user_id.is_some());
            let identity = oidc::identity(&settings, &session).await.unwrap();
            assert_eq!(identity.email, "first@example.com");
            assert_eq!(identity.organizations, vec!["default"]);
            assert!(
                db::user::get(Some("default"), &identity.email)
                    .await
                    .unwrap()
                    .is_some()
            );
            for credential in ["", "!", "wrong-password"] {
                let result = openobserve_api_common::auth::validator::validate_credentials(
                    &identity.email,
                    credential,
                    "default/_json",
                    &axum::http::Method::POST,
                    false,
                )
                .await
                .unwrap();
                assert!(!result.is_valid);
            }

            provider.email_scope.store(true, Ordering::SeqCst);
            let (browser, state) = start_login(&settings, &provider, "first-subject", "openid profile email").await;
            assert!(oidc::complete(&settings, &browser, &state, "fixture-code").await.is_ok());

            let provisioning = settings.auto_provision.take();
            assert!(oidc::identity(&settings, &session).await.is_err());
            let (browser, state) = start_login(&settings, &provider, "unapproved-subject", "openid profile").await;
            provider.login.lock().await["userinfo"]["email"] = json!("unapproved@example.com");
            assert!(
                oidc::complete(&settings, &browser, &state, "fixture-code")
                    .await
                    .is_err()
            );
            assert_eq!(infra::table::users::list(None).await.unwrap().len(), 1);
            settings.auto_provision = provisioning;
            let (browser, state) = start_login(&settings, &provider, "unverified-subject", "openid profile email").await;
            provider.login.lock().await["userinfo"] = json!({
                "sub": "different-subject", "email": "unverified@example.com", "email_verified": true
            });
            assert!(
                oidc::complete(&settings, &browser, &state, "fixture-code")
                    .await
                    .is_err()
            );
            assert!(
                infra::table::oidc_identities::get(&settings.issuer, "unverified-subject")
                    .await
                    .is_err()
            );
            assert_eq!(infra::table::users::list(None).await.unwrap().len(), 1);
            oidc::revoke(&headers).await.unwrap();
            assert!(oidc::session(&settings, &headers).await.is_err());
            server.abort();
        });
}
