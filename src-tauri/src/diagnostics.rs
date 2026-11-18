//! User-triggered support reports.
//!
//! The product log is intentionally local-only. This module turns the active
//! development diary into one reviewable Markdown attachment with a stable,
//! machine-readable manifest, UI/runtime context, and best-effort secret/path
//! redaction. It never copies conversation transcripts or project files.

use crate::debug_log;
use chrono::{Local, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

const REPORT_SCHEMA: u32 = 1;
const MAX_LOG_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECENT_ERROR_LINES: usize = 30;
const LOG_TAIL_CAPS: [usize; 4] = [4 * 1024 * 1024, 2 * 1024 * 1024, 1024 * 1024, 1024 * 1024];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReportRequest {
    pub schema_version: u32,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub ui_state: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSaveResult {
    pub path: String,
    pub bytes: u64,
    pub redactions: usize,
    pub log_files: usize,
}

#[derive(Debug, Clone)]
struct LogFile {
    name: String,
    size: u64,
    shown_bytes: usize,
    truncated: bool,
    content: String,
}

#[derive(Debug, Default)]
struct LogBundle {
    files: Vec<LogFile>,
    issues: Vec<String>,
}

#[derive(Debug)]
struct RuntimeEnvironment {
    utc: String,
    local: String,
    app_version: String,
    build_profile: &'static str,
    os: &'static str,
    arch: &'static str,
    family: &'static str,
    cpu_count: usize,
    pid: u32,
    exe_path: String,
    log_dir: String,
    inflight: String,
    acp: String,
    home: Option<PathBuf>,
}

#[derive(Debug)]
struct RenderedReport {
    text: String,
    redactions: usize,
    log_files: usize,
}

#[tauri::command]
pub fn save_diagnostic_report(
    request: DiagnosticReportRequest,
) -> Result<Option<DiagnosticSaveResult>, String> {
    let _trace = debug_log::CmdTrace::new("save_diagnostic_report");
    if request.schema_version != REPORT_SCHEMA {
        return Err(format!(
            "Unsupported diagnostic schema {} (expected {})",
            request.schema_version, REPORT_SCHEMA
        ));
    }
    let default_name = format!(
        "Marionette-diagnostics-{}.md",
        Local::now().format("%Y%m%d-%H%M%S")
    );
    let picked = rfd::FileDialog::new()
        .set_title(&crate::ts!("导出诊断报告", "Export diagnostic report"))
        .set_file_name(&default_name)
        .add_filter("Markdown", &["md"])
        .save_file();
    let Some(path) = picked else {
        return Ok(None);
    };

    let session_id = request
        .ui_state
        .pointer("/activeSession/id")
        .and_then(Value::as_str)
        .unwrap_or("");
    debug_log::append(
        "diagnostics",
        "info",
        session_id,
        "diagnostic report export requested",
        Some(&format!(
            "schema={} summaryChars={}",
            request.schema_version,
            request.summary.chars().count()
        )),
    );
    debug_log::flush_async(Duration::from_millis(750));

    let rendered = render_report(&request)?;
    fs::write(&path, rendered.text.as_bytes())
        .map_err(|error| format!("Write diagnostic report failed: {error}"))?;
    debug_log::append(
        "diagnostics",
        "info",
        session_id,
        "diagnostic report saved",
        Some(&format!(
            "bytes={} logs={} redactions={}",
            rendered.text.len(),
            rendered.log_files,
            rendered.redactions
        )),
    );
    Ok(Some(DiagnosticSaveResult {
        path: path.to_string_lossy().into_owned(),
        bytes: rendered.text.len() as u64,
        redactions: rendered.redactions,
        log_files: rendered.log_files,
    }))
}

fn render_report(request: &DiagnosticReportRequest) -> Result<RenderedReport, String> {
    render_report_with(request, capture_runtime(), collect_logs())
}

fn render_report_with(
    request: &DiagnosticReportRequest,
    runtime: RuntimeEnvironment,
    logs: LogBundle,
) -> Result<RenderedReport, String> {
    let mut redactions = 0usize;

    let (summary, count) = sanitize_text(request.summary.trim(), runtime.home.as_deref());
    redactions += count;
    let (ui_state, count) = sanitize_value(&request.ui_state, runtime.home.as_deref());
    redactions += count;
    let (exe_path, count) = sanitize_text(&runtime.exe_path, runtime.home.as_deref());
    redactions += count;
    let (log_dir, count) = sanitize_text(&runtime.log_dir, runtime.home.as_deref());
    redactions += count;
    let (inflight, count) = sanitize_text(&runtime.inflight, runtime.home.as_deref());
    redactions += count;
    let (acp, count) = sanitize_text(&runtime.acp, runtime.home.as_deref());
    redactions += count;

    let mut clean_files = Vec::with_capacity(logs.files.len());
    for file in &logs.files {
        let (content, count) = sanitize_text(&file.content, runtime.home.as_deref());
        redactions += count;
        clean_files.push((file, content));
    }
    let mut clean_issues = Vec::with_capacity(logs.issues.len());
    for issue in &logs.issues {
        let (clean, count) = sanitize_text(issue, runtime.home.as_deref());
        redactions += count;
        clean_issues.push(clean);
    }

    // Oldest file first gives one chronological stream across rotations.
    let mut chronological = String::new();
    for (_, content) in clean_files.iter().rev() {
        chronological.push_str(content);
        if !chronological.ends_with('\n') {
            chronological.push('\n');
        }
    }
    let warn_count = chronological
        .lines()
        .filter(|line| line.contains(" [warn] "))
        .count();
    let error_count = chronological
        .lines()
        .filter(|line| line.contains(" [error] "))
        .count();
    let recent_errors: Vec<&str> = chronological
        .lines()
        .rev()
        .filter(|line| line.contains(" [warn] ") || line.contains(" [error] "))
        .take(MAX_RECENT_ERROR_LINES)
        .collect();

    let manifest = json!({
        "schema": "marionette-diagnostic",
        "schemaVersion": REPORT_SCHEMA,
        "generatedAt": &runtime.utc,
        "generatedAtLocal": &runtime.local,
        "application": {
            "version": &runtime.app_version,
            "buildProfile": runtime.build_profile,
            "executable": &exe_path,
        },
        "system": {
            "os": runtime.os,
            "arch": runtime.arch,
            "family": runtime.family,
            "logicalCpuCount": runtime.cpu_count,
            "processId": runtime.pid,
        },
        "runtime": {
            "logDirectory": &log_dir,
            "inFlight": &inflight,
            "acp": &acp,
        },
        "uiState": ui_state,
        "logSummary": {
            "files": clean_files.iter().map(|(file, _)| json!({
                "name": &file.name,
                "bytes": file.size,
                "includedBytes": file.shown_bytes,
                "truncated": file.truncated,
            })).collect::<Vec<_>>(),
            "warnings": warn_count,
            "errors": error_count,
            "readIssues": &clean_issues,
        },
        "redactions": redactions,
        "included": {
            "userDescription": !summary.is_empty(),
            "uiSnapshot": true,
            "logs": !clean_files.is_empty(),
            "fullConversationTranscript": false,
            "projectFiles": false,
            "processEnvironment": false,
        },
    });
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("Serialize diagnostic manifest failed: {error}"))?;

    let mut report = String::new();
    report.push_str("<!-- marionette-diagnostic-report:v1 -->\n");
    report.push_str("# Marionette diagnostic report\n\n");
    report.push_str("> Generated locally by Marionette. Attach this file to the GitHub Issue.\n\n");
    report.push_str("## User description\n\n");
    if summary.is_empty() {
        report.push_str("_Not provided._\n\n");
    } else {
        report.push_str(&summary);
        report.push_str("\n\n");
    }
    report.push_str("## Runtime\n\n");
    report.push_str(&format!(
        "- App: {} ({})\n",
        runtime.app_version, runtime.build_profile
    ));
    report.push_str(&format!(
        "- Platform: {} / {} / {} / {} logical CPUs\n",
        runtime.os, runtime.arch, runtime.family, runtime.cpu_count
    ));
    report.push_str(&format!("- Executable: {exe_path}\n"));
    report.push_str(&format!("- Process ID: {}\n", runtime.pid));
    report.push_str(&format!("- Log directory: {log_dir}\n"));
    report.push_str(&format!("- ACP runtime: {acp}\n"));
    report.push_str(&format!("- In-flight commands: {inflight}\n\n"));

    report.push_str("## Recent warnings and errors\n\n");
    if recent_errors.is_empty() {
        report.push_str("_No warning or error lines were found._\n\n");
    } else {
        report.push_str("Newest first. Full retained logs follow below.\n\n");
        push_code_fence(&mut report, "text");
        for line in recent_errors {
            report.push_str(line);
            report.push('\n');
        }
        push_code_fence(&mut report, "");
        report.push('\n');
    }

    report.push_str("## Machine-readable manifest\n\n");
    push_code_fence(&mut report, "json");
    report.push_str(&manifest_json);
    report.push('\n');
    push_code_fence(&mut report, "");
    report.push('\n');

    report.push_str("## Logs\n\n");
    if !clean_issues.is_empty() {
        report.push_str("Log read issues:\n\n");
        for issue in &clean_issues {
            report.push_str(&format!("- {issue}\n"));
        }
        report.push('\n');
    }
    if clean_files.is_empty() {
        report.push_str("_No local log file was found._\n\n");
    } else {
        for (file, content) in &clean_files {
            report.push_str(&format!("### {}\n\n", file.name));
            report.push_str(&format!(
                "- Original size: {} bytes\n- Included: {} bytes{}\n\n",
                file.size,
                file.shown_bytes,
                if file.truncated { " (tail only)" } else { "" }
            ));
            report.push_str("~~~~text\n");
            report.push_str(content);
            if !content.ends_with('\n') {
                report.push('\n');
            }
            report.push_str("~~~~\n\n");
        }
    }

    report.push_str("## Privacy\n\n");
    report.push_str(
        "This report does not include full conversation transcripts, project files, the process environment block, or credential stores. Common API key/token/authorization shapes and the current user profile path are replaced before writing. Review the file before publishing it if the issue description itself contains sensitive data.\n",
    );

    Ok(RenderedReport {
        text: report,
        redactions,
        log_files: clean_files.len(),
    })
}

fn push_code_fence(report: &mut String, info: &str) {
    for _ in 0..3 {
        report.push('\u{60}');
    }
    report.push_str(info);
    report.push('\n');
}

fn capture_runtime() -> RuntimeEnvironment {
    let now = Utc::now();
    let log_paths = debug_log::log_file_paths().unwrap_or_default();
    let log_dir = log_paths
        .first()
        .and_then(|path| path.parent())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "%USERPROFILE%\\.marionette\\logs".to_string());
    RuntimeEnvironment {
        utc: now.to_rfc3339(),
        local: Local::now().to_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        os: env::consts::OS,
        arch: env::consts::ARCH,
        family: env::consts::FAMILY,
        cpu_count: std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(0),
        pid: std::process::id(),
        exe_path: env::current_exe()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|error| format!("<current_exe failed: {error}>")),
        log_dir,
        inflight: debug_log::inflight_summary(),
        acp: crate::acp::pipeline_summary(),
        home: user_home(),
    }
}

