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

use std::{collections::HashSet, time::Duration};

use axum::http::{HeaderMap, Method, header};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use infra::table::{entity::oidc_records::Model, oidc_records as store};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use rand::RngExt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use url::Url;

pub const SESSION_COOKIE: &str = "o2_oidc_session";
pub const BROWSER_COOKIE: &str = "o2_oidc_browser";
const MAX_DOCUMENT: usize = 65_536;

#[derive(Debug)]
pub struct OidcError;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub issuer: String,
    pub client_id: String,
    pub client_secret_file: String,
    pub public_url: String,
    #[serde(default)]
    pub allow_insecure_loopback: bool,
    #[serde(default = "default_idle")]
    pub idle_seconds: i64,
    #[serde(default = "default_absolute")]
    pub absolute_seconds: i64,
    pub bindings: Vec<Binding>,
}

impl Settings {
    pub async fn load() -> Result<Self, OidcError> {
        if !config::get_config().oidc.enabled {
            return Err(OidcError);
        }
        Self::load_for_recovery().await
    }

    pub async fn load_for_recovery() -> Result<Self, OidcError> {
        if cfg!(feature = "enterprise") {
            return Err(OidcError);
        }
        let cfg = config::get_config();
        let bytes = tokio::fs::read(&cfg.oidc.config_file)
            .await
            .map_err(|_| OidcError)?;
        if bytes.len() > MAX_DOCUMENT {
            return Err(OidcError);
        }
        let settings: Self = serde_json::from_slice(&bytes).map_err(|_| OidcError)?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<(), OidcError> {
        let issuer = self.url(&self.issuer)?;
        let public = self.url(&self.public_url)?;
        let prefix = config::get_config()
            .common
            .base_uri
            .trim_end_matches('/')
            .to_string();
        if self.issuer.ends_with('/')
            || self.public_url.ends_with('/')
            || public.path().trim_end_matches('/') != prefix
            || issuer.query().is_some()
            || public.query().is_some()
            || self.client_id.is_empty()
            || self.client_id.len() > 256
            || !(60..=3600).contains(&self.idle_seconds)
            || !(60..=28800).contains(&self.absolute_seconds)
            || self.idle_seconds > self.absolute_seconds
            || self.bindings.len() > 1000
        {
            return Err(OidcError);
        }
        let mut identities = HashSet::new();
        for binding in &self.bindings {
            if binding.issuer != self.issuer
                || binding.subject.is_empty()
                || binding.subject.len() > 255
                || binding.email.is_empty()
                || binding.user_id.is_empty()
                || !binding.allow_oss_admin
                || binding.organizations.is_empty()
                || !identities.insert((&binding.issuer, &binding.subject))
            {
                return Err(OidcError);
            }
        }
        Ok(())
    }

    pub fn url(&self, value: &str) -> Result<Url, OidcError> {
        let url = Url::parse(value).map_err(|_| OidcError)?;
        let local = self.allow_insecure_loopback
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
        if (url.scheme() != "https" && !local)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(OidcError);
        }
        Ok(url)
    }

    fn endpoint(&self, value: &str) -> Result<Url, OidcError> {
        let endpoint = self.url(value)?;
        let issuer = self.url(&self.issuer)?;
        let prefix = format!("{}/", issuer.path().trim_end_matches('/'));
        if endpoint.origin() != issuer.origin()
            || !endpoint.path().starts_with(&prefix)
            || endpoint.query().is_some()
        {
            return Err(OidcError);
        }
        Ok(endpoint)
    }

