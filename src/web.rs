//! Serves the built frontend, which is embedded directly into the binary so the
//! panel ships as a single file with no assets to deploy alongside it.

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "web/dist"]
struct Assets;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if let Some(response) = serve(path) {
        return response;
    }

    // Unknown paths fall through to the SPA so client-side routing works,
    // but a missing API route should still look like a 404.
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }

    serve("index.html").unwrap_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "frontend is not built yet - run `npm install && npm run build` in web/",
        )
            .into_response()
    })
}

fn serve(path: &str) -> Option<Response> {
    let file = Assets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    // Hashed asset filenames can be cached hard; index.html must not be.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };

    Some(
        (
            [
                (header::CONTENT_TYPE, mime.as_ref()),
                (header::CACHE_CONTROL, cache),
            ],
            file.data,
        )
            .into_response(),
    )
}
