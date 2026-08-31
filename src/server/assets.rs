//! Serving the single-page app.
//!
//! In a release build the whole `web/dist` tree is compiled into the binary, so
//! `gribovik` is one file with no companion assets to lose. During frontend
//! work that is inconvenient — a rebuild of the SPA would mean a rebuild of the
//! crate — so `--assets <dir>` swaps the embedded tree for a directory on disk.
//!
//! Both modes end in the same place: a byte slice plus a guessed content type,
//! with anything unrecognized falling back to `index.html` so the SPA's own
//! router can decide what the path means.

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The frontend build, baked into the binary at compile time.
///
/// `build.rs` is what guarantees the directory exists; without it this derive
/// silently produces an empty asset set.
#[derive(RustEmbed)]
#[folder = "web/dist"]
struct Embedded;

/// The path served when a request matches no asset.
const INDEX: &str = "index.html";

/// Where the SPA's files come from.
#[derive(Debug, Clone)]
pub enum Assets {
    /// The `web/dist` tree embedded at build time.
    Embedded,
    /// A directory on disk, for iterating on the frontend without recompiling.
    Dir(PathBuf),
}

impl Assets {
    /// Choose the source: a directory when one was named on the command line,
    /// the embedded tree otherwise.
    pub fn new(dir: Option<PathBuf>) -> Self {
        match dir {
            Some(dir) => Assets::Dir(dir),
            None => Assets::Embedded,
        }
    }

    /// Read one asset by its request path, or `None` if there is no such file.
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Assets::Embedded => Embedded::get(path).map(|file| file.data.into_owned()),
            Assets::Dir(dir) => {
                let full = safe_join(dir, path)?;
                std::fs::read(full).ok()
            }
        }
    }

    /// Respond to a request for `path`.
    ///
    /// A miss is not a 404: the SPA owns client-side routes that have no file
    /// behind them, so anything unmatched gets `index.html` and is resolved in
    /// the browser. Only a build with no `index.html` at all can 404 here.
    pub fn respond(&self, path: &str) -> Response {
        let path = normalize(path);
        if let Some(bytes) = self.read(&path) {
            return file_response(&path, bytes);
        }
        match self.read(INDEX) {
            Some(bytes) => file_response(INDEX, bytes),
            None => (
                StatusCode::NOT_FOUND,
                "frontend assets are missing from this build",
            )
                .into_response(),
        }
    }
}

/// Turn a request path into an asset key: no leading slash, and `/` means the
/// index rather than the directory itself.
fn normalize(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        INDEX.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolve `path` inside `dir`, refusing anything that would climb out of it.
///
/// Only relative, downward components are allowed — a request for
/// `../../etc/passwd` has to fail rather than escape the asset directory.
fn safe_join(dir: &Path, path: &str) -> Option<PathBuf> {
    let mut full = dir.to_path_buf();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => full.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(full)
}

/// Wrap asset bytes in a response, guessing the content type from the name.
fn file_response(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    (
        [(header::CONTENT_TYPE, mime.as_ref().to_string())],
        Body::from(bytes),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use tempfile::TempDir;

    async fn body_string(response: Response) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn content_type(response: &Response) -> &str {
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
    }

    fn dir_assets() -> (TempDir, Assets) {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("index.html"), "<html>from disk</html>").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/app.js"), "console.log(1)").unwrap();
        let assets = Assets::new(Some(dir.path().to_path_buf()));
        (dir, assets)
    }

    #[test]
    fn new_without_a_directory_uses_the_embedded_tree() {
        assert!(matches!(Assets::new(None), Assets::Embedded));
    }

    #[test]
    fn normalize_maps_the_root_to_the_index() {
        assert_eq!(normalize("/"), INDEX);
        assert_eq!(normalize(""), INDEX);
        assert_eq!(normalize("/assets/app.js"), "assets/app.js");
    }

    #[test]
    fn safe_join_refuses_to_climb_out_of_the_directory() {
        let dir = Path::new("/srv/dist");
        assert_eq!(safe_join(dir, "../etc/passwd"), None);
        assert_eq!(safe_join(dir, "/etc/passwd"), None);
        assert_eq!(
            safe_join(dir, "./assets/app.js"),
            Some(PathBuf::from("/srv/dist/assets/app.js"))
        );
    }

    #[tokio::test]
    async fn the_embedded_index_is_served_at_the_root() {
        let response = Assets::Embedded.respond("/");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(content_type(&response), "text/html");
        assert!(body_string(response).await.contains("<div id=\"root\">"));
    }

    #[tokio::test]
    async fn an_unknown_embedded_path_falls_back_to_the_index() {
        let index = body_string(Assets::Embedded.respond("/")).await;
        let response = Assets::Embedded.respond("/some/spa/route");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_string(response).await, index);
    }

    #[tokio::test]
    async fn a_directory_overrides_the_embedded_tree() {
        let (_dir, assets) = dir_assets();

        let response = assets.respond("/");
        assert_eq!(content_type(&response), "text/html");
        assert_eq!(body_string(response).await, "<html>from disk</html>");
    }

    #[tokio::test]
    async fn directory_assets_get_a_guessed_content_type() {
        let (_dir, assets) = dir_assets();

        let response = assets.respond("/assets/app.js");
        assert_eq!(content_type(&response), "text/javascript");
        assert_eq!(body_string(response).await, "console.log(1)");
    }

    #[tokio::test]
    async fn a_traversing_path_falls_back_to_the_index_instead_of_escaping() {
        let (dir, assets) = dir_assets();
        std::fs::write(dir.path().parent().unwrap().join("secret.txt"), "nope").unwrap();

        let response = assets.respond("/../secret.txt");
        assert_eq!(body_string(response).await, "<html>from disk</html>");
    }

    #[tokio::test]
    async fn a_directory_without_an_index_is_a_404() {
        let dir = TempDir::new().unwrap();
        let assets = Assets::new(Some(dir.path().to_path_buf()));

        let response = assets.respond("/");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