fn user_home() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
}

fn collect_logs() -> LogBundle {
    let mut bundle = LogBundle::default();
    let Ok(paths) = debug_log::log_file_paths() else {
        bundle
            .issues
            .push("Unable to resolve the local log directory.".to_string());
        return bundle;
    };
    let mut remaining = MAX_LOG_BYTES;
    for (index, path) in paths.into_iter().enumerate() {
        if remaining == 0 {
            break;
        }
        let cap = LOG_TAIL_CAPS
            .get(index)
            .copied()
            .unwrap_or(1024 * 1024)
            .min(remaining);
        match read_log_tail(&path, cap) {
            Ok(Some(file)) if file.size > 0 => {
                remaining = remaining.saturating_sub(file.shown_bytes);
                bundle.files.push(file);
            }
            Ok(_) => {}
            Err(error) => bundle.issues.push(error),
        }
    }
    bundle
}

fn read_log_tail(path: &Path, cap: usize) -> Result<Option<LogFile>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Could not read {}: {error}",
                path.to_string_lossy()
            ));
        }
    };
    let size = bytes.len() as u64;
    let truncated = bytes.len() > cap;
    let start = bytes.len().saturating_sub(cap);
    let slice = &bytes[start..];
    let mut content = String::from_utf8_lossy(slice).into_owned();
    if truncated {
        content.insert_str(
            0,
            &format!("[older {start} bytes omitted from this log generation]\n"),
        );
    }
    Ok(Some(LogFile {
        name: file_name(path),
        size,
        shown_bytes: slice.len(),
        truncated,
        content,
    }))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "dev.log".to_string())
}

