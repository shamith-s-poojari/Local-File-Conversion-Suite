//! `preview:` URI scheme (spec section 11.3).
//!
//! URLs look like `preview://localhost/<token>/<n>.png` (on Windows the
//! WebView spells this `http://preview.localhost/<token>/<n>.png`). The token
//! is random and maps to one job's preview directory; `<n>` is digits only,
//! so a URL can't express a path. Tokens die with the job via [`PreviewRegistry::revoke`].
//! Only files that really are PNGs are served.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::http::{header, Method, Request, Response, StatusCode};

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const TOKEN_HEX_LEN: usize = 32;
/// Rendered pages are well under this; anything larger isn't ours.
const MAX_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Default)]
pub struct PreviewRegistry {
    dirs: Mutex<HashMap<String, PathBuf>>,
}

impl PreviewRegistry {
    /// Register a job's preview directory and return its token.
    #[allow(dead_code)] // Used once the scheduler (T12) renders previews.
    pub fn register(&self, dir: PathBuf) -> String {
        let mut buf = [0u8; TOKEN_HEX_LEN / 2];
        getrandom::fill(&mut buf).expect("the OS random number generator is unavailable");
        let token: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        self.dirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(token.clone(), dir);
        token
    }

    /// Forget a token. Called when its job is discarded or fails.
    #[allow(dead_code)] // Used once the scheduler (T12) renders previews.
    pub fn revoke(&self, token: &str) {
        self.dirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(token);
    }

    fn resolve(&self, token: &str, page: u32) -> Option<PathBuf> {
        let dirs = self.dirs.lock().unwrap_or_else(|e| e.into_inner());
        dirs.get(token).map(|d| d.join(format!("{page}.png")))
    }
}

/// Parse `/<token>/<n>.png`. Anything else is rejected.
fn parse_path(path: &str) -> Option<(&str, u32)> {
    let rest = path.strip_prefix('/')?;
    let (token, file) = rest.split_once('/')?;
    let page = file.strip_suffix(".png")?;
    let token_ok = token.len() == TOKEN_HEX_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let page_ok = !page.is_empty() && page.len() <= 6 && page.bytes().all(|b| b.is_ascii_digit());
    if !(token_ok && page_ok) {
        return None;
    }
    Some((token, page.parse().ok()?))
}

fn read_png(path: &Path) -> Option<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_PREVIEW_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    bytes.starts_with(PNG_MAGIC).then_some(bytes)
}

fn empty(status: StatusCode) -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(status)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Borrowed(&[][..]))
        .expect("static response is valid")
}

pub fn handle(
    registry: &PreviewRegistry,
    request: &Request<Vec<u8>>,
) -> Response<Cow<'static, [u8]>> {
    if request.method() != Method::GET {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let Some((token, page)) = parse_path(request.uri().path()) else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Some(bytes) = registry.resolve(token, page).as_deref().and_then(read_png) else {
        return empty(StatusCode::NOT_FOUND);
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Cow::Owned(bytes))
        .expect("response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn get(uri: &str) -> Request<Vec<u8>> {
        Request::builder().uri(uri).body(Vec::new()).unwrap()
    }

    #[test]
    fn parses_only_token_and_page() {
        assert_eq!(parse_path(&format!("/{TOKEN}/1.png")), Some((TOKEN, 1)));
        assert_eq!(
            parse_path(&format!("/{TOKEN}/123456.png")),
            Some((TOKEN, 123_456))
        );
        for bad in [
            format!("/{TOKEN}/../1.png"),
            format!("/{TOKEN}/1.png/x"),
            format!("/{TOKEN}/1.jpg"),
            format!("/{TOKEN}/.png"),
            format!("/{TOKEN}/-1.png"),
            format!("/{TOKEN}/1234567.png"),
            format!("/{}/1.png", TOKEN.to_uppercase()),
            format!("/{}/1.png", &TOKEN[1..]),
            "/../../etc/passwd".into(),
            "/".into(),
            String::new(),
        ] {
            assert_eq!(parse_path(&bad), None, "{bad}");
        }
    }

    #[test]
    fn serves_registered_pngs_only() {
        let dir = std::env::temp_dir().join(format!("preview-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("1.png"), [PNG_MAGIC, b"rest"].concat()).unwrap();
        std::fs::write(dir.join("2.png"), b"<script>alert(1)</script>").unwrap();

        let reg = PreviewRegistry::default();
        let token = reg.register(dir.clone());

        let ok = handle(&reg, &get(&format!("preview://localhost/{token}/1.png")));
        assert_eq!(ok.status(), StatusCode::OK);
        assert_eq!(ok.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(ok.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");

        // Windows spelling of the same URL.
        let win = handle(
            &reg,
            &get(&format!("http://preview.localhost/{token}/1.png")),
        );
        assert_eq!(win.status(), StatusCode::OK);

        let not_png = handle(&reg, &get(&format!("preview://localhost/{token}/2.png")));
        assert_eq!(not_png.status(), StatusCode::NOT_FOUND);
        let missing = handle(&reg, &get(&format!("preview://localhost/{token}/3.png")));
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let unknown = handle(&reg, &get(&format!("preview://localhost/{TOKEN}/1.png")));
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

        let post = Request::builder()
            .method(Method::POST)
            .uri(format!("preview://localhost/{token}/1.png"))
            .body(Vec::new())
            .unwrap();
        assert_eq!(handle(&reg, &post).status(), StatusCode::METHOD_NOT_ALLOWED);

        reg.revoke(&token);
        let revoked = handle(&reg, &get(&format!("preview://localhost/{token}/1.png")));
        assert_eq!(revoked.status(), StatusCode::NOT_FOUND);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tokens_are_random_hex() {
        let reg = PreviewRegistry::default();
        let a = reg.register(PathBuf::from("a"));
        let b = reg.register(PathBuf::from("a"));
        assert_ne!(a, b);
        assert!(parse_path(&format!("/{a}/1.png")).is_some());
    }
}