    pub fn callback(&self) -> String {
        format!("{}/auth/oidc/callback", self.public_url)
    }
    pub fn home(&self) -> String {
        format!("{}/web/", self.url_path())
    }
    pub fn login(&self) -> String {
        format!("{}/web/login", self.url_path())
    }
    pub fn url_path(&self) -> String {
        Url::parse(&self.public_url)
            .map(|u| u.path().trim_end_matches('/').to_string())
            .unwrap_or_default()
    }
    pub fn secure(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub issuer: String,
    pub subject: String,
    pub email: String,
    pub user_id: String,
    pub allow_oss_admin: bool,
    pub organizations: Vec<String>,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    userinfo_endpoint: String,
    code_challenge_methods_supported: Vec<String>,
    token_endpoint_auth_methods_supported: Vec<String>,
    id_token_signing_alg_values_supported: Vec<String>,
    response_modes_supported: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub struct Transaction {
    nonce: String,
    verifier: String,
    pub return_path: String,
    issuer: String,
    client_id: String,
    redirect_uri: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub client_id: String,
    pub issuer: String,
    pub subject: String,
    pub email: String,
    pub csrf: String,
}

#[derive(Serialize)]
pub struct Identity {
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub organizations: Vec<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    id_token: String,
    token_type: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: String,
    exp: i64,
    iat: i64,
    nonce: String,
}

#[derive(Deserialize)]
struct UserInfo {
    sub: String,
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
pub fn random() -> String {
    URL_SAFE_NO_PAD.encode(rand::rng().random::<[u8; 32]>())
}
pub fn digest(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found = None;
    for line in headers.get_all(header::COOKIE) {
        for item in line.to_str().ok()?.split(';') {
            if let Some((key, value)) = item.trim().split_once('=')
                && key == name
            {
                if found.is_some()
                    || value.len() != 43
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    return None;
                }
                found = Some(value.to_owned());
            }
        }
    }
    found
}

pub fn has_session_cookie(headers: &HeaderMap) -> bool {
    headers.get_all(header::COOKIE).iter().any(|h| {
        h.to_str().is_ok_and(|v| {
            v.split(';').any(|item| {
                item.trim()
                    .split_once('=')
                    .is_some_and(|(k, _)| k == SESSION_COOKIE)
            })
        })
    })
}

pub fn csrf(settings: &Settings, headers: &HeaderMap, proof: &str) -> Result<(), OidcError> {
    let origin = settings
        .url(&settings.public_url)?
        .origin()
        .ascii_serialization();
    if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) != Some(origin.as_str())
        || headers
            .get("x-o2-csrf")
            .and_then(|v| v.to_str().ok())
            .map(digest)
            != Some(digest(proof))
    {
        return Err(OidcError);
    }
    Ok(())
}

pub fn safe_return(settings: &Settings, value: &str) -> String {
    let home = settings.home();
    if value.starts_with(&home)
        && value.len() <= 2048
        && !value
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
        && !value.contains(['\\', '\r', '\n', '#', '?', '%'])
    {
        value.to_owned()
    } else {
        home
    }
}

pub async fn begin(
    settings: &Settings,
    browser: &str,
    return_path: &str,
) -> Result<String, OidcError> {
    let browser = digest(browser);
    if !store::capacity(&browser, now())
        .await
        .map_err(|_| OidcError)?
    {
        return Err(OidcError);
    }
    let discovery = discover(settings).await?;
    let state = random();
    let transaction = Transaction {
        nonce: random(),
        verifier: random(),
        return_path: safe_return(settings, return_path),
        issuer: settings.issuer.clone(),
        client_id: settings.client_id.clone(),
        redirect_uri: settings.callback(),
    };
    let mut authorize = settings.endpoint(&discovery.authorization_endpoint)?;
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", settings.client_id.as_str()),
        ("response_type", "code"),
        ("response_mode", "query"),
        ("redirect_uri", transaction.redirect_uri.as_str()),
        ("scope", "openid profile"),
        ("state", state.as_str()),
        ("nonce", transaction.nonce.as_str()),
        ("code_challenge", digest(&transaction.verifier).as_str()),
        ("code_challenge_method", "S256"),
    ]);
    save(&state, "transaction", &browser, &transaction, now() + 300).await?;
    Ok(authorize.to_string())
}

