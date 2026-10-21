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

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{DefaultBodyLimit, RawQuery, Request},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use openobserve_api_common::auth::oidc::{self, Settings};
use serde::Deserialize;

static EXCHANGES: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);

#[derive(Deserialize)]
pub struct LoginInput {
    return_path: Option<String>,
}
#[derive(Deserialize)]
pub struct CallbackInput {
    state: String,
    code: String,
}

pub fn routes() -> Router {
    Router::new()
        .route("/auth/oidc/login", post(login))
        .route("/auth/oidc/callback", get(callback))
        .route("/auth/oidc/complete", post(complete))
        .layer(DefaultBodyLimit::max(16_384))
}

pub async fn bootstrap(headers: HeaderMap) -> Response {
    let enabled = config::get_config().oidc.enabled;
    if !enabled && !oidc::has_session_cookie(&headers) {
        return private(
            Json(serde_json::json!({"enabled": false, "authenticated": false})).into_response(),
        );
    }
    let Ok(settings) = Settings::load_for_recovery().await else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let origin = match settings.url(&settings.public_url) {
        Ok(url) => url.origin().ascii_serialization(),
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE),
    };
    if headers
        .get("x-o2-bootstrap")
        .and_then(|value| value.to_str().ok())
        != Some("1")
        || headers
            .get(header::ORIGIN)
            .is_some_and(|value| value.to_str().ok() != Some(origin.as_str()))
    {
        return failure(StatusCode::FORBIDDEN);
    }
    let browser = oidc::cookie(&headers, oidc::BROWSER_COOKIE).unwrap_or_else(oidc::random);
    let mut payload =
        serde_json::json!({"enabled": enabled, "csrf": browser, "authenticated": false});
    if oidc::has_session_cookie(&headers) {
        match oidc::session(&settings, &headers).await {
            Ok(session) => {
                payload["csrf"] = session.csrf.clone().into();
                match oidc::identity(&settings, &session).await {
                    Ok(identity) if enabled => {
                        payload["authenticated"] = true.into();
                        payload["user"] = serde_json::to_value(identity).unwrap_or_default();
                    }
                    _ => payload["session_invalid"] = true.into(),
                }
            }
            Err(_) => payload["session_invalid"] = true.into(),
        }
    }
    let mut response = private(Json(payload).into_response());
    set_cookie(
        &mut response,
        &settings,
        oidc::BROWSER_COOKIE,
        &browser,
        28800,
    );
    response
}

pub async fn login(headers: HeaderMap, Json(input): Json<LoginInput>) -> Response {
    let Ok(_permit) = EXCHANGES.try_acquire() else {
        return failure(StatusCode::TOO_MANY_REQUESTS);
    };
    let Ok(settings) = Settings::load().await else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(browser) = oidc::cookie(&headers, oidc::BROWSER_COOKIE) else {
        return failure(StatusCode::FORBIDDEN);
    };
    let proof = if oidc::has_session_cookie(&headers) {
        match oidc::session(&settings, &headers).await {
            Ok(session) => session.csrf,
            Err(_) => return failure(StatusCode::UNAUTHORIZED),
        }
    } else {
        browser.clone()
    };
    if oidc::csrf(&settings, &headers, &proof).is_err() {
        return failure(StatusCode::FORBIDDEN);
    }
    match oidc::begin(
        &settings,
        &browser,
        input.return_path.as_deref().unwrap_or(&settings.home()),
    )
    .await
    {
        Ok(authorize_url) => {
            private(Json(serde_json::json!({"authorize_url": authorize_url})).into_response())
        }
        Err(_) => failure(StatusCode::BAD_GATEWAY),
    }
}

pub async fn callback(headers: HeaderMap, RawQuery(query): RawQuery) -> Response {
    let Ok(settings) = Settings::load().await else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(browser) = oidc::cookie(&headers, oidc::BROWSER_COOKIE) else {
        return clean_redirect(&settings.login());
    };
    let Some(query) = query.filter(|q| q.len() <= 4096) else {
        return clean_redirect(&settings.login());
    };
    let pairs: Vec<_> = url::form_urlencoded::parse(query.as_bytes()).collect();
    if pairs.len() != 2
        || pairs.iter().filter(|(k, _)| k == "code").count() != 1
        || pairs.iter().filter(|(k, _)| k == "state").count() != 1
    {
        return clean_redirect(&settings.login());
    }
    let code = pairs
        .iter()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.as_ref())
        .unwrap_or("");
    let state = pairs
        .iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.as_ref())
        .unwrap_or("");
    let data = serde_json::json!({"code": code, "state": state});
    let encode = |value: serde_json::Value| value.to_string().replace('<', "\\u003c");
    let nonce = oidc::random();
    let html = format!(
        "<!doctype html><meta charset=utf-8><script nonce=\"{nonce}\">history.replaceState(null,'',{});fetch({},{{method:'POST',credentials:'same-origin',headers:{{'Content-Type':'application/json','x-o2-csrf':{}}},body:JSON.stringify({})}}).then(async r=>{{if(!r.ok)throw Error();location.replace((await r.json()).return_path)}}).catch(()=>location.replace({}));</script>",
        encode(settings.login().into()),
        encode(format!("{}/auth/oidc/complete", settings.url_path()).into()),
        encode(browser.into()),
        encode(data),
        encode(settings.login().into())
    );
    let mut response =
        private(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response());
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_str(&format!("default-src 'none'; script-src 'nonce-{nonce}'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'")).unwrap());
    response
}

