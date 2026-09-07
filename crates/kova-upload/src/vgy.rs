//! The vgy.me upload provider.
//!
//! # Security posture
//!
//! - **HTTPS only.** The endpoint is a constant and [`VgyProvider::new`] refuses
//!   any base URL that is not `https://`, so a tampered config cannot downgrade
//!   an upload to plaintext.
//! - **Bounded everything.** Connect and response timeouts, a redirect limit,
//!   and a cap on how much of the response body is read. A hostile or broken
//!   server cannot hang the app or exhaust its memory.
//! - **The key is never logged.** `user_key` is moved into the request body and
//!   never appears in a tracing event or an error message.
//! - **The delete URL is treated as a secret.** Anyone holding it can remove the
//!   upload, so it is returned to the caller for local storage but never logged.

use std::time::Duration;

use kova_screen_core::{Error, Result};

use crate::multipart::Multipart;
use crate::provider::{MediaKind, UploadProvider, UploadRequest, UploadResult};

/// The documented upload endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://vgy.me/upload";

/// How long to wait for the connection and for the response.
///
/// Uploads are user-initiated and happen in the background, so a generous but
/// finite response timeout is right: long enough for a large GIF on a slow
/// link, short enough that a dead server does not leave a spinner forever.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

/// Redirects are allowed but bounded, so a redirect loop cannot spin.
const MAX_REDIRECTS: u32 = 3;

/// Most of a JSON response we will read.
///
/// The real response is a few hundred bytes. Anything beyond this is either a
/// broken server or an attempt to exhaust memory, and is refused.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

/// Uploads to vgy.me.
pub struct VgyProvider {
    endpoint: String,
    agent: ureq::Agent,
}

impl VgyProvider {
    /// Builds a provider pointing at the production endpoint.
    pub fn new() -> Result<Self> {
        Self::with_endpoint(DEFAULT_ENDPOINT)
    }

    /// Builds a provider pointing at `endpoint`.
    ///
    /// Only used by tests, which point it at a local server. Plaintext is
    /// allowed solely for `127.0.0.1`, so a misconfigured production endpoint
    /// still cannot send a capture over the network unencrypted.
    pub fn with_endpoint(endpoint: &str) -> Result<Self> {
        let is_https = endpoint.starts_with("https://");
        let is_loopback =
            endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:");

        if !is_https && !is_loopback {
            return Err(Error::Upload(
                "uploads must use https; refusing to send a capture over plaintext".into(),
            ));
        }

        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(RESPONSE_TIMEOUT))
            .max_redirects(MAX_REDIRECTS)
            .user_agent(concat!("KovaScreen/", env!("CARGO_PKG_VERSION")))
            .build();

        Ok(Self {
            endpoint: endpoint.to_string(),
            agent: config.into(),
        })
    }

    /// Content type for the file part.
    fn content_type_for(name: &str) -> &'static str {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".png") {
            "image/png"
        } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else if lower.ends_with(".webp") {
            "image/webp"
        } else if lower.ends_with(".gif") {
            "image/gif"
        } else if lower.ends_with(".mp4") {
            "video/mp4"
        } else {
            "application/octet-stream"
        }
    }
}

/// The JSON vgy.me returns.
///
/// Every field beyond `error` is optional in this struct even though the
/// documented response includes them: a provider that changes its shape, or an
/// error response that omits half of it, must produce a clear message rather
/// than a deserialisation panic.
#[derive(Debug, serde::Deserialize)]
struct VgyResponse {
    #[serde(default)]
    error: bool,
    #[serde(default)]
    filesize: Option<u64>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    ext: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    delete: Option<String>,
    /// Present on failures; shape varies, so it is kept as raw JSON.
    #[serde(default)]
    messages: Option<serde_json::Value>,
}

impl UploadProvider for VgyProvider {
    fn id(&self) -> &'static str {
        "vgy.me"
    }

    fn display_name(&self) -> &'static str {
        "vgy.me"
    }

    /// vgy.me is an image host. It accepts PNG, JPEG, WebP and GIF, but not
    /// video, so an MP4 upload is refused locally with an explanation instead of
    /// being sent and rejected.
    fn supports(&self, kind: MediaKind) -> bool {
        matches!(kind, MediaKind::Screenshot | MediaKind::Gif)
    }

    fn upload(&self, request: &UploadRequest) -> Result<UploadResult> {
        if !self.supports(request.kind) {
            return Err(Error::Upload(self.unsupported_message(request.kind)));
        }

        let size = request.validate()?;
        let file_name = request.file_name();

        // Read only after validation, so an oversized file is never loaded.
        let data = std::fs::read(&request.path).map_err(|source| Error::Storage {
            path: request.path.clone(),
            source,
        })?;

        let mut form = Multipart::new();
        form.file(
            "file",
            &file_name,
            Self::content_type_for(&file_name),
            &data,
        );
        if let Some(key) = request.user_key.as_deref() {
            form.text("userkey", key.trim());
        }

        let content_type = form.content_type();
        let body = form.finish()?;

        // Deliberately logs the size and never the key or the file contents.
        tracing::info!(bytes = size, "uploading to vgy.me");

        let response = self
            .agent
            .post(&self.endpoint)
            .header("Content-Type", &content_type)
            .send(&body[..]);

        let mut response = match response {
            Ok(response) => response,
            Err(err) => return Err(Error::Upload(describe_transport_error(&err))),
        };

        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|e| Error::Upload(format!("could not read the vgy.me response: {e}")))?;

        parse_response(status, &text)
    }
}

