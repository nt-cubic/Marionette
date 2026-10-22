//! Orb capture enrichment (docs/plan-orb-assistant.md §2 / 档③).
//!
//! Two small sinks for things dragged/pasted/typed into the orb:
//! - `save_blob`: base64 file blob → `~/.marionette/inbox/files/<ts>-<name>`.
//! - `fetch_url`: http(s) GET — image responses download into the same folder
//!   (content-type checked, size-capped), html/plain responses yield their
//!   `<title>` for the capture note. Network failures return Err so the
//!   frontend can degrade to recording the raw URL.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Cap for both blob saves and image downloads (same budget as the plan).
const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;
/// Page bodies are only scanned for `<title>`; 1 MB bounds memory and time.
const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// Connect+read budget for capture fetches (combined, best effort).
const FETCH_TIMEOUT_SECS: u64 = 15;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureFetchResult {
    /// "image" (downloaded to `path`) | "page" (title scanned or unknown type).
    pub kind: String,
    pub path: Option<String>,
    pub title: Option<String>,
}

/// The capture inbox root — also the cwd of the 收集箱 project.
pub fn inbox_dir() -> Result<PathBuf, String> {
    Ok(crate::app_paths::global_dir()?.join("inbox"))
}

fn inbox_files_dir() -> Result<PathBuf, String> {
    Ok(inbox_dir()?.join("files"))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Decode a base64 blob (a `data:…;base64,` prefix from the web layer is
/// accepted too — same convention as `save_pasted_image`) and store it under
/// `inbox/files/<millis>-<sanitized name>`. Returns the absolute path so the
/// capture prompt can attach it like any local file.
pub fn save_blob(data: &str, name: Option<&str>) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};

    let raw = data.trim();
    let b64 = raw
        .split_once("base64,")
        .map(|(_, rest)| rest.trim())
        .unwrap_or(raw);
    let bytes = B64
        .decode(b64)
        .map_err(|e| format!("invalid base64 blob: {e}"))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!(
            "file too large ({} MB > 20 MB)",
            bytes.len() / (1024 * 1024)
        ));
    }

    let file_name = format!("{}-{}", now_millis(), sanitize_file_name(name));
    let dir = inbox_files_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("create inbox/files failed: {e}"))?;
    let path = dir.join(&file_name);
    fs::write(&path, &bytes).map_err(|e| format!("write capture file failed: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// Keep `[A-Za-z0-9._-]` only, cap at 64 chars, never empty or dot-leading —
/// combined with the timestamp prefix every stored name is a plain ASCII leaf.
fn sanitize_file_name(name: Option<&str>) -> String {
    // ASCII-only by construction, so truncate can never split a codepoint
    // (cf. the PTY drain lesson in AGENTS.md).
    let mut out: String = name
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    out.truncate(64);
    if out.is_empty() || out.starts_with('.') {
        return "capture.bin".into();
    }
    out
}

/// Fetch a URL for capture enrichment. Mirrors the provider_usage probe
/// conventions: shared native-tls agent, explicit UA, one combined timeout.
pub fn fetch_url(url: &str) -> Result<CaptureFetchResult, String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(format!("only http(s) URLs can be fetched: {trimmed}"));
    }

    let resp = match crate::http_client::agent()?
        .get(trimmed)
        .set("User-Agent", "Marionette/0.1 (capture)")
        .set("Accept", "*/*")
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
        .call()
    {
        Ok(response) => response,
        // ureq returns non-2xx responses here; keep only the status.
        Err(ureq::Error::Status(status, _)) => {
            return Err(format!("HTTP {status} from {trimmed}"))
        }
        Err(error) => return Err(format!("HTTP request to {trimmed}: {error}")),
    };

    // Strip parameters: "image/png; charset=…" → "image/png".
    let content_type = resp.header("Content-Type").unwrap_or("");
    let mime = content_type
        .to_ascii_lowercase()
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    let image_ext = match mime.as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        "image/avif" => Some("avif"),
        _ => None,
    };

    if let Some(ext) = image_ext {
        let dir = inbox_files_dir()?;
        fs::create_dir_all(&dir).map_err(|e| format!("create inbox/files failed: {e}"))?;
        let path = dir.join(format!("{}.{ext}", now_millis()));
        if let Err(error) = stream_to_file_limited(resp.into_reader(), &path, MAX_FILE_BYTES) {
            // Never leave a truncated image behind for the agent to pick up.
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        return Ok(CaptureFetchResult {
            kind: "image".into(),
            path: Some(path.to_string_lossy().to_string()),
            title: None,
        });
    }

    if mime == "text/html" || mime == "text/plain" {
        let mut body = Vec::new();
        resp.into_reader()
            .take(MAX_TEXT_BYTES as u64)
            .read_to_end(&mut body)
            .map_err(|e| format!("read page body failed: {e}"))?;
        let title = extract_html_title(&String::from_utf8_lossy(&body));
        return Ok(CaptureFetchResult {
            kind: "page".into(),
            path: None,
            title,
        });
    }

    // Anything else (JSON feeds, unknown binaries): the URL itself is the
    // capture note — record it as a page without a title.
    Ok(CaptureFetchResult {
        kind: "page".into(),
        path: None,
        title: None,
    })
}

