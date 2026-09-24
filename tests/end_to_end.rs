//! The whole plugin, through a mock Tauri app, against a real OTLP/HTTP
//! receiver on a loopback socket.
//!
//! Nothing here is faked below the exporter: the bytes asserted on are the
//! gzip'd protobuf the plugin actually posted. One test function on purpose,
//! because the plugin installs the process-global `tracing` subscriber and an
//! integration-test binary is one process.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use flate2::read::GzDecoder;
use opentelemetry_proto::tonic::{
    collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
    common::v1::{any_value::Value, AnyValue, KeyValue},
    logs::v1::LogRecord,
    trace::v1::Span,
};
use prost::Message as _;
use serde_json::json;
use tauri::{
    ipc::{CallbackFn, InvokeBody},
    test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY},
    webview::InvokeRequest,
    WebviewWindowBuilder,
};
use tauri_plugin_otel::{OtelExt as _, TraceParent};

/// One request the collector received: the path and the decoded body.
type Request = (String, Vec<u8>);

/// Every request the collector received.
#[derive(Default, Clone)]
struct Received(Arc<Mutex<Vec<Request>>>);

impl Received {
    fn spans(&self) -> Vec<(Vec<KeyValue>, Span)> {
        let mut out = Vec::new();
        for (path, body) in self.0.lock().unwrap().iter() {
            if path != "/v1/traces" {
                continue;
            }
            let request = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
            for resource in request.resource_spans {
                let attributes = resource.resource.unwrap_or_default().attributes;
                for scope in resource.scope_spans {
                    for span in scope.spans {
                        out.push((attributes.clone(), span));
                    }
                }
            }
        }
        out
    }

    fn logs(&self) -> Vec<LogRecord> {
        let mut out = Vec::new();
        for (path, body) in self.0.lock().unwrap().iter() {
            if path != "/v1/logs" {
                continue;
            }
            let request = ExportLogsServiceRequest::decode(body.as_slice()).unwrap();
            for resource in request.resource_logs {
                for scope in resource.scope_logs {
                    out.extend(scope.log_records);
                }
            }
        }
        out
    }
}

/// A minimal OTLP/HTTP receiver: HTTP/1.1, keep-alive, `Content-Length`
/// bodies, gzip or not, always `200`.
fn collector() -> (String, Received) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let received = Received::default();
    let sink = received.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let sink = sink.clone();
            std::thread::spawn(move || serve(stream, &sink));
        }
    });
    (endpoint, received)
}

fn serve(stream: TcpStream, sink: &Received) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let path = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        let (mut length, mut gzip) = (0, false);
        loop {
            let mut header = String::new();
            reader.read_line(&mut header).unwrap();
            let header = header.trim_end();
            if header.is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap();
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap(),
                "content-encoding" => gzip = value.trim() == "gzip",
                _ => {}
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        if gzip {
            let mut plain = Vec::new();
            GzDecoder::new(body.as_slice())
                .read_to_end(&mut plain)
                .unwrap();
            body = plain;
        }
        sink.0.lock().unwrap().push((path, body));
        writer
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/x-protobuf\r\ncontent-length: 0\r\n\r\n",
            )
            .unwrap();
    }
}

fn string(value: &Option<AnyValue>) -> Option<&str> {
    match value.as_ref()?.value.as_ref()? {
        Value::StringValue(text) => Some(text),
        _ => None,
    }
}

fn attribute<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|kv| kv.key == key)
        .and_then(|kv| string(&kv.value))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// An app command that continues the caller's trace.
#[tauri::command]
fn save_listing(parent: TraceParent) -> String {
    let _span = parent
        .parent_of(tracing::info_span!("save_listing"))
        .entered();
    tracing::info!(step = "validated", "withheld until something goes wrong");
    tracing::error!(listing = "l_42", "the push was refused");
    "saved".into()
}

const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const PAGE_SPAN: &str = "00f067aa0ba902b7";

/// The origin Tauri treats as the app's own, which differs by platform: wry
/// serves the app over `http://tauri.localhost` on Windows and Android and
/// over `tauri://localhost` elsewhere (`tauri` 2.11, `tauri_protocol_url`).
/// A request from anything else is remote, and the ACL refuses it.
fn local_origin() -> &'static str {
    if cfg!(any(windows, target_os = "android")) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
}

fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    body: serde_json::Value,
    traceparent: Option<&str>,
) -> Result<tauri::ipc::InvokeResponseBody, serde_json::Value> {
    let mut headers = tauri::http::HeaderMap::new();
    if let Some(value) = traceparent {
        headers.insert("traceparent", value.parse().unwrap());
    }
    get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: local_origin().parse().unwrap(),
            body: InvokeBody::Json(body),
            headers,
            invoke_key: INVOKE_KEY.into(),
        },
    )
}