/// Turns a vgy.me response into an [`UploadResult`] or a readable error.
fn parse_response(status: u16, text: &str) -> Result<UploadResult> {
    // A non-JSON body (an HTML error page, a proxy notice) must not surface as
    // a parser error the user cannot act on.
    let parsed: VgyResponse = serde_json::from_str(text).map_err(|_| {
        if (200..300).contains(&status) {
            Error::Upload("vgy.me returned a response Kova Screen could not understand".into())
        } else {
            Error::Upload(format!("vgy.me rejected the upload (http {status})"))
        }
    })?;

    if parsed.error {
        let detail = parsed
            .messages
            .as_ref()
            .map(summarise_messages)
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "the server did not say why".to_string());
        return Err(Error::Upload(format!(
            "vgy.me rejected the upload: {detail}"
        )));
    }

    if !(200..300).contains(&status) {
        return Err(Error::Upload(format!("vgy.me returned http {status}")));
    }

    // A success with no usable link is a failure from the user point of view:
    // there would be nothing to put on the clipboard.
    let page_url = parsed.url.unwrap_or_default();
    let direct_url = parsed.image.unwrap_or_default();
    if page_url.is_empty() && direct_url.is_empty() {
        return Err(Error::Upload(
            "vgy.me reported success but returned no link".into(),
        ));
    }

    Ok(UploadResult {
        page_url,
        direct_url,
        delete_url: parsed.delete.filter(|d| !d.is_empty()),
        filename: match (parsed.filename, parsed.ext) {
            (Some(name), Some(ext)) if !ext.is_empty() => Some(format!("{name}.{ext}")),
            (Some(name), _) => Some(name),
            _ => None,
        },
        size: parsed.filesize,
    })
}

