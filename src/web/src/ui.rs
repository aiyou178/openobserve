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
    Router,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use rust_embed_for_web::EmbedableFile;

use crate::{SOURCE_ARCHIVE, WebAssets};

/// Serve static files from embedded web assets
pub async fn serve(Path(path): Path<String>) -> impl IntoResponse {
    let mut file_path = path.as_str();

    // For SPA routing, serve index.html for non-asset paths
    if !file_path.starts_with("src/")
        && !file_path.starts_with("assets/")
        && !file_path.starts_with("monacoeditorwork/")
        && !file_path.eq("favicon.ico")
    {
        file_path = "index.html";
    }

    if let Some((asset, data)) = SOURCE_ARCHIVE
        && asset == file_path
    {
        return asset_response(file_path, Body::from(data));
    }

    match WebAssets::get(file_path) {
        Some(content) => asset_response(file_path, Body::from(content.data().to_vec())),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not Found"))
            .unwrap(),
    }
}

pub fn ui_routes(base_href: &str) -> Router {
    Router::new()
        .route("/", get(|| async { serve(Path("".to_string())).await }))
        .route("/{*path}", get(serve))
        .layer(middleware::from_fn_with_state(
            base_href.to_string(),
            base_href_middleware,
        ))
}

fn asset_response(file_path: &str, body: Body) -> Response {
    let mime = mime_guess::from_path(file_path).first_or_octet_stream();
    // Content-addressed assets stay immutable while index.html must pick up new deployments.
    let cache_control =
        if file_path.starts_with("assets/") || file_path.starts_with("monacoeditorwork/") {
            "public, max-age=31536000, immutable"
        } else if file_path == "index.html" {
            "no-cache"
        } else {
            "public, max-age=3600"
        };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, cache_control)
        .header("X-Content-Type-Options", "nosniff")
        .body(body)
        .unwrap()
}

/// Middleware to rewrite base href in HTML responses
///
/// `base_href` is resolved once by the caller. Keeping it as middleware state is
/// what lets this crate stay free of a `config` dependency — see the module docs
/// on [`crate`].
async fn base_href_middleware(
    State(base_href): State<String>,
    request: axum::http::Request<Body>,
    next: Next,
) -> Response {
    let path = request
        .uri()
        .path()
        .strip_prefix("/")
        .unwrap_or("")
        .to_string();

    let response = next.run(request).await;

    // Only modify HTML responses for non-asset paths
    if path.starts_with("src/")
        || path.starts_with("assets/")
        || path.starts_with("monacoeditorwork/")
        || path.eq("favicon.ico")
    {
        return response;
    }

    // Check if this is an HTML response
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.contains("text/html") {
        return response;
    }

    // Extract and modify the body
    let (parts, body) = response.into_parts();
    match axum::body::to_bytes(body, usize::MAX).await {
        Ok(bytes) => {
            let body_str = String::from_utf8_lossy(&bytes);
            let modified_body = body_str.replace(
                r#"<base href="/" />"#,
                &format!(r#"<base href="{base_href}" />"#),
            );
            Response::from_parts(parts, Body::from(modified_body))
        }
        Err(_) => Response::from_parts(parts, Body::empty()),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn test_index_ok() {
        let app = ui_routes("/web/");
        let req = Request::builder()
            .uri("/index.html")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // In test environment, web assets might not be present
        // so we just check the route is reachable
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND);
    }

    /// The `<base href>` rewrite is the only reason `ui_routes` takes an
    /// argument, so pin it: whatever mount point the caller passes must come
    /// back out in the served HTML.
    ///
    /// Driven through a synthetic HTML response rather than the embedded
    /// `index.html`, so it still asserts something in builds where `web/dist`
    /// is absent and the embed is empty.
    #[tokio::test]
    async fn base_href_is_rewritten_to_the_mount_point() {
        for mount in ["/web/", "/abc/web/"] {
            let app = Router::new()
                .route(
                    "/index.html",
                    get(|| async {
                        (
                            [(header::CONTENT_TYPE, "text/html")],
                            r#"<html><head><base href="/" /></head></html>"#,
                        )
                    }),
                )
                .layer(middleware::from_fn_with_state(
                    mount.to_string(),
                    base_href_middleware,
                ));
            let req = Request::builder()
                .uri("/index.html")
                .body(Body::empty())
                .unwrap();

            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "mount={mount}");
            let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap();
            let html = String::from_utf8_lossy(&bytes);

            assert!(
                html.contains(&format!(r#"<base href="{mount}" />"#)),
                "mount={mount} produced {html}"
            );
        }
    }

    #[tokio::test]
    async fn test_index_not_ok() {
        let app = ui_routes("/web/");
        let req = Request::builder()
            .uri("/abc.html")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        // SPA routing should serve index.html for unknown paths
        // In test environment, this might return NOT_FOUND if assets aren't built
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::NOT_FOUND);
    }
}
