//! Local browser UI server (`theflash serve`).
//!
//! Serves the embedded HTML, CSS and compiled JS on `127.0.0.1:<port>` and
//! handles `POST /api/event` in-process through `damai::Client` and
//! `damai::Gate::for_user`, sharing the rate limit and cross-process
//! risk-control breaker.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use theflash::{damai, presentation};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const INDEX_HTML: &str = include_str!("../../frontend/public/index.html");
const STYLE_CSS: &str = include_str!("../../frontend/public/style.css");
const MAIN_TS: &str = include_str!("../../frontend/src/main.ts");
const APP_TS: &str = include_str!("../../frontend/src/app.ts");

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src data:; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 8192;

fn main_js() -> &'static str {
    static JS: OnceLock<String> = OnceLock::new();
    JS.get_or_init(|| strip_typescript(MAIN_TS))
}

fn app_js() -> &'static str {
    static JS: OnceLock<String> = OnceLock::new();
    JS.get_or_init(|| strip_typescript(APP_TS))
}

struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub async fn serve(port: u16, open: bool) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("没法在 127.0.0.1:{port} 启动本地界面服务：{e}"))?;
    let bound_port = listener
        .local_addr()
        .map_err(|e| format!("读不到本地界面服务端口：{e}"))?
        .port();
    let url = format!("http://127.0.0.1:{bound_port}");
    println!("theFlash 演出查询：{url}");
    println!("只查询，不登录，不下单。按 Ctrl+C 停止。");
    let _ = std::io::stdout().flush();

    if open {
        let _ = open_browser(&url);
    }

    let busy = Arc::new(AtomicBool::new(false));
    loop {
        tokio::select! {
            accept = listener.accept() => {
                if let Ok((stream, _)) = accept {
                    let busy = busy.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, bound_port, busy).await;
                    });
                }
            }
            _ = shutdown_signal() => {
                return Ok(());
            }
        }
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

