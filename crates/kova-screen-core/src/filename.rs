//! Filename templating and sanitisation.
//!
//! Every capture filename is derived from a user-editable template. Because the
//! template is user input that becomes a filesystem path, it is sanitised
//! aggressively: the output is always a single path component, never escapes
//! the capture directory, and never collides with an existing file.

use std::path::{Path, PathBuf};

use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;

/// The default template, matching the names in the product spec, e.g.
/// `KovaScreen_2026-09-07_09-24-52.png`.
pub const DEFAULT_TEMPLATE: &str = "KovaScreen_{date}_{time}";

/// Longest stem we will emit before appending a collision suffix.
///
/// Windows `MAX_PATH` is 260 for non-extended paths. Capping the stem at 120
/// leaves ample room for the directory, extension and `_NN` suffix.
const MAX_STEM_LEN: usize = 120;

const DATE_FMT: &[BorrowedFormatItem<'_>] = format_description!("[year]-[month]-[day]");
const TIME_FMT: &[BorrowedFormatItem<'_>] = format_description!("[hour]-[minute]-[second]");

/// Device names that Windows reserves at every directory level.
///
/// A file called `CON.png` cannot be created, so a template that renders to one
/// of these is rewritten rather than left to fail at save time.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Expands `{...}` tokens in `template` against `now`.
///
/// Unknown tokens are left verbatim (minus the braces, which get sanitised
/// away) so a typo produces an odd-looking name rather than a hard error.
///
/// Supported tokens: `{date}`, `{time}`, `{year}`, `{month}`, `{day}`,
/// `{hour}`, `{minute}`, `{second}`.
pub fn expand_template(template: &str, now: OffsetDateTime) -> String {
    let date = now.format(DATE_FMT).unwrap_or_else(|_| "0000-00-00".into());
    let time = now.format(TIME_FMT).unwrap_or_else(|_| "00-00-00".into());

    let expanded = template
        .replace("{date}", &date)
        .replace("{time}", &time)
        .replace("{year}", &format!("{:04}", now.year()))
        .replace("{month}", &format!("{:02}", now.month() as u8))
        .replace("{day}", &format!("{:02}", now.day()))
        .replace("{hour}", &format!("{:02}", now.hour()))
        .replace("{minute}", &format!("{:02}", now.minute()))
        .replace("{second}", &format!("{:02}", now.second()));

    sanitize_stem(&expanded)
}

/// Reduces arbitrary text to one safe Windows filename stem.
///
/// Strips path separators (so `..\\..\\evil` cannot traverse), control
/// characters, and characters Windows forbids; trims the trailing dots and
/// spaces that Windows silently drops; and rewrites reserved device names.
/// Always returns a non-empty string.
pub fn sanitize_stem(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        let safe = match ch {
            // Path separators and drive markers: the source of traversal bugs.
            '/' | '\\' | ':' => '_',
            // Characters Windows rejects outright.
            '<' | '>' | '"' | '|' | '?' | '*' => '_',
            // Template braces that survived expansion.
            '{' | '}' => '_',
            // Control characters, including newlines that could forge log lines.
            c if c.is_control() => '_',
            c => c,
        };
        out.push(safe);
    }

    // Windows strips trailing dots and spaces, which would desync the name we
    // think we wrote from the one on disk.
    let trimmed = out.trim_matches(|c: char| c == '.' || c == ' ' || c == '\u{a0}');
    let mut result = trimmed.to_string();

    if result.chars().count() > MAX_STEM_LEN {
        result = result.chars().take(MAX_STEM_LEN).collect();
        // Truncation can re-expose a trailing dot or space.
        result = result.trim_end_matches(['.', ' ']).to_string();
    }

    if result.is_empty() {
        return "KovaScreen".to_string();
    }

    // Reserved names are matched on the part before the first dot, case-insensitively.
    let head = result.split('.').next().unwrap_or(&result);
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(head)) {
        result.insert(0, '_');
    }

    result
}