fn sanitize_value(value: &Value, home: Option<&Path>) -> (Value, usize) {
    match value {
        Value::String(text) => {
            let (clean, count) = sanitize_text(text, home);
            (Value::String(clean), count)
        }
        Value::Array(items) => {
            let mut redactions = 0usize;
            let clean = items
                .iter()
                .map(|item| {
                    let (value, count) = sanitize_value(item, home);
                    redactions += count;
                    value
                })
                .collect();
            (Value::Array(clean), redactions)
        }
        Value::Object(object) => {
            let mut redactions = 0usize;
            let mut clean = Map::with_capacity(object.len());
            for (key, value) in object {
                if is_secret_key(key) && !value.is_null() {
                    clean.insert(key.clone(), Value::String("<redacted>".to_string()));
                    redactions += 1;
                    continue;
                }
                let (value, count) = sanitize_value(value, home);
                redactions += count;
                clean.insert(key.clone(), value);
            }
            (Value::Object(clean), redactions)
        }
        other => (other.clone(), 0),
    }
}

fn is_secret_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    matches!(
        normalized.as_str(),
        "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "authtoken"
            | "apikey"
            | "xapikey"
            | "password"
            | "secret"
            | "clientsecret"
            | "privatekey"
            | "authorization"
            | "proxyauthorization"
            | "cookie"
    )
}