fn open_browser(url: &str) -> Result<(), String> {
    let mut cmd = match std::env::var_os("THEFLASH_BROWSER") {
        Some(prog) if prog.is_empty() => return Ok(()),
        Some(prog) => Command::new(prog),
        None if cfg!(target_os = "macos") => Command::new("open"),
        None => Command::new("xdg-open"),
    };
    cmd.arg(url);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

async fn handle_connection(mut stream: TcpStream, port: u16, busy: Arc<AtomicBool>) {
    let mut buf = Vec::with_capacity(1024);
    let header_end = loop {
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() >= MAX_HEADER_BYTES {
            return;
        }
        let mut chunk = [0u8; 1024];
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let Some(request_line) = lines.next() else {
        return;
    };
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    let mut host: Option<String> = None;
    let mut origin: Option<String> = None;
    let mut sec_fetch_site: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut content_length: usize = 0;

    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        match name.as_str() {
            "host" => host = Some(value),
            "origin" => origin = Some(value),
            "sec-fetch-site" => sec_fetch_site = Some(value),
            "content-type" => content_type = Some(value),
            "content-length" => content_length = value.parse().unwrap_or(0),
            _ => {}
        }
    }

    let valid_host = match host.as_deref() {
        Some(h) => {
            h == format!("127.0.0.1:{port}")
                || h == format!("localhost:{port}")
                || h == "127.0.0.1"
                || h == "localhost"
        }
        None => false,
    };
    if !valid_host {
        write_json(
            &mut stream,
            403,
            &json!({
                "error": { "code": "forbidden", "message": "只允许通过本机地址访问。" }
            }),
        )
        .await;
        return;
    }

    if target == "/api/event" && method == "POST" {
        let host_val = host.as_deref().unwrap_or_default();
        let origin_ok = match origin.as_deref() {
            Some(o) => {
                o == format!("http://{host_val}")
                    || o == format!("http://127.0.0.1:{port}")
                    || o == format!("http://localhost:{port}")
            }
            None => false,
        };
        let fetch_site_ok = sec_fetch_site.as_deref() != Some("cross-site");
        let content_type_ok = content_type
            .as_deref()
            .and_then(|ct| ct.split(';').next())
            .is_some_and(|ct| ct.trim().eq_ignore_ascii_case("application/json"));
        if !origin_ok || !fetch_site_ok || !content_type_ok {
            write_json(
                &mut stream,
                403,
                &json!({
                    "error": { "code": "forbidden", "message": "请在本地查询页面中发起查询。" }
                }),
            )
            .await;
            return;
        }

        if busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            write_json(
                &mut stream,
                409,
                &json!({
                    "error": {
                        "code": "busy",
                        "message": "上一场还在查询中，请稍候（已有演出正在查询，请等查询结束后再试）。"
                    }
                }),
            )
            .await;
            return;
        }
        let _guard = BusyGuard(busy);

        let body_start = header_end + 4;
        let already = buf.len().saturating_sub(body_start);
        if content_length > MAX_BODY_BYTES {
            let mut remaining = content_length.saturating_sub(already).min(256 * 1024);
            let mut sink = [0u8; 2048];
            while remaining > 0 {
                let step = remaining.min(sink.len());
                match stream.read(&mut sink[..step]).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => remaining -= n,
                }
            }
            write_json(
                &mut stream,
                400,
                &json!({
                    "error": {
                        "code": "invalid-input",
                        "message": "查询内容过长，请只输入演出链接或 ID。"
                    }
                }),
            )
            .await;
            return;
        }
        let mut body = buf[body_start..].to_vec();
        while body.len() < content_length {
            let mut chunk = [0u8; 1024];
            let need = (content_length - body.len()).min(chunk.len());
            match stream.read(&mut chunk[..need]).await {
                Ok(0) | Err(_) => return,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
            }
        }

        let parsed: Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(_) => {
                write_json(
                    &mut stream,
                    400,
                    &json!({
                        "error": {
                            "code": "invalid-input",
                            "message": "无法读取查询内容，请输入演出链接或 ID。"
                        }
                    }),
                )
                .await;
                return;
            }
        };
        let Some(raw_input) = parsed
            .as_object()
            .and_then(|o| o.get("input"))
            .and_then(Value::as_str)
        else {
            write_json(
                &mut stream,
                400,
                &json!({
                    "error": {
                        "code": "invalid-input",
                        "message": "请输入大麦演出详情页链接或纯数字 ID。"
                    }
                }),
            )
            .await;
            return;
        };
        let trimmed = raw_input.trim();
        if trimmed.is_empty() || raw_input.encode_utf16().count() > 2048 {
            write_json(
                &mut stream,
                400,
                &json!({
                    "error": {
                        "code": "invalid-input",
                        "message": "请输入大麦演出详情页链接或纯数字 ID。"
                    }
                }),
            )
            .await;
            return;
        }

        let input = trimmed.to_string();
        let outcome = tokio::select! {
            biased;
            _ = wait_for_disconnect(&mut stream) => None,
            res = query_event(&input) => Some(res),
        };
        if let Some((status, payload)) = outcome {
            write_json(&mut stream, status, &payload).await;
        }
        return;
    }

    if method == "GET" {
        let asset = match target.as_str() {
            "/" => Some(("text/html; charset=utf-8", INDEX_HTML)),
            "/style.css" => Some(("text/css; charset=utf-8", STYLE_CSS)),
            "/main.js" => Some(("text/javascript; charset=utf-8", main_js())),
            "/app.js" => Some(("text/javascript; charset=utf-8", app_js())),
            _ => None,
        };
        if let Some((content_type, data)) = asset {
            write_response(&mut stream, 200, content_type, data.as_bytes()).await;
            return;
        }
    }

    write_json(
        &mut stream,
        404,
        &json!({
            "error": { "code": "not-found", "message": "页面不存在。" }
        }),
    )
    .await;
}

async fn wait_for_disconnect(stream: &mut TcpStream) {
    let mut buf = [0u8; 64];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => continue,
        }
    }
}