pub async fn complete(headers: HeaderMap, Json(input): Json<CallbackInput>) -> Response {
    let Ok(_permit) = EXCHANGES.try_acquire() else {
        return failure(StatusCode::TOO_MANY_REQUESTS);
    };
    let Ok(settings) = Settings::load().await else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(browser) = oidc::cookie(&headers, oidc::BROWSER_COOKIE) else {
        return failure(StatusCode::FORBIDDEN);
    };
    if oidc::csrf(&settings, &headers, &browser).is_err() {
        return failure(StatusCode::FORBIDDEN);
    }
    match oidc::complete(&settings, &browser, &input.state, &input.code).await {
        Ok((session, return_path)) => {
            if oidc::revoke(&headers).await.is_err() {
                return failure(StatusCode::SERVICE_UNAVAILABLE);
            }
            let mut response =
                private(Json(serde_json::json!({"return_path": return_path})).into_response());
            set_cookie(
                &mut response,
                &settings,
                oidc::SESSION_COOKIE,
                &session,
                settings.absolute_seconds,
            );
            for name in ["auth_tokens", "auth_ext"] {
                set_cookie(&mut response, &settings, name, "", 0);
            }
            response
        }
        Err(_) => failure(StatusCode::UNAUTHORIZED),
    }
}

pub async fn logout(headers: HeaderMap) -> Response {
    let Ok(settings) = Settings::load_for_recovery().await else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let proof = match oidc::session(&settings, &headers).await {
        Ok(session) => session.csrf,
        Err(_) => match oidc::cookie(&headers, oidc::BROWSER_COOKIE) {
            Some(browser) => browser,
            None => return failure(StatusCode::FORBIDDEN),
        },
    };
    if oidc::csrf(&settings, &headers, &proof).is_err() {
        return failure(StatusCode::FORBIDDEN);
    }
    if oidc::revoke(&headers).await.is_err() {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    }
    let mut response = private(StatusCode::NO_CONTENT.into_response());
    for name in [
        oidc::SESSION_COOKIE,
        oidc::BROWSER_COOKIE,
        "auth_tokens",
        "auth_ext",
    ] {
        set_cookie(&mut response, &settings, name, "", 0);
    }
    response
}

pub async fn authenticate(mut request: Request, next: Next) -> Response {
    let Ok(settings) = Settings::load().await else {
        return failure(StatusCode::UNAUTHORIZED);
    };
    let identity = match oidc::authorize(
        &settings,
        request.headers(),
        request.method(),
        request.uri().path(),
    )
    .await
    {
        Ok(identity) => identity,
        Err(_) => return failure(StatusCode::UNAUTHORIZED),
    };
    let path = request.uri().path().trim_start_matches('/');
    let path = path.strip_prefix("api/").unwrap_or(path);
    let path = path.strip_prefix("v2/").unwrap_or(path);
    let org_list = path == "organizations" && request.method() == Method::GET;
    let Ok(email) = HeaderValue::from_str(&identity.email) else {
        return failure(StatusCode::UNAUTHORIZED);
    };
    request.headers_mut().insert("user_id", email);
    let response = next.run(request).await;
    if !org_list {
        return private(response);
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, 1_048_576).await else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Ok(mut payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(orgs) = payload
        .get_mut("data")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return failure(StatusCode::SERVICE_UNAVAILABLE);
    };
    orgs.retain(|org| {
        org.get("identifier")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| identity.organizations.iter().any(|allowed| allowed == id))
    });
    parts.headers.remove(header::CONTENT_LENGTH);
    private(Response::from_parts(parts, Body::from(payload.to_string())))
}

fn private(mut response: Response) -> Response {
    for (name, value) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::PRAGMA, "no-cache"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

fn failure(status: StatusCode) -> Response {
    private((status, "Authentication unavailable").into_response())
}
fn clean_redirect(path: &str) -> Response {
    private(axum::response::Redirect::to(path).into_response())
}
fn set_cookie(response: &mut Response, settings: &Settings, name: &str, value: &str, age: i64) {
    let secure = if settings.secure() { "; Secure" } else { "" };
    let value = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}{secure}");
    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}