/// Flattens the varying `messages` shape into one line.
fn summarise_messages(value: &serde_json::Value) -> String {
    fn collect(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(items) => {
                for item in items {
                    collect(item, out);
                }
            }
            serde_json::Value::Object(map) => {
                for item in map.values() {
                    collect(item, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    collect(value, &mut out);
    // Bound the length: a hostile server should not be able to render a
    // multi-kilobyte toast.
    let joined = out.join("; ");
    if joined.chars().count() > 200 {
        joined.chars().take(200).collect::<String>() + "..."
    } else {
        joined
    }
}

/// Turns a transport failure into something a user can act on.
fn describe_transport_error(err: &ureq::Error) -> String {
    let text = err.to_string();
    let lower = text.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        "the upload timed out; the capture is still saved locally".to_string()
    } else if lower.contains("dns") || lower.contains("resolve") {
        "could not reach vgy.me; check your internet connection".to_string()
    } else if lower.contains("redirect") {
        "vgy.me redirected too many times".to_string()
    } else if lower.contains("tls") || lower.contains("certificate") {
        "the secure connection to vgy.me could not be established".to_string()
    } else {
        format!("could not reach vgy.me: {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A single-request local HTTP server, so the whole upload path -- multipart
    /// body, headers, status handling, JSON parsing -- is exercised for real
    /// without touching the network.
    struct TestServer {
        port: u16,
        handle: Option<std::thread::JoinHandle<Option<CapturedRequest>>>,
    }

    #[derive(Debug)]
    struct CapturedRequest {
        content_type: String,
        body: Vec<u8>,
    }

    impl TestServer {
        fn start(status: u16, response: String) -> Self {
            let server =
                Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("a local test server"));
            let port = server.server_addr().to_ip().unwrap().port();

            let handle = std::thread::spawn(move || {
                let mut request = server.recv().ok()?;
                let content_type = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Content-Type"))
                    .map(|h| h.value.as_str().to_string())
                    .unwrap_or_default();

                let mut body = Vec::new();
                std::io::copy(request.as_reader(), &mut body).ok()?;

                let response = tiny_http::Response::new(
                    tiny_http::StatusCode(status),
                    vec![
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap(),
                    ],
                    Cursor::new(response.into_bytes()),
                    None,
                    None,
                );
                request.respond(response).ok()?;

                Some(CapturedRequest { content_type, body })
            });

            Self {
                port,
                handle: Some(handle),
            }
        }

        fn endpoint(&self) -> String {
            format!("http://127.0.0.1:{}/upload", self.port)
        }

        fn captured(&mut self) -> Option<CapturedRequest> {
            self.handle.take()?.join().ok()?
        }
    }

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join("kova-vgy-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn request(path: PathBuf, user_key: Option<&str>) -> UploadRequest {
        UploadRequest {
            kind: MediaKind::from_path(&path).unwrap_or(MediaKind::Screenshot),
            path,
            user_key: user_key.map(str::to_string),
            max_bytes: 64 * 1024 * 1024,
        }
    }

    const SUCCESS_JSON: &str = r#"{
        "error": false,
        "filesize": 123456,
        "filename": "abc123",
        "ext": "png",
        "url": "https://vgy.me/u/abc123",
        "image": "https://i.vgy.me/abc123.png",
        "delete": "https://vgy.me/delete/abc123xyz"
    }"#;

    #[test]
    fn a_successful_upload_parses_every_documented_field() {
        let result = parse_response(200, SUCCESS_JSON).unwrap();
        assert_eq!(result.page_url, "https://vgy.me/u/abc123");
        assert_eq!(result.direct_url, "https://i.vgy.me/abc123.png");
        assert_eq!(
            result.delete_url.as_deref(),
            Some("https://vgy.me/delete/abc123xyz")
        );
        assert_eq!(result.filename.as_deref(), Some("abc123.png"));
        assert_eq!(result.size, Some(123_456));
    }

    #[test]
    fn an_end_to_end_upload_sends_a_well_formed_multipart_body() {
        let mut server = TestServer::start(200, SUCCESS_JSON.to_string());
        let provider = VgyProvider::with_endpoint(&server.endpoint()).unwrap();
        let path = temp_file("shot.png", b"\x89PNG\r\n\x1a\nFAKEPNGDATA");

        let result = provider
            .upload(&request(path, None))
            .expect("the upload succeeds");
        assert_eq!(result.direct_url, "https://i.vgy.me/abc123.png");

        let captured = server.captured().expect("the server saw a request");
        assert!(
            captured
                .content_type
                .starts_with("multipart/form-data; boundary="),
            "wrong content type: {}",
            captured.content_type
        );

        let body = String::from_utf8_lossy(&captured.body);
        assert!(body.contains("name=\"file\""), "no file part was sent");
        assert!(body.contains("filename=\"shot.png\""));
        assert!(body.contains("Content-Type: image/png"));
        assert!(
            body.contains("FAKEPNGDATA"),
            "the payload did not arrive intact"
        );
    }

    #[test]
    fn a_user_key_is_sent_when_configured() {
        let mut server = TestServer::start(200, SUCCESS_JSON.to_string());
        let provider = VgyProvider::with_endpoint(&server.endpoint()).unwrap();
        let path = temp_file("keyed.png", b"data");

        provider
            .upload(&request(path, Some("my-user-key")))
            .unwrap();

        let body = String::from_utf8_lossy(&server.captured().unwrap().body).to_string();
        assert!(body.contains("name=\"userkey\""));
        assert!(body.contains("my-user-key"));
    }

    #[test]
    fn no_userkey_field_is_sent_for_an_anonymous_upload() {
        let mut server = TestServer::start(200, SUCCESS_JSON.to_string());
        let provider = VgyProvider::with_endpoint(&server.endpoint()).unwrap();
        let path = temp_file("anon.png", b"data");

        provider.upload(&request(path, None)).unwrap();

        let body = String::from_utf8_lossy(&server.captured().unwrap().body).to_string();
        assert!(!body.contains("userkey"), "an empty userkey was sent");
    }

    #[test]
    fn an_error_response_reports_the_server_message() {
        let json = r#"{"error":true,"messages":{"file":["The file is too large."]}}"#;
        let err = parse_response(200, json).unwrap_err().to_string();
        assert!(
            err.contains("The file is too large."),
            "unhelpful message: {err}"
        );
    }

    #[test]
    fn an_error_response_without_messages_still_reads_sensibly() {
        let err = parse_response(200, r#"{"error":true}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not say why"), "unhelpful message: {err}");
    }

    #[test]
    fn an_html_error_page_does_not_surface_as_a_parser_error() {
        let err = parse_response(502, "<html><body>Bad Gateway</body></html>")
            .unwrap_err()
            .to_string();
        assert!(err.contains("502"), "the status code was lost: {err}");
        assert!(
            !err.contains("expected value"),
            "a serde message leaked: {err}"
        );
    }

    #[test]
    fn a_success_with_no_link_is_treated_as_a_failure() {
        // There would be nothing to put on the clipboard.
        let err = parse_response(200, r#"{"error":false}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("no link"), "unhelpful message: {err}");
    }

    #[test]
    fn a_partial_response_still_yields_what_it_can() {
        // Only the direct link: the result must still be usable.
        let json = r#"{"error":false,"image":"https://i.vgy.me/x.png"}"#;
        let result = parse_response(200, json).unwrap();
        assert_eq!(result.direct_url, "https://i.vgy.me/x.png");
        assert!(result.page_url.is_empty());
        assert_eq!(result.delete_url, None);
        assert_eq!(result.filename, None);
    }

    #[test]
    fn unexpected_field_types_do_not_crash_the_parser() {
        // A server change must produce an error, never a panic.
        for json in [
            r#"{"error":"yes"}"#,
            r#"{"error":false,"filesize":"big","image":"https://i.vgy.me/x.png"}"#,
            r#"[]"#,
            r#"null"#,
            r#"{"error":false,"image":12345}"#,
        ] {
            let _ = parse_response(200, json);
        }
    }

    #[test]
    fn an_oversized_error_message_is_truncated() {
        let long = "x".repeat(5000);
        let json = format!(r#"{{"error":true,"messages":["{long}"]}}"#);
        let err = parse_response(200, &json).unwrap_err().to_string();
        assert!(
            err.len() < 400,
            "a hostile server produced a {}-char message",
            err.len()
        );
    }

    #[test]
    fn plaintext_endpoints_are_refused_outside_loopback() {
        assert!(VgyProvider::with_endpoint("http://vgy.me/upload").is_err());
        assert!(VgyProvider::with_endpoint("ftp://vgy.me/upload").is_err());
        assert!(VgyProvider::with_endpoint("https://vgy.me/upload").is_ok());
        // Loopback stays available for the tests above.
        assert!(VgyProvider::with_endpoint("http://127.0.0.1:8080/upload").is_ok());
    }

    #[test]
    fn the_production_endpoint_is_https() {
        assert!(DEFAULT_ENDPOINT.starts_with("https://"));
        assert!(VgyProvider::new().is_ok());
    }

    #[test]
    fn video_uploads_are_refused_locally_with_an_explanation() {
        // vgy.me is an image host; sending an MP4 would waste the user upload.
        let provider = VgyProvider::new().unwrap();
        assert!(!provider.supports(MediaKind::Video));
        assert!(provider.supports(MediaKind::Screenshot));
        assert!(provider.supports(MediaKind::Gif));

        let path = temp_file("clip.mp4", b"fake mp4");
        let err = provider
            .upload(&request(path, None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("recording"), "unhelpful message: {err}");
        assert!(err.contains("vgy.me"));
    }

    #[test]
    fn an_oversized_file_is_refused_before_any_network_call() {
        let provider = VgyProvider::with_endpoint("http://127.0.0.1:1/upload").unwrap();
        let path = temp_file("huge.png", &vec![0u8; 5000]);
        let mut req = request(path, None);
        req.max_bytes = 1000;
        // Port 1 would refuse a connection; the error must be about the size.
        let err = provider.upload(&req).unwrap_err().to_string();
        assert!(
            err.contains("upload limit"),
            "the size check did not run first: {err}"
        );
    }

    #[test]
    fn content_types_match_the_extension() {
        assert_eq!(VgyProvider::content_type_for("a.png"), "image/png");
        assert_eq!(VgyProvider::content_type_for("a.JPG"), "image/jpeg");
        assert_eq!(VgyProvider::content_type_for("a.jpeg"), "image/jpeg");
        assert_eq!(VgyProvider::content_type_for("a.webp"), "image/webp");
        assert_eq!(VgyProvider::content_type_for("a.gif"), "image/gif");
        assert_eq!(VgyProvider::content_type_for("a.mp4"), "video/mp4");
        assert_eq!(
            VgyProvider::content_type_for("a.bin"),
            "application/octet-stream"
        );
    }

    #[test]
    fn an_unreachable_server_produces_a_readable_error() {
        // Port 1 is reserved and never listening.
        let provider = VgyProvider::with_endpoint("http://127.0.0.1:1/upload").unwrap();
        let path = temp_file("unreachable.png", b"data");
        let err = provider.upload(&request(path, None)).unwrap_err();
        assert!(matches!(err, Error::Upload(_)));
        let text = err.to_string();
        assert!(!text.is_empty());
    }

    #[test]
    fn a_server_error_status_is_reported_with_its_code() {
        let mut server = TestServer::start(500, "{}".to_string());
        let provider = VgyProvider::with_endpoint(&server.endpoint()).unwrap();
        let path = temp_file("fail.png", b"data");
        let err = provider
            .upload(&request(path, None))
            .unwrap_err()
            .to_string();
        assert!(err.contains("500"), "the status code was lost: {err}");
        let _ = server.captured();
    }
}