/// A mock context whose ACL is resolved from this crate's real `permissions/`
/// and one capability granting `otel:default` to the `main` window — the same
/// resolution `tauri-build` performs for an app, so a permission missing from
/// `default.toml` fails this test instead of a user's first launch.
fn context() -> tauri::Context<tauri::test::MockRuntime> {
    use std::collections::BTreeMap;
    use tauri_utils::acl::{
        capability::Capability,
        manifest::{Manifest, PermissionFile},
        resolved::Resolved,
    };

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("permissions");
    let mut files = vec![root.join("default.toml")];
    for entry in std::fs::read_dir(root.join("autogenerated/commands")).unwrap() {
        files.push(entry.unwrap().path());
    }
    let permissions = files
        .iter()
        .map(|path| {
            toml::from_str::<PermissionFile>(&std::fs::read_to_string(path).unwrap()).unwrap()
        })
        .collect();
    let acl = BTreeMap::from([(
        tauri_plugin_otel::PLUGIN_NAME.to_owned(),
        Manifest::new(permissions, None),
    )]);
    let capability: Capability = serde_json::from_value(json!({
        "identifier": "main",
        "windows": ["main"],
        "permissions": ["otel:default"]
    }))
    .unwrap();
    let resolved = Resolved::resolve(
        &acl,
        BTreeMap::from([("main".to_owned(), capability)]),
        tauri_utils::platform::Target::current(),
    )
    .unwrap();
    let mut context = mock_context(noop_assets());
    *context.runtime_authority_mut() = tauri::ipc::RuntimeAuthority::new(acl, resolved);
    context
}

#[test]
fn rust_and_webview_telemetry_reach_the_collector_as_one_app() {
    let (endpoint, received) = collector();
    let app = mock_builder()
        .plugin(
            tauri_plugin_otel::Builder::new("vaam-vendor", "development")
                .endpoint(&endpoint)
                .build_id("0.1.0+7")
                .console(false)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![save_listing])
        .build(context())
        .unwrap();
    assert!(app.otel().is_exporting(), "{:?}", app.otel().status());
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    // 1. A command called with the page's span as its traceparent.
    let response = invoke(
        &webview,
        "save_listing",
        json!({}),
        Some(&format!("00-{TRACE}-{PAGE_SPAN}-01")),
    );
    assert!(response.is_ok(), "{response:?}");

    // 2. The page's own span, and a malformed one, through the plugin command.
    let accepted = invoke(
        &webview,
        "plugin:otel|export_spans",
        json!({"spans": [
            {"traceId": TRACE, "spanId": PAGE_SPAN, "name": "click save",
             "startTime": [1_758_700_000, 0], "endTime": [1_758_700_001, 0],
             "scope": {"name": "vendor-ui"}},
            {"traceId": "nope", "spanId": PAGE_SPAN, "name": "broken",
             "startTime": [1, 0], "endTime": [2, 0]}
        ]}),
        None,
    );
    assert_eq!(
        accepted.map(|body| body.deserialize::<usize>().unwrap()),
        Ok(1),
        "the plugin command was refused or miscounted"
    );

    // 3. The page's log records: one withheld, one exported.
    let logged = invoke(
        &webview,
        "plugin:otel|log",
        json!({"records": [
            {"level": "debug", "message": "page mounted"},
            {"level": "warn", "message": "offline, retrying",
             "traceId": TRACE, "spanId": PAGE_SPAN}
        ]}),
        None,
    );
    assert!(logged.is_ok(), "{logged:?}");

    app.otel().flush();

    // --- Spans ---------------------------------------------------------------
    let spans = received.spans();
    let command = spans
        .iter()
        .find(|(_, span)| span.name == "save_listing")
        .expect("the command's span was not exported");
    assert_eq!(
        hex(&command.1.trace_id),
        TRACE,
        "the trace did not continue"
    );
    assert_eq!(hex(&command.1.parent_span_id), PAGE_SPAN);
    let page = spans
        .iter()
        .find(|(_, span)| span.name == "click save")
        .expect("the page's span was not exported");
    assert_eq!(
        hex(&page.1.span_id),
        PAGE_SPAN,
        "the page's id was not kept"
    );
    assert!(!spans.iter().any(|(_, span)| span.name == "broken"));

    // One identity for both halves.
    for (resource, span) in [command, page] {
        assert_eq!(
            attribute(resource, "service.name"),
            Some("vaam-vendor"),
            "{}",
            span.name
        );
        assert_eq!(
            attribute(resource, "deployment.environment.name"),
            Some("development")
        );
        assert_eq!(attribute(resource, "app.build_id"), Some("0.1.0+7"));
        assert!(attribute(resource, "service.version").is_some());
    }

    // --- Logs ----------------------------------------------------------------
    let logs = received.logs();
    let bodies: Vec<&str> = logs.iter().filter_map(|log| string(&log.body)).collect();
    assert!(
        bodies.iter().any(|body| body.starts_with("vaam-vendor ")),
        "no startup record in {bodies:?}"
    );
    // The command's INFO was withheld and rode along with its ERROR.
    let fault = bodies
        .iter()
        .position(|body| *body == "the push was refused")
        .expect("the error was not exported");
    assert_eq!(bodies[fault - 1], "breadcrumbs");
    assert!(!bodies.contains(&"withheld until something goes wrong"));
    assert_eq!(
        hex(&logs[fault].trace_id),
        TRACE,
        "the error lost its trace"
    );
    // The page's DEBUG stayed home; its WARN left, carrying the page's span.
    assert!(!bodies.contains(&"page mounted"));
    let warn = logs
        .iter()
        .find(|log| string(&log.body) == Some("offline, retrying"))
        .expect("the page's warning was not exported");
    assert_eq!(hex(&warn.span_id), PAGE_SPAN);
    assert_eq!(warn.severity_text, "WARN");

    app.otel().shutdown();
}
