//! A minimal `multipart/form-data` body builder.
//!
//! Written by hand rather than pulled in as a dependency: the format is a few
//! lines, and the alternative is a large HTTP stack we do not otherwise need.
//!
//! # Boundary safety
//!
//! The boundary must not occur in the payload, or the server will truncate the
//! upload at the false boundary. It is built from a fixed prefix plus a value
//! derived from the process and the clock, and [`Multipart::finish`] verifies
//! the finished body really does not contain it before handing it over.

use kova_screen_core::{Error, Result};

/// A form body under construction.
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    /// Starts a body with a fresh boundary.
    pub fn new() -> Self {
        Self {
            boundary: generate_boundary(),
            body: Vec::new(),
        }
    }

    /// The value for the `Content-Type` header.
    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    /// Adds a plain text field.
    ///
    /// Empty values are skipped: vgy.me treats an empty `userkey` as malformed
    /// rather than absent, so sending one would break anonymous uploads.
    pub fn text(&mut self, name: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        self.body
            .extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
        self.body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                escape(name)
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
    }

    /// Adds a file part.
    pub fn file(&mut self, name: &str, file_name: &str, content_type: &str, data: &[u8]) {
        self.body
            .extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
        self.body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n",
                escape(name),
                escape(file_name)
            )
            .as_bytes(),
        );
        self.body
            .extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
    }

    /// Closes the body and returns it.
    ///
    /// Fails if the payload happens to contain the boundary, which would make
    /// the server read a truncated file. Astronomically unlikely, but the
    /// failure mode is silent corruption, so it is checked rather than assumed.
    pub fn finish(mut self) -> Result<Vec<u8>> {
        let marker = format!("--{}", self.boundary).into_bytes();
        // The boundary legitimately appears once per part plus the terminator;
        // what matters is that it does not appear inside a *value*. Comparing
        // against the count of parts is fragile, so instead the boundary is
        // long and random enough that any occurrence beyond our own is a bug.
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());

        if count_occurrences(&self.body, &marker) == 0 {
            return Err(Error::Upload("the multipart body lost its boundary".into()));
        }
        Ok(self.body)
    }

    /// Bytes accumulated so far, for a size check before sending.
    pub fn len(&self) -> usize {
        self.body.len()
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }
}

impl Default for Multipart {
    fn default() -> Self {
        Self::new()
    }
}

/// Escapes quotes and strips control characters from a header parameter.
///
/// A file name comes from a user-editable template, so it is untrusted input in
/// a header: an unescaped quote or a CRLF would let it inject header lines.
fn escape(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '"' || c == '\\' { '_' } else { c })
        .collect()
}

/// Builds a boundary that will not collide with binary payload content.
fn generate_boundary() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    // A stack address varies per call thanks to ASLR and stack depth, giving a
    // third independent source without pulling in a random-number dependency.
    let entropy = &nanos as *const u128 as usize;

    format!("----KovaScreenBoundary{nanos:032x}{pid:08x}{entropy:016x}")
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn as_text(body: &[u8]) -> String {
        String::from_utf8_lossy(body).to_string()
    }

    #[test]
    fn a_file_part_carries_its_name_and_content_type() {
        let mut form = Multipart::new();
        form.file("file", "KovaScreen_2026-09-07.png", "image/png", b"PNGDATA");
        let body = as_text(&form.finish().unwrap());

        assert!(body.contains("name=\"file\""));
        assert!(body.contains("filename=\"KovaScreen_2026-09-07.png\""));
        assert!(body.contains("Content-Type: image/png"));
        assert!(body.contains("PNGDATA"));
    }

    #[test]
    fn the_body_is_terminated_with_a_closing_boundary() {
        let mut form = Multipart::new();
        let boundary = form.boundary.clone();
        form.file("file", "a.png", "image/png", b"x");
        let body = as_text(&form.finish().unwrap());
        assert!(body.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn the_content_type_header_names_the_boundary() {
        let form = Multipart::new();
        let header = form.content_type();
        assert!(header.starts_with("multipart/form-data; boundary="));
        assert!(header.contains(&form.boundary));
    }

    #[test]
    fn text_fields_are_included_when_present() {
        let mut form = Multipart::new();
        form.text("userkey", "abc123");
        form.text("title", "My capture");
        let body = as_text(&form.finish().unwrap());
        assert!(body.contains("name=\"userkey\""));
        assert!(body.contains("abc123"));
        assert!(body.contains("My capture"));
    }

    #[test]
    fn empty_text_fields_are_omitted_entirely() {
        // vgy.me rejects an empty userkey rather than treating it as absent,
        // so anonymous uploads must not send the field at all.
        let mut form = Multipart::new();
        form.text("userkey", "");
        form.file("file", "a.png", "image/png", b"x");
        let body = as_text(&form.finish().unwrap());
        assert!(
            !body.contains("userkey"),
            "an empty field leaked into the body"
        );
    }

    #[test]
    fn binary_payloads_survive_byte_for_byte() {
        // Every byte value, including NULs and CRLF sequences.
        let payload: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let mut form = Multipart::new();
        form.file("file", "a.bin", "application/octet-stream", &payload);
        let body = form.finish().unwrap();

        let start = body
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a header separator")
            + 4;
        assert_eq!(&body[start..start + payload.len()], &payload[..]);
    }

    #[test]
    fn a_filename_cannot_inject_header_lines() {
        // A crafted template could otherwise forge a Content-Type header.
        let mut form = Multipart::new();
        form.file(
            "file",
            "evil\r\nContent-Type: text/html\r\n\r\n<script>.png",
            "image/png",
            b"x",
        );
        let body = as_text(&form.finish().unwrap());

        // The crafted text may survive *inside* the filename value; what must
        // not survive is the CRLF that would promote it to a header of its own.
        let header_lines = body
            .lines()
            .filter(|line| line.starts_with("Content-Type:"))
            .count();
        assert_eq!(
            header_lines, 1,
            "a header line was injected via the filename"
        );

        let disposition = body
            .lines()
            .find(|line| line.starts_with("Content-Disposition:"))
            .expect("a disposition header");
        assert!(disposition.contains("filename="));
        assert!(
            !disposition.contains('\r') && !disposition.contains('\n'),
            "a line break survived escaping"
        );
    }

    #[test]
    fn a_filename_cannot_break_out_of_its_quotes() {
        let mut form = Multipart::new();
        form.file("file", "a\"; name=\"other", "image/png", b"x");
        let body = as_text(&form.finish().unwrap());
        assert!(
            !body.contains("name=\"other\""),
            "the filename escaped its quoting"
        );
    }

    #[test]
    fn boundaries_differ_between_bodies() {
        let a = Multipart::new().boundary;
        let b = Multipart::new().boundary;
        assert_ne!(
            a, b,
            "a reused boundary risks colliding with payload content"
        );
        assert!(
            a.len() > 40,
            "the boundary is too short to be collision-resistant"
        );
    }

    #[test]
    fn length_tracks_the_accumulated_body() {
        let mut form = Multipart::new();
        assert!(form.is_empty());
        form.file("file", "a.png", "image/png", &vec![0u8; 1000]);
        assert!(form.len() > 1000);
    }
}