/// Builds a collision-free path in `dir` for `stem`.`ext`.
///
/// If the plain name is taken, appends `_1`, `_2`, ... up to `limit` attempts.
/// Returns `None` if every candidate is taken, which the caller should treat as
/// a storage error rather than overwriting a file the user already has.
///
/// This is inherently a check-then-create race; the storage layer creates the
/// file with `create_new` so a lost race fails loudly instead of clobbering.
pub fn unique_path(dir: &Path, stem: &str, ext: &str, limit: u32) -> Option<PathBuf> {
    let stem = sanitize_stem(stem);
    let ext = sanitize_stem(ext);

    let candidate = dir.join(format!("{stem}.{ext}"));
    if !candidate.exists() {
        return Some(candidate);
    }
    for n in 1..=limit {
        let candidate = dir.join(format!("{stem}_{n}.{ext}"));
        if !candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const T: OffsetDateTime = datetime!(2026-09-07 09:24:52 UTC);

    #[test]
    fn default_template_matches_the_documented_name() {
        assert_eq!(
            expand_template(DEFAULT_TEMPLATE, T),
            "KovaScreen_2026-09-07_09-24-52"
        );
    }

    #[test]
    fn expands_individual_component_tokens() {
        assert_eq!(
            expand_template("{year}{month}{day}-{hour}{minute}{second}", T),
            "20260907-092452"
        );
    }

    #[test]
    fn unknown_tokens_degrade_to_a_readable_name() {
        // Braces are sanitised, so the user sees the typo rather than an error.
        assert_eq!(expand_template("shot_{nope}", T), "shot__nope_");
    }

    #[test]
    fn strips_path_separators_so_templates_cannot_traverse() {
        // Separators become underscores; the leading dots are then trimmed
        // because Windows would drop them anyway.
        assert_eq!(sanitize_stem("../../evil"), "_.._evil");
        assert_eq!(sanitize_stem(r"..\..\evil"), "_.._evil");
        assert_eq!(
            sanitize_stem("C:/Windows/System32/x"),
            "C__Windows_System32_x"
        );
    }

    #[test]
    fn template_with_traversal_never_yields_a_separator() {
        let out = expand_template("../../../{date}", T);
        assert!(!out.contains('/') && !out.contains('\\'));
    }

    #[test]
    fn strips_control_characters_including_newlines() {
        assert_eq!(sanitize_stem("a\nb\tc\0d"), "a_b_c_d");
    }

    #[test]
    fn trims_trailing_dots_and_spaces_windows_would_drop() {
        assert_eq!(sanitize_stem("shot.  "), "shot");
        assert_eq!(sanitize_stem("  shot..."), "shot");
    }

    #[test]
    fn rewrites_reserved_device_names() {
        assert_eq!(sanitize_stem("CON"), "_CON");
        assert_eq!(sanitize_stem("con"), "_con");
        assert_eq!(sanitize_stem("nul.png"), "_nul.png");
        // Names that merely start with a reserved word are fine.
        assert_eq!(sanitize_stem("CONTENT"), "CONTENT");
    }

    #[test]
    fn empty_or_fully_stripped_input_falls_back_to_a_usable_name() {
        assert_eq!(sanitize_stem(""), "KovaScreen");
        assert_eq!(sanitize_stem("..."), "KovaScreen");
        assert_eq!(sanitize_stem("   "), "KovaScreen");
    }

    #[test]
    fn caps_length_and_leaves_no_trailing_dot() {
        let long = "x".repeat(500);
        assert_eq!(sanitize_stem(&long).len(), MAX_STEM_LEN);
        let dotty = format!("{}{}", "y".repeat(MAX_STEM_LEN - 1), "..zz");
        assert!(!sanitize_stem(&dotty).ends_with('.'));
    }

    #[test]
    fn unique_path_returns_plain_name_when_free() {
        let dir = std::env::temp_dir().join("kova-uniq-free");
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("shot.png"));
        let p = unique_path(&dir, "shot", "png", 10).unwrap();
        assert_eq!(p.file_name().unwrap(), "shot.png");
    }

    #[test]
    fn unique_path_avoids_an_existing_file() {
        let dir = std::env::temp_dir().join("kova-uniq-collide");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dup.png"), b"x").unwrap();
        let p = unique_path(&dir, "dup", "png", 10).unwrap();
        assert_eq!(p.file_name().unwrap(), "dup_1.png");
        std::fs::write(dir.join("dup_1.png"), b"x").unwrap();
        let p = unique_path(&dir, "dup", "png", 10).unwrap();
        assert_eq!(p.file_name().unwrap(), "dup_2.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unique_path_gives_up_rather_than_overwriting() {
        let dir = std::env::temp_dir().join("kova-uniq-exhausted");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("full.png"), b"x").unwrap();
        std::fs::write(dir.join("full_1.png"), b"x").unwrap();
        assert!(unique_path(&dir, "full", "png", 1).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