async fn query_event(input: &str) -> (u16, Value) {
    let item_id = match damai::parse_item_id(input) {
        Ok(id) => id,
        Err(e) => return error_response(&e),
    };
    let result = match super::client() {
        Ok(client) => client.event(item_id).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(ev) => (200, presentation::event(&ev)),
        Err(e) => error_response(&e),
    }
}

fn error_response(e: &damai::Error) -> (u16, Value) {
    let payload = presentation::error(e);
    let code = payload["error"]["code"].as_str().unwrap_or_default();
    (status_for(code), payload)
}

fn status_for(code: &str) -> u16 {
    match code {
        "invalid-input" => 400,
        "not-found" => 404,
        "risk-control" | "risk-stopped" => 403,
        "risk-state" | "invalid-config" | "setup" => 500,
        _ => 502,
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Error",
    }
}

async fn write_json(stream: &mut TcpStream, status: u16, value: &Value) {
    let body = value.to_string();
    write_response(
        stream,
        status,
        "application/json; charset=utf-8",
        body.as_bytes(),
    )
    .await;
}

async fn write_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status} {}\r\n\
         Content-Type: {content_type}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: {CSP}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        reason_phrase(status),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// Strips the TypeScript type syntax from `frontend/src/main.ts` and
/// `frontend/src/app.ts` so `theflash serve` embeds ready-to-run browser
/// JavaScript without needing a separate `frontend/dist` build directory at
/// `cargo build` time.
fn strip_typescript(ts: &str) -> String {
    let mut out = String::with_capacity(ts.len());
    for line in ts.lines() {
        if line.trim_start().starts_with("import type ") {
            continue;
        }
        let line = line.replace("<HTMLElement>(", "(");
        let line = strip_line_types(&line);
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn strip_line_types(line: &str) -> String {
    let trimmed = line.trim_start();
    if (trimmed.starts_with("export function ") || trimmed.starts_with("function "))
        && trimmed.ends_with('{')
    {
        return strip_signature_line(line, "{");
    }
    if trimmed.starts_with("const ") && trimmed.ends_with("=> {") {
        return strip_signature_line(line, "=> {");
    }
    if trimmed.starts_with("const ")
        && let Some((lhs, rhs)) = line.split_once(" = ")
        && !lhs.contains('(')
        && let Some((decl, _ty)) = lhs.split_once(':')
    {
        return format!("{decl} = {rhs}");
    }
    line.to_string()
}

fn strip_signature_line(line: &str, tail_marker: &str) -> String {
    let Some(open) = line.find('(') else {
        return line.to_string();
    };
    let Some(close) = line.rfind(')') else {
        return line.to_string();
    };
    let prefix = &line[..=open];
    let params = &line[open + 1..close];
    let cleaned_params = params
        .split(',')
        .map(|param| {
            let p = param.trim();
            if p.is_empty() {
                return String::new();
            }
            if let Some((before_eq, default_val)) = p.split_once(" = ") {
                let name = before_eq
                    .split_once(':')
                    .map_or(before_eq, |(n, _)| n)
                    .trim_end_matches('?');
                format!("{name} = {default_val}")
            } else if let Some((name, _ty)) = p.split_once(':') {
                name.trim_end_matches('?').to_string()
            } else {
                p.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{prefix}{cleaned_params}) {tail_marker}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stripped_browser_modules_contain_no_typescript_annotations() {
        let main = main_js();
        let app = app_js();
        assert!(main.contains("import { mount } from \"./app.js\";"));
        assert!(main.contains("document.querySelector(\"#app\")"));
        assert!(app.contains("export function mount(root, fetcher = fetch) {"));
        assert!(app.contains("export function renderEvent(root, event) {"));
        assert!(app.contains("const data = await response.json();"));
        assert!(app.contains("const el = (tag, className, text) => {"));
        assert!(app.contains("const badge = (status) => {"));
        for ts_only in [
            "import type",
            "querySelector<HTMLElement>",
            ": HTMLElement",
            ": EventView",
            ": Status",
            "text?: string",
            "): void",
        ] {
            assert!(!main.contains(ts_only), "main.js kept {ts_only}");
            assert!(!app.contains(ts_only), "app.js kept {ts_only}");
        }
    }
}