pub async fn complete(
    settings: &Settings,
    browser: &str,
    state: &str,
    code: &str,
) -> Result<(String, String), OidcError> {
    if state.len() != 43 || code.is_empty() || code.len() > 2048 {
        return Err(OidcError);
    }
    let handle = digest(state);
    let record = store::get(&handle, "transaction")
        .await
        .map_err(|_| OidcError)?
        .ok_or(OidcError)?;
    if !store::consume(&handle, &digest(browser), now())
        .await
        .map_err(|_| OidcError)?
    {
        return Err(OidcError);
    }
    let transaction: Transaction = serde_json::from_str(&record.payload).map_err(|_| OidcError)?;
    if transaction.issuer != settings.issuer
        || transaction.client_id != settings.client_id
        || transaction.redirect_uri != settings.callback()
    {
        return Err(OidcError);
    }
    let claims = exchange(settings, &transaction, code).await?;
    let binding = settings
        .bindings
        .iter()
        .find(|b| b.issuer == claims.iss && b.subject == claims.sub)
        .ok_or(OidcError)?;
    let session = Session {
        client_id: settings.client_id.clone(),
        issuer: claims.iss,
        subject: claims.sub,
        email: binding.email.clone(),
        csrf: random(),
    };
    identity(settings, &session).await?;
    let handle = random();
    save(
        &handle,
        "session",
        "",
        &session,
        now() + settings.absolute_seconds,
    )
    .await?;
    Ok((handle, transaction.return_path))
}

pub async fn session(settings: &Settings, headers: &HeaderMap) -> Result<Session, OidcError> {
    let handle = cookie(headers, SESSION_COOKIE).ok_or(OidcError)?;
    let hashed = digest(&handle);
    let record = store::get(&hashed, "session")
        .await
        .map_err(|_| OidcError)?
        .ok_or(OidcError)?;
    if !store::touch(&hashed, now(), settings.idle_seconds)
        .await
        .map_err(|_| OidcError)?
    {
        return Err(OidcError);
    }
    serde_json::from_str(&record.payload).map_err(|_| OidcError)
}

pub async fn identity(settings: &Settings, session: &Session) -> Result<Identity, OidcError> {
    let binding = binding(settings, session)?;
    let (id, user) = infra::table::users::get_oidc_identity(&binding.email)
        .await
        .map_err(|_| OidcError)?;
    if id != binding.user_id || !binding.allow_oss_admin || user.is_root {
        return Err(OidcError);
    }
    let memberships = infra::table::org_users::list_oidc_orgs_by_user(&binding.email)
        .await
        .map_err(|_| OidcError)?;
    let organizations = organization_intersection(
        binding,
        memberships.into_iter().map(|org| (org.org_id, org.role)),
    );
    if organizations.is_empty() {
        return Err(OidcError);
    }
    Ok(Identity {
        email: user.email,
        first_name: user.first_name,
        last_name: user.last_name,
        organizations,
    })
}

pub async fn authorize(
    settings: &Settings,
    headers: &HeaderMap,
    method: &Method,
    path: &str,
) -> Result<Identity, OidcError> {
    let session = session(settings, headers).await?;
    let identity = identity(settings, &session).await?;
    let path = path.trim_start_matches('/');
    let path = path.strip_prefix("api/").unwrap_or(path);
    let path = path.strip_prefix("v2/").unwrap_or(path);
    if common::meta::ingestion_routes::classify(method, path).is_some() {
        return Err(OidcError);
    }
    if headers
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
    {
        let origin = settings
            .url(&settings.public_url)?
            .origin()
            .ascii_serialization();
        if headers
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            != Some(origin.as_str())
        {
            return Err(OidcError);
        }
    }
    if !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        csrf(settings, headers, &session.csrf)?;
    }
    if path == "organizations" && method == Method::GET {
        return Ok(identity);
    }
    let segments: Vec<_> = path.split('/').collect();
    if segments
        .get(1)
        .is_some_and(|segment| *segment == "organizations")
    {
        return Err(OidcError);
    }
    let org = path.split('/').next().ok_or(OidcError)?;
    if !identity.organizations.iter().any(|allowed| allowed == org) {
        return Err(OidcError);
    }
    Ok(identity)
}

pub async fn revoke(headers: &HeaderMap) -> Result<(), OidcError> {
    if let Some(handle) = cookie(headers, SESSION_COOKIE) {
        store::revoke(&digest(&handle))
            .await
            .map_err(|_| OidcError)?;
    }
    Ok(())
}

fn binding<'a>(settings: &'a Settings, session: &Session) -> Result<&'a Binding, OidcError> {
    if session.client_id != settings.client_id {
        return Err(OidcError);
    }
    settings
        .bindings
        .iter()
        .find(|b| {
            b.issuer == session.issuer
                && b.subject == session.subject
                && b.email == session.email
                && b.allow_oss_admin
        })
        .ok_or(OidcError)
}