fn sanitize_text(input: &str, home: Option<&Path>) -> (String, usize) {
    let mut text = input.to_string();
    let mut redactions = 0usize;

    if let Some(home) = home {
        let canonical = home.to_string_lossy();
        let variants = [
            canonical.clone().into_owned(),
            canonical.replace('/', "\\"),
            canonical.replace('\\', "/"),
            canonical.replace('\\', "\\\\"),
        ];
        let mut seen = HashSet::new();
        for variant in variants {
            if variant.len() < 4 || !seen.insert(variant.to_ascii_lowercase()) {
                continue;
            }
            let Ok(pattern) = Regex::new(&format!("(?i){}", regex::escape(&variant))) else {
                continue;
            };
            redactions += pattern.find_iter(&text).count();
            text = pattern.replace_all(&text, "%USERPROFILE%").into_owned();
        }
    }

    for pattern in redaction_patterns() {
        redactions += pattern.find_iter(&text).count();
        let replacement = if pattern.captures_len() > 1 {
            "$1<redacted>"
        } else {
            "<redacted>"
        };
        text = pattern.replace_all(&text, replacement).into_owned();
    }
    (text, redactions)
}

fn redaction_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            [
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
                r#"(?i)((?:proxy[-_])?authorization["']?\s*[:=]\s*)(?:bearer\s+)?[^"'\s,;}\]]+"#,
                r#"(?i)((?:set[-_])?cookie["']?\s*[:=]\s*)[^\r\n]+"#,
                r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]{8,}",
                r#"(?i)((?:(?:x[-_])?api[-_]?key|aws[-_]?secret[-_]?access[-_]?key|access[-_]?token|refresh[-_]?token|id[-_]?token|auth[-_]?token|private[-_]?key|client[-_]?secret|password|secret|token)["']?\s*[:=]\s*["']?)[^"'\s,;}\]]+"#,
                r"(?i)([?&](?:api[-_]?key|access[-_]?token|refresh[-_]?token|auth[-_]?token|token|key|secret|password|signature|sig)=)[^&#\s]+",
                r"(?i)[a-z][a-z0-9+.-]*://[^/\s:@]+:[^@\s/]+@",
                r"\bsk-[A-Za-z0-9_-]{12,}\b",
                r"\b(?:xox[baprs]-[A-Za-z0-9-]{10,})\b",
                r"\b[0-9]{6,12}:[A-Za-z0-9_-]{30,}\b",
                r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b",
                r"\bAIza[0-9A-Za-z_-]{20,}\b",
                r"\bAKIA[0-9A-Z]{16}\b",
                r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
            ]
            .into_iter()
            .map(|pattern| Regex::new(pattern).expect("diagnostic redaction regex must compile"))
            .collect()
        })
        .as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_runtime() -> RuntimeEnvironment {
        RuntimeEnvironment {
            utc: "2026-11-18T00:00:00Z".to_string(),
            local: "2026-11-18T09:00:00+09:00".to_string(),
            app_version: "9.9.9".to_string(),
            build_profile: "debug",
            os: "windows",
            arch: "x86_64",
            family: "windows",
            cpu_count: 8,
            pid: 42,
            exe_path: "C:\\Users\\alice\\Marionette.exe".to_string(),
            log_dir: "C:\\Users\\alice\\.marionette\\logs".to_string(),
            inflight: "<none>".to_string(),
            acp: "sessions=0".to_string(),
            home: Some(PathBuf::from("C:\\Users\\alice")),
        }
    }

    #[test]
    fn redacts_common_secret_shapes() {
        let input = concat!(
            "Authorization: Bearer abcdefghijklmnop\n",
            "{\"api_key\":\"sk-abcdefghijklmnop\"}\n",
            "https://example.test/path?access_token=qwerty123\n",
            "postgres://user:dummy-password@example.test/db\n",
            "Cookie: session=dummy-cookie-value\n",
            "123456789:abcdefghijklmnopqrstuvwxyz123456\n",
            "ghp_abcdefghijklmnopqrstuvwxyz1234\n",
            "jwt=eyJabcdefghijk.abcdefghijk.abcdefghijk\n",
        );
        let (clean, redactions) = sanitize_text(input, None);
        assert!(redactions >= 5, "unexpected redaction count: {redactions}");
        assert!(clean.contains("<redacted>"));
        assert!(!clean.contains("abcdefghijklmnop"));
        assert!(!clean.contains("qwerty123"));
        assert!(!clean.contains("dummy-password"));
        assert!(!clean.contains("dummy-cookie-value"));
        assert!(!clean.contains("abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!clean.contains("ghp_"));
        assert!(!clean.contains("eyJabcdefghijk"));
    }

    #[test]
    fn token_counts_are_not_mistaken_for_credentials() {
        let input = "usage input_tokens=123 output_tokens=456 cached_tokens=7";
        let (clean, redactions) = sanitize_text(input, None);
        assert_eq!(clean, input);
        assert_eq!(redactions, 0);
    }

    #[test]
    fn user_profile_paths_are_removed() {
        let home = PathBuf::from("C:\\Users\\alice");
        let (clean, redactions) = sanitize_text(
            "C:\\Users\\alice\\.marionette\\logs\\dev.log and C:/Users/alice/project",
            Some(&home),
        );
        assert_eq!(redactions, 2);
        assert!(!clean.to_ascii_lowercase().contains("users\\alice"));
        assert_eq!(clean.matches("%USERPROFILE%").count(), 2);
    }

    #[test]
    fn report_has_stable_manifest_and_privacy_boundary() {
        let request = DiagnosticReportRequest {
            schema_version: 1,
            summary: "api_key=dummyvalue123".to_string(),
            ui_state: json!({
                "activeSession": { "id": "session-1", "accessToken": "dummy-token" },
                "logicalCpuCount": 16,
            }),
        };
        let logs = LogBundle {
            files: vec![LogFile {
                name: "dev.log".to_string(),
                size: 120,
                shown_bytes: 120,
                truncated: false,
                content: "[1] [error] [acp] session=session-1 failed | Authorization: Bearer abcdefghijklmnop\n".to_string(),
            }],
            issues: vec![],
        };
        let rendered = render_report_with(&request, fake_runtime(), logs).expect("report");
        assert!(rendered
            .text
            .contains("<!-- marionette-diagnostic-report:v1 -->"));
        assert!(rendered.text.contains("## Machine-readable manifest"));
        assert!(rendered.text.contains("## Logs"));
        assert!(rendered.text.contains("\"errors\": 1"));
        assert!(rendered
            .text
            .contains("\"fullConversationTranscript\": false"));
        assert!(rendered.text.contains("<redacted>"));
        assert!(!rendered.text.contains("dummyvalue123"));
        assert!(!rendered.text.contains("abcdefghijklmnop"));
    }
}
