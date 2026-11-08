//! Spec section 11.4: navigation and new windows are blocked. The webview
//! may only show the app's own bundled frontend (or the dev server in debug
//! builds). Links in document content are never opened.

use tauri::Url;

/// `(scheme, host, port)` the bundled frontend is served from, per OS WebView.
const APP_ORIGINS: &[(&str, &str, Option<u16>)] = &[
    ("tauri", "localhost", None),
    ("http", "tauri.localhost", None),
    ("https", "tauri.localhost", None),
];

/// The Vite dev server, from `build.devUrl` in `tauri.conf.json`.
const DEV_ORIGIN: (&str, &str, Option<u16>) = ("http", "127.0.0.1", Some(1420));

pub fn is_allowed(url: &Url, allow_dev_server: bool) -> bool {
    // Compare parts rather than `Url::origin`, which is opaque for `tauri:`.
    let Some(host) = url.host_str() else {
        return false;
    };
    // `Url::port` is `None` for a scheme's default port, so app origins must use it.
    let matches = |(scheme, h, port): (&str, &str, Option<u16>)| {
        url.scheme() == scheme && host == h && url.port() == port
    };
    APP_ORIGINS.iter().any(|&o| matches(o)) || (allow_dev_server && matches(DEV_ORIGIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(s: &str, dev: bool) -> bool {
        is_allowed(&Url::parse(s).unwrap(), dev)
    }

    #[test]
    fn app_origins_are_allowed() {
        assert!(allowed("tauri://localhost/index.html", false));
        assert!(allowed("http://tauri.localhost/", false));
        assert!(allowed("https://tauri.localhost/assets/x.js", false));
    }

    #[test]
    fn dev_server_only_in_dev() {
        assert!(allowed("http://127.0.0.1:1420/", true));
        assert!(!allowed("http://127.0.0.1:1420/", false));
    }

    #[test]
    fn everything_else_is_blocked() {
        for url in [
            "https://example.com/",
            "http://localhost:1420/",
            "http://127.0.0.1:1421/",
            "http://tauri.localhost.evil.example/",
            "http://tauri.localhost:8080/",
            "tauri://localhost.evil.example/",
            "file:///C:/Windows/system.ini",
            "preview://localhost/abc/1.png",
            "javascript:alert(1)",
            "data:text/html,hi",
        ] {
            assert!(!allowed(url, true), "{url} should be blocked");
        }
    }
}