fn organization_intersection(
    binding: &Binding,
    memberships: impl Iterator<Item = (String, config::meta::user::UserRole)>,
) -> Vec<String> {
    memberships
        .filter(|(org, role)| {
            binding.organizations.contains(org) && *role == config::meta::user::UserRole::Admin
        })
        .map(|(org, _)| org)
        .collect()
}

fn default_idle() -> i64 {
    1800
}
fn default_absolute() -> i64 {
    28800
}

async fn save<T: Serialize>(
    handle: &str,
    kind: &str,
    browser: &str,
    payload: &T,
    expires_at: i64,
) -> Result<(), OidcError> {
    store::insert(Model {
        handle: digest(handle),
        kind: kind.to_owned(),
        browser: browser.to_owned(),
        payload: serde_json::to_string(payload).map_err(|_| OidcError)?,
        expires_at,
        touched_at: now(),
        consumed: false,
    })
    .await
    .map_err(|_| OidcError)
}

fn client() -> Result<reqwest::Client, OidcError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(3))
        .build()
        .map_err(|_| OidcError)
}

async fn document<T: DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<T, OidcError> {
    let mut response = request.send().await.map_err(|_| OidcError)?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > MAX_DOCUMENT as u64)
    {
        return Err(OidcError);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| OidcError)? {
        if bytes.len() + chunk.len() > MAX_DOCUMENT {
            return Err(OidcError);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| OidcError)
}

async fn discover(settings: &Settings) -> Result<Discovery, OidcError> {
    let discovery: Discovery = document(client()?.get(format!(
        "{}/.well-known/openid-configuration",
        settings.issuer
    )))
    .await?;
    if discovery.issuer != settings.issuer
        || !discovery
            .code_challenge_methods_supported
            .iter()
            .any(|v| v == "S256")
        || !discovery
            .token_endpoint_auth_methods_supported
            .iter()
            .any(|v| v == "client_secret_basic")
        || !discovery
            .id_token_signing_alg_values_supported
            .iter()
            .any(|v| v == "RS256")
        || !discovery
            .response_modes_supported
            .iter()
            .any(|v| v == "query")
    {
        return Err(OidcError);
    }
    for endpoint in [
        &discovery.authorization_endpoint,
        &discovery.token_endpoint,
        &discovery.jwks_uri,
        &discovery.userinfo_endpoint,
    ] {
        settings.endpoint(endpoint)?;
    }
    Ok(discovery)
}

async fn exchange(
    settings: &Settings,
    transaction: &Transaction,
    code: &str,
) -> Result<Claims, OidcError> {
    let discovery = discover(settings).await?;
    let secret = tokio::fs::read_to_string(&settings.client_secret_file)
        .await
        .map_err(|_| OidcError)?;
    if secret.is_empty() || secret.len() > 4096 {
        return Err(OidcError);
    }
    let encode =
        |value: &str| url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>();
    let tokens: TokenResponse = document(
        client()?
            .post(settings.endpoint(&discovery.token_endpoint)?)
            .basic_auth(encode(&settings.client_id), Some(encode(secret.trim_end())))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", transaction.redirect_uri.as_str()),
                ("code_verifier", transaction.verifier.as_str()),
            ]),
    )
    .await?;
    if !tokens.token_type.eq_ignore_ascii_case("Bearer")
        || tokens.access_token.len() > 8192
        || tokens.id_token.len() > 16384
    {
        return Err(OidcError);
    }
    let jwks: JwkSet = document(client()?.get(settings.endpoint(&discovery.jwks_uri)?)).await?;
    let claims = verify(settings, transaction, &tokens.id_token, &jwks)?;
    let userinfo: UserInfo = document(
        client()?
            .get(settings.endpoint(&discovery.userinfo_endpoint)?)
            .bearer_auth(&tokens.access_token),
    )
    .await?;
    if userinfo.sub != claims.sub {
        return Err(OidcError);
    }
    Ok(claims)
}