/// Stream a body to `path`, aborting (caller deletes the partial) once more
/// than `limit` bytes arrive. Returns the byte count written.
fn stream_to_file_limited(
    mut reader: impl Read,
    path: &std::path::Path,
    limit: usize,
) -> Result<usize, String> {
    let mut file = fs::File::create(path).map_err(|e| format!("create file failed: {e}"))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0usize;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download read failed: {e}"))?;
        if n == 0 {
            break;
        }
        total += n;
        if total > limit {
            return Err(format!(
                "image exceeds the {} MB cap",
                limit / (1024 * 1024)
            ));
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write download failed: {e}"))?;
    }
    file.flush().map_err(|e| format!("flush download failed: {e}"))?;
    Ok(total)
}

/// Case-insensitive manual scan for `<title>…</title>` — no HTML parser (size
/// budget). Whitespace collapses to single spaces; the 5 basic entities are
/// decoded. A missing/malformed tag yields None.
fn extract_html_title(html: &str) -> Option<String> {
    let bytes = html.as_bytes();

    // `<title` followed by '>', '/' or whitespace (skips `<titled>` etc.).
    let open = (0..bytes.len().saturating_sub(6)).find(|&i| {
        if !bytes[i..i + 6].eq_ignore_ascii_case(b"<title") {
            return false;
        }
        match bytes.get(i + 6) {
            Some(b'>') | Some(b'/') => true,
            Some(&c) => c.is_ascii_whitespace(),
            None => false,
        }
    })?;
    let tag_end = (open + 6..bytes.len()).find(|&i| bytes[i] == b'>')?;

    // A truncated read may have cut the closing tag — then take the rest.
    let close = (tag_end + 1..=bytes.len().saturating_sub(7))
        .find(|&i| bytes[i..i + 7].eq_ignore_ascii_case(b"</title"))
        .unwrap_or(bytes.len());
    let raw = html
        .get(tag_end + 1..close)
        .unwrap_or("")
        .trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if raw.is_empty() {
        return None;
    }
    // Titles are short; a runaway match must not paste megabytes into a prompt.
    let mut title = decode_basic_entities(&raw);
    if title.chars().count() > 300 {
        title = title.chars().take(300).collect();
    }
    Some(title)
}

/// The 5 basic entities only — anything else stays verbatim. `&amp;` is
/// decoded last so `&amp;lt;` yields `&lt;`, not `<`.
fn decode_basic_entities(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::{decode_basic_entities, extract_html_title, sanitize_file_name};

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name(Some("photo.png")), "photo.png");
        assert_eq!(sanitize_file_name(Some("a b/\\:photo (1).jpeg")), "abphoto1.jpeg");
        assert_eq!(sanitize_file_name(Some(".hidden")), "capture.bin");
        assert_eq!(sanitize_file_name(Some("...")), "capture.bin");
        assert_eq!(sanitize_file_name(Some("")), "capture.bin");
        assert_eq!(sanitize_file_name(None), "capture.bin");
        let long = "x".repeat(200);
        assert_eq!(sanitize_file_name(Some(&long)).len(), 64);
        // CJK names leave only a dot-leading ".png" → default, never odd leaf.
        assert_eq!(sanitize_file_name(Some("截图.png")), "capture.bin");
    }

    #[test]
    fn extracts_titles_case_insensitively() {
        assert_eq!(
            extract_html_title("<html><HEAD><TITLE>Hello &amp; World</TITLE></HEAD>"),
            Some("Hello & World".into())
        );
        assert_eq!(
            extract_html_title("<title lang=\"en\">\n  Spaced  out\n title </title>"),
            Some("Spaced out title".into())
        );
        // `<titled>` is not a title tag.
        assert_eq!(extract_html_title("<titled>x</titled>"), None);
        assert_eq!(extract_html_title("<title>"), None);
        assert_eq!(extract_html_title("no title at all"), None);
        // Entities; `&amp;lt;` must survive as literal `&lt;`.
        assert_eq!(
            extract_html_title("<title>a &lt;b&gt; c &quot;q&quot; d &#39;e&apos; &amp;lt;</title>"),
            Some("a <b> c \"q\" d 'e' &lt;".into())
        );
    }

    #[test]
    fn decodes_only_basic_entities() {
        assert_eq!(decode_basic_entities("a &amp; b"), "a & b");
        assert_eq!(decode_basic_entities("&amp;lt;"), "&lt;");
        assert_eq!(decode_basic_entities("&nbsp;"), "&nbsp;");
    }
}