fn verify(
    settings: &Settings,
    transaction: &Transaction,
    token: &str,
    jwks: &JwkSet,
) -> Result<Claims, OidcError> {
    let header = decode_header(token).map_err(|_| OidcError)?;
    if header.alg != Algorithm::RS256 {
        return Err(OidcError);
    }
    let kid = header.kid.ok_or(OidcError)?;
    if kid.is_empty() || kid.len() > 256 {
        return Err(OidcError);
    }
    let mut keys = jwks
        .keys
        .iter()
        .filter(|jwk| jwk.common.key_id.as_deref() == Some(&kid));
    let key = keys.next().ok_or(OidcError)?;
    if key
        .common
        .key_algorithm
        .as_ref()
        .is_some_and(|alg| alg != &jsonwebtoken::jwk::KeyAlgorithm::RS256)
        || key
            .common
            .public_key_use
            .as_ref()
            .is_some_and(|usage| usage != &jsonwebtoken::jwk::PublicKeyUse::Signature)
        || key
            .common
            .key_operations
            .as_ref()
            .is_some_and(|operations| {
                !operations.contains(&jsonwebtoken::jwk::KeyOperations::Verify)
            })
    {
        return Err(OidcError);
    }
    if keys.next().is_some() {
        return Err(OidcError);
    }
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[&settings.issuer]);
    validation.set_audience(&[&settings.client_id]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat", "nonce"]);
    validation.leeway = 60;
    validation.validate_nbf = true;
    let claims = decode::<Claims>(
        token,
        &DecodingKey::from_jwk(key).map_err(|_| OidcError)?,
        &validation,
    )
    .map_err(|_| OidcError)?
    .claims;
    if claims.iss != settings.issuer
        || claims.aud != settings.client_id
        || claims.sub.is_empty()
        || claims.nonce != transaction.nonce
        || claims.iat > now() + 60
        || claims.iat < now() - 600
        || claims.exp <= claims.iat
        || claims.exp - claims.iat > 600
    {
        return Err(OidcError);
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{EncodingKey, Header, encode};

    use super::*;

    fn settings() -> Settings {
        Settings {
            issuer: "https://identity.example/oidc".into(),
            client_id: "client".into(),
            client_secret_file: "unused".into(),
            public_url: "https://observe.example".into(),
            allow_insecure_loopback: false,
            idle_seconds: 60,
            absolute_seconds: 300,
            bindings: vec![],
        }
    }

    fn transaction() -> Transaction {
        Transaction {
            nonce: "expected-nonce".into(),
            verifier: "unused".into(),
            return_path: "/web/".into(),
            issuer: settings().issuer,
            client_id: "client".into(),
            redirect_uri: "https://observe.example/auth/oidc/callback".into(),
        }
    }

    fn claims() -> serde_json::Value {
        serde_json::json!({"iss": "https://identity.example/oidc", "sub": "subject-1234", "aud": "client", "iat": now(), "exp": now()+300, "nonce": "expected-nonce"})
    }

    fn sign(claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(include_bytes!("fixtures/oidc-test-only.pem")).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn explicit_identity_and_live_membership_intersection_are_both_required() {
        use config::meta::user::UserRole;
        let mut settings = settings();
        settings.bindings.push(Binding {
            issuer: settings.issuer.clone(),
            subject: "subject-a".into(),
            email: "local@example.invalid".into(),
            user_id: "immutable-local-id".into(),
            allow_oss_admin: true,
            organizations: vec!["allowed".into(), "removed".into()],
        });
        let mut session = Session {
            client_id: "client".into(),
            issuer: settings.issuer.clone(),
            subject: "subject-a".into(),
            email: "local@example.invalid".into(),
            csrf: "proof".into(),
        };
        let policy = binding(&settings, &session).unwrap();
        assert_eq!(
            organization_intersection(
                policy,
                [
                    ("allowed".into(), UserRole::Admin),
                    ("extra".into(), UserRole::Admin)
                ]
                .into_iter()
            ),
            vec!["allowed"]
        );
        assert!(
            organization_intersection(policy, [("extra".into(), UserRole::Admin)].into_iter())
                .is_empty()
        );
        assert!(
            organization_intersection(policy, [("allowed".into(), UserRole::Root)].into_iter())
                .is_empty()
        );
        session.subject = "unmapped".into();
        assert!(binding(&settings, &session).is_err());
        session.subject = "subject-a".into();
        session.issuer = "https://other.example/oidc".into();
        assert!(binding(&settings, &session).is_err());
        session.issuer = settings.issuer.clone();
        session.client_id = "other-client".into();
        assert!(binding(&settings, &session).is_err());
        session.client_id = "client".into();
        settings.bindings[0].allow_oss_admin = false;
        assert!(binding(&settings, &session).is_err());
        settings.bindings.clear();
        assert!(binding(&settings, &session).is_err());
    }

    #[test]
    fn signed_identity_must_belong_to_this_transaction_and_client() {
        let jwks: JwkSet =
            serde_json::from_str(include_str!("fixtures/oidc-test-only.jwks.json")).unwrap();
        assert!(verify(&settings(), &transaction(), &sign(&claims()), &jwks).is_ok());
        for (key, value) in [
            ("iss", serde_json::json!("https://attacker.example/oidc")),
            ("aud", serde_json::json!("other")),
            ("aud", serde_json::json!(["client", "other"])),
            ("nonce", serde_json::json!("other")),
            ("sub", serde_json::json!("")),
            ("exp", serde_json::json!(now() - 120)),
            ("iat", serde_json::json!(now() + 120)),
        ] {
            let mut invalid = claims();
            invalid[key] = value;
            assert!(
                verify(&settings(), &transaction(), &sign(&invalid), &jwks).is_err(),
                "accepted {key}"
            );
        }
        let mut tampered = sign(&claims());
        tampered.push('x');
        assert!(verify(&settings(), &transaction(), &tampered, &jwks).is_err());
        let mut encryption_key = jwks.clone();
        encryption_key.keys[0].common.public_key_use =
            Some(jsonwebtoken::jwk::PublicKeyUse::Encryption);
        assert!(
            verify(
                &settings(),
                &transaction(),
                &sign(&claims()),
                &encryption_key
            )
            .is_err()
        );
        let empty = JwkSet { keys: vec![] };
        assert!(verify(&settings(), &transaction(), &sign(&claims()), &empty).is_err());
    }

    #[test]
    fn provider_documents_cannot_redirect_fetches_to_another_trust_domain() {
        let settings = settings();
        assert!(
            settings
                .endpoint("https://identity.example/oidc/jwks")
                .is_ok()
        );
        for value in [
            "https://attacker.example/oidc/jwks",
            "https://identity.example/other",
            "http://identity.example/oidc/jwks",
            "https://identity.example/oidc/jwks?url=evil",
            "https://user@identity.example/oidc/jwks",
            "https://identity.example/oidc/jwks#fragment",
        ] {
            assert!(settings.endpoint(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn csrf_needs_exact_origin_and_server_issued_proof() {
        let settings = settings();
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "https://observe.example".parse().unwrap());
        headers.insert("x-o2-csrf", "proof".parse().unwrap());
        assert!(csrf(&settings, &headers, "proof").is_ok());
        headers.insert(
            header::ORIGIN,
            "https://observe.example.evil".parse().unwrap(),
        );
        assert!(csrf(&settings, &headers, "proof").is_err());
        headers.insert(header::ORIGIN, "https://observe.example".parse().unwrap());
        headers.remove("x-o2-csrf");
        assert!(csrf(&settings, &headers, "proof").is_err());
    }

    #[test]
    fn ambiguous_cookies_and_external_return_paths_fail_closed() {
        let mut headers = HeaderMap::new();
        let value = "a".repeat(43);
        headers.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={value}; {SESSION_COOKIE}={value}")
                .parse()
                .unwrap(),
        );
        assert!(has_session_cookie(&headers));
        assert!(cookie(&headers, SESSION_COOKIE).is_none());
        for value in [
            "//evil.example/",
            "/web/../auth/login?next=evil",
            "/web/../../auth/login",
            "/web/%2f%2fevil",
            "/web/\\evil",
        ] {
            assert_eq!(safe_return(&settings(), value), "/web/");
        }
        assert_eq!(safe_return(&settings(), "/web/logs"), "/web/logs");
    }
}
