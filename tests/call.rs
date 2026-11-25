use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn temp(label: &str) -> (tempfile::TempDir, PathBuf) {
    let root_guard = tempfile::Builder::new()
        .prefix(&format!("twaco-call-{label}-"))
        .tempdir()
        .unwrap();
    let root = root_guard.path().to_path_buf();
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Test\"\n").unwrap();
    (root_guard, root)
}

fn profile(root: &Path, address: std::net::SocketAddr) {
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url = \"http://{address}/Thingworx/\"\nusername = \"user\"\npassword = \"pass\"\n"
        ),
    )
    .unwrap();
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(args)
        .current_dir(root)
        .env_remove("TWACO_URL")
        .env_remove("TWACO_USERNAME")
        .env_remove("TWACO_PASSWORD")
        .env_remove("TWX_URL")
        .env_remove("TWX_USERNAME")
        .env_remove("TWX_PASSWORD")
        .output()
        .unwrap()
}

fn receive(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut buffer).unwrap();
        request.extend_from_slice(&buffer[..read]);
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        assert!(read > 0);
    };
    let head = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map(|value| value.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    while request.len() < header_end + length {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0);
        request.extend_from_slice(&buffer[..read]);
    }
    request
}

fn answer(stream: &mut std::net::TcpStream, status: &str, content_type: &str, body: &str) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
}

#[test]
fn deploy_apply_refuses_excluded_referrers_before_contacting_the_server() {
    deploy_excluded_referrers(false, true);
}

#[test]
fn deploy_plan_notes_excluded_referrers_without_importing() {
    deploy_excluded_referrers(false, false);
}

#[test]
fn mcp_deploy_apply_refuses_excluded_referrers_before_contacting_the_server() {
    deploy_excluded_referrers(true, true);
}

#[test]
fn mcp_deploy_plan_notes_excluded_referrers_without_importing() {
    deploy_excluded_referrers(true, false);
}

fn deploy_excluded_referrers(mcp: bool, apply: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let (_dir, root) = temp("deploy-excluded-referrers");
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname='Acme.App'\n").unwrap();
    profile(&root, listener.local_addr().unwrap());
    for (collection, name, body) in [
        ("ThingShapes", "Acme.Shape", ""),
        (
            "Things",
            "Acme.Pump",
            "<ImplementedShapes><ImplementedShape name=\"Acme.Shape\"/></ImplementedShapes>",
        ),
    ] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        let tag = if collection == "Things" {
            "Thing"
        } else {
            "ThingShape"
        };
        std::fs::write(root.join(format!("{collection}/{name}.xml")), format!(
            "<Entities><{collection}><{tag} name=\"{name}\" projectName=\"Acme.App\">{body}</{tag}></{collection}></Entities>"
        )).unwrap();
    }
    let (stop, stopped) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while stopped.try_recv().is_err() && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let request = receive(&mut stream);
                    let line = String::from_utf8_lossy(&request)
                        .lines()
                        .next()
                        .unwrap()
                        .to_string();
                    if line.contains("/Projects/Acme.App") {
                        answer(
                            &mut stream,
                            "200 OK",
                            "text/xml",
                            r#"<Entities><Projects><Project name="Acme.App"/></Projects></Entities>"#,
                        );
                    } else {
                        answer(&mut stream, "404 Not Found", "text/plain", "absent");
                    }
                    requests.push(line);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        }
        requests
    });
    let (failed, text) = if mcp {
        let mut arguments =
            serde_json::json!({"exclude":["ThingShapes/Acme.Shape"], "skip_checks":true});
        if apply {
            arguments["dry_run"] = serde_json::json!(false);
        }
        let input = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"deploy", "arguments":arguments}}).to_string() + "\n";
        let mut output = Vec::new();
        twaco::mcp::serve_tools(
            &root,
            &twaco::mcp::Selection::ALL,
            input.as_bytes(),
            &mut output,
        )
        .unwrap();
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        (
            response["result"]["isError"].as_bool().unwrap_or(false),
            response.to_string(),
        )
    } else {
        let mut args = vec![
            "deploy",
            "--exclude",
            "ThingShapes/Acme.Shape",
            "--skip-checks",
        ];
        if apply {
            args.push("--apply");
        }
        let output = run(&root, &args);
        if apply {
            assert_eq!(output.status.code(), Some(2), "{output:?}");
        }
        (
            !output.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        )
    };
    stop.send(()).unwrap();
    let requests = server.join().unwrap();
    assert_eq!(failed, apply, "{text}; requests: {requests:?}");
    assert!(
        text.contains("excluded ThingShapes/Acme.Shape is still referenced by Things/Acme.Pump"),
        "{text}"
    );
    assert!(text.contains("--with-dependents"), "{text}");
    if apply {
        assert!(requests.is_empty(), "{requests:?}");
        assert!(text.contains("nothing was imported"), "{text}");
    } else {
        assert!(!requests.is_empty());
        assert!(
            requests.iter().all(|request| request.starts_with("GET ")),
            "{requests:?}"
        );
    }
}

#[test]
fn live_status_record_json_lists_pruned_entries_without_detail() {
    status_record_json(false);
}

#[test]
fn mcp_status_record_lists_pruned_entries_without_detail() {
    status_record_json(true);
}

fn status_record_json(mcp: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("status-record-json");
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname='Acme.App'\n").unwrap();
    profile(&root, listener.local_addr().unwrap());
    std::fs::create_dir(root.join("Things")).unwrap();
    let xml = r#"<Entities><Things><Thing name="Acme.Manager" projectName="Acme.App"/></Things></Entities>"#;
    std::fs::write(root.join("Things/Acme.Manager.xml"), xml).unwrap();
    std::fs::create_dir_all(root.join(".twaco/baselines")).unwrap();
    std::fs::write(
        root.join(".twaco/baselines/default.json"),
        r#"{"entities":{"Users":{"Acme.Gone":{"local":"v10:a","server":"v10:a"}}}}"#,
    )
    .unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..5 {
            let (mut stream, _) = listener.accept().unwrap();
            let request = receive(&mut stream);
            let request = String::from_utf8_lossy(&request);
            if request.contains("/Things/Acme.Manager") {
                answer(&mut stream, "200 OK", "text/xml", xml);
            } else if request.contains("/Users/Acme.Gone") {
                answer(&mut stream, "404 Not Found", "text/plain", "absent");
            } else {
                answer(&mut stream, "200 OK", "application/json", r#"{"rows":[]}"#);
            }
        }
    });
    let result: serde_json::Value = if mcp {
        let input = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"status", "arguments":{"all":true,"record":true}}}).to_string() + "\n";
        let mut output = Vec::new();
        twaco::mcp::serve_tools(
            &root,
            &twaco::mcp::Selection::ALL,
            input.as_bytes(),
            &mut output,
        )
        .unwrap();
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        response["result"]["structuredContent"].clone()
    } else {
        let output = run(&root, &["entity", "status", "--all", "--record", "--json"]);
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    };
    assert_eq!(result["recorded"], 1);
    assert_eq!(result["pruned"], 1);
    assert_eq!(
        result["pruned_entries"],
        serde_json::json!(["Users/Acme.Gone"])
    );
    assert_eq!(result["counts"]["in-sync"], 1);
    server.join().unwrap();
}

#[test]
fn status_record_prints_post_record_verdicts_and_returns_their_exit_code() {
    for matches in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let (_dir, root) = temp("status-record");
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname='Acme.App'\n").unwrap();
        profile(&root, listener.local_addr().unwrap());
        std::fs::create_dir(root.join("Things")).unwrap();
        let xml = "<Entities><Things><Thing name=\"Acme.Manager\" projectName=\"Acme.App\"><value>current</value></Thing></Things></Entities>";
        std::fs::write(root.join("Things/Acme.Manager.xml"), xml).unwrap();
        let baseline =
            r#"{"entities":{"Things":{"Acme.Manager":{"local":"v14:old","server":"v14:old"}}}}"#;
        std::fs::create_dir_all(root.join(".twaco/baselines")).unwrap();
        std::fs::write(root.join(".twaco/baselines/default.json"), baseline).unwrap();
        let response = if matches {
            xml.to_string()
        } else {
            xml.replace("current", "different")
        };
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                receive(&mut stream);
                answer(&mut stream, "200 OK", "text/xml", &response);
            }
        });
        let before = run(
            &root,
            &["entity", "status", "Things/Acme.Manager", "--detail"],
        );
        if matches {
            assert_eq!(before.status.code(), Some(0));
            assert!(
                String::from_utf8_lossy(&before.stdout).contains("Things/Acme.Manager  converged")
            );
        } else {
            assert_eq!(before.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&before.stdout)
                .contains("Things/Acme.Manager  both-changed"));
        }
        let after = run(
            &root,
            &["entity", "status", "Things/Manager", "--record", "--detail"],
        );
        let text = String::from_utf8_lossy(&after.stdout);
        if matches {
            assert_eq!(after.status.code(), Some(0), "{text}");
            assert!(text.contains("baseline recorded for 1 entity"), "{text}");
            assert!(text.contains("Things/Acme.Manager  in-sync"), "{text}");
        } else {
            assert_eq!(after.status.code(), Some(1), "{text}");
            assert!(
                text.contains("nothing to record: no entity matches the server"),
                "{text}"
            );
            assert!(text.contains("Things/Acme.Manager  both-changed"), "{text}");
            let saved: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join(".twaco/baselines/default.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                saved["entities"]["Things"]["Acme.Manager"]["local"],
                "v14:old"
            );
        }
        server.join().unwrap();
    }
}

#[test]
fn entity_get_never_creates_or_changes_the_baseline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("get-baseline");
    profile(&root, listener.local_addr().unwrap());
    std::fs::create_dir_all(root.join("Things")).unwrap();
    let xml =
        "<Entities><Things><Thing name=\"T\" projectName=\"Test\"></Thing></Things></Entities>";
    std::fs::write(root.join("Things/T.xml"), xml).unwrap();
    std::fs::create_dir_all(root.join(".twaco/baselines")).unwrap();
    let baseline_path = root.join(".twaco/baselines/default.json");
    let baseline = b"{\n  \"sentinel\": \"must stay byte-identical\"\n}\n";
    std::fs::write(&baseline_path, baseline).unwrap();
    let response = xml.to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = receive(&mut stream);
        assert!(String::from_utf8_lossy(&request).starts_with("GET /Thingworx/Exporter/Things/T "));
        answer(&mut stream, "200 OK", "text/xml; charset=utf-8", &response);
    });

    let output = run(&root, &["entity", "get", "T", "--out", "fetched.xml"]);
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read(&baseline_path).unwrap(), baseline);
    assert_eq!(
        std::fs::read_to_string(root.join("fetched.xml")).unwrap(),
        xml
    );
    server.join().unwrap();
}

#[test]
fn bare_and_qualified_targets_encode_segments_and_send_the_required_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("wire");
    profile(&root, listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            requests.push(receive(&mut stream));
            answer(&mut stream, "200 OK", "application/json", "{}");
        }
        requests
    });

    assert!(run(&root, &["call", "A? Thing", "Do/It", "--apply"])
        .status
        .success());
    assert!(run(
        &root,
        &[
            "call",
            "Resources/Entity Services",
            "List Things",
            r#"{"type":"Project"}"#,
            "--apply"
        ],
    )
    .status
    .success());
    let requests = server.join().unwrap();
    let first = String::from_utf8(requests[0].clone()).unwrap();
    let second = String::from_utf8(requests[1].clone()).unwrap();
    assert!(
        first.starts_with("POST /Thingworx/Things/A%3F%20Thing/Services/Do%2FIt HTTP/1.1"),
        "{first}"
    );
    assert!(second.starts_with(
        "POST /Thingworx/Resources/Entity%20Services/Services/List%20Things HTTP/1.1"
    ));
    for request in [&first, &second] {
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("accept: application/json"));
        assert!(lower.contains("content-type: application/json"));
        assert!(lower.contains("authorization: basic dxnlcjpwyxnz"));
        assert!(lower.contains("x-xsrf-token: twx-xsrf-token-value"));
    }
    assert!(first.ends_with("{}"));
    assert!(second.ends_with(r#"{"type":"Project"}"#));
}

#[test]
fn a_short_name_is_resolved_against_the_solution_and_said_aloud() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("resolve");
    profile(&root, listener.local_addr().unwrap());
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::write(
        root.join("Things/Acme.Test.Manager.xml"),
        "<Entities><Things><Thing name=\"Acme.Test.Manager\" projectName=\"Test\"></Thing></Things></Entities>\n",
    )
    .unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            requests.push(receive(&mut stream));
            answer(&mut stream, "200 OK", "application/json", "{}");
        }
        requests
    });

    // The platform's own Manager is reached by its collection; the solution's by its last segment.
    let short = run(&root, &["call", "Manager", "Do", "--apply"]);
    let platform = run(&root, &["call", "Things/Manager", "Do", "--apply"]);
    let requests = server.join().unwrap();
    let first = String::from_utf8(requests[0].clone()).unwrap();
    let second = String::from_utf8(requests[1].clone()).unwrap();
    assert!(
        first.starts_with("POST /Thingworx/Things/Acme.Test.Manager/Services/Do "),
        "{first}"
    );
    assert!(
        second.starts_with("POST /Thingworx/Things/Manager/Services/Do "),
        "{second}"
    );
    let said = String::from_utf8_lossy(&short.stderr);
    assert!(
        said.contains("twaco: calling Things/Acme.Test.Manager"),
        "{said}"
    );
    let said = String::from_utf8_lossy(&platform.stderr);
    assert!(
        !said.contains("twaco: calling"),
        "an explicit target needs no announcement: {said}"
    );
}

#[test]
fn void_summary_detail_invalid_json_and_http_error_have_distinct_results() {
    let replies = [
        ("200 OK", "application/json", ""),
        (
            "200 OK",
            "application/json",
            r#"{"dataShape":{"fieldDefinitions":{"name":{"baseType":"STRING"},"ok":{"baseType":"BOOLEAN"}}},"rows":[{"name":"one","ok":true},{"name":"two","ok":false}]}"#,
        ),
        (
            "200 OK",
            "application/json",
            r#"{"dataShape":{"fieldDefinitions":{"name":{"baseType":"STRING"}}},"rows":[{"name":"one"}]}"#,
        ),
        ("200 OK", "text/plain", "not json"),
        ("500 Error", "text/plain", "No service handler defined"),
    ];
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("results");
    profile(&root, listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for (status, content_type, body) in replies {
            let (mut stream, _) = listener.accept().unwrap();
            receive(&mut stream);
            answer(&mut stream, status, content_type, body);
        }
    });

    let void = run(&root, &["call", "T", "Void", "--apply"]);
    assert!(void.status.success());
    assert_eq!(String::from_utf8(void.stdout).unwrap(), "done\n");

    let summary = run(&root, &["call", "T", "Table", "--apply"]);
    let summary = String::from_utf8(summary.stdout).unwrap();
    assert!(summary.contains("2 row(s)"));
    assert!(summary.contains("fields: name, ok"));
    assert!(summary.contains("first row:"));
    assert!(summary.contains(r#""name": "one""#));
    assert!(!summary.contains("two"));

    let detail = run(&root, &["call", "T", "Table", "--detail", "--apply"]);
    let detail = String::from_utf8(detail.stdout).unwrap();
    assert!(detail.contains("fieldDefinitions"));
    assert!(detail.contains(r#""rows": ["#));

    let invalid = run(&root, &["call", "T", "Text", "--apply"]);
    let invalid_error = String::from_utf8(invalid.stderr).unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid_error.contains("invalid response"));

    let http = run(&root, &["call", "T", "Missing", "--apply"]);
    let http_error = String::from_utf8(http.stderr).unwrap();
    assert_eq!(http.status.code(), Some(2));
    assert!(http_error.contains("HTTP 500"));
    assert!(http_error.contains("No service handler defined"));
    assert!(!http_error.contains("pass"));
    server.join().unwrap();
}

#[test]
fn json_call_writes_one_complete_value_and_keeps_logs_on_stderr() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("json");
    profile(&root, listener.local_addr().unwrap());
    // Answer by request path: `--with-logs` polls the logs a varying number of times.
    let server = std::thread::spawn(move || loop {
        let (mut stream, _) = listener.accept().unwrap();
        let request = receive(&mut stream);
        let line = String::from_utf8_lossy(&request)
            .lines()
            .next()
            .unwrap_or_default()
            .to_string();
        let body = if line.contains("/Services/Table") {
            r#"{"dataShape":{"fieldDefinitions":{"name":{"baseType":"STRING"}}},"rows":[{"name":"one"},{"name":"two"}]}"#
        } else if line.contains("/Services/Void") {
            ""
        } else {
            r#"{"rows":[]}"#
        };
        answer(&mut stream, "200 OK", "application/json", body);
        if line.contains("/Services/Void") {
            break;
        }
    });

    let table = run(
        &root,
        &["call", "T", "Table", "--json", "--with-logs", "--apply"],
    );
    assert!(table.status.success());
    let value: serde_json::Value = serde_json::from_slice(&table.stdout).unwrap();
    assert_eq!(value["rows"].as_array().unwrap().len(), 2);
    assert!(String::from_utf8_lossy(&table.stderr).contains("logged during the call"));

    let void = run(&root, &["call", "T", "Void", "--json", "--apply"]);
    assert!(void.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&void.stdout).unwrap(),
        serde_json::Value::Null
    );
    server.join().unwrap();
}

#[test]
fn non_object_parameters_are_refused_without_a_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let (_dir, root) = temp("object");
    profile(&root, listener.local_addr().unwrap());
    for value in ["[1]", r#""x""#] {
        let output = run(&root, &["call", "T", "S", value]);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("must be a JSON object"));
    }
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[test]
fn request_timeout_overrides_the_agent_default() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("timeout");
    profile(&root, listener.local_addr().unwrap());
    // The server stalls far longer than the 1 s timeout, so a pass cannot be the response
    // arriving late. The margin absorbs process start-up under a loaded test run; the thread
    // is not joined, so the test does not wait out the stall.
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        receive(&mut stream);
        std::thread::sleep(Duration::from_secs(20));
        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    });
    let started = Instant::now();
    let output = run(&root, &["call", "T", "Slow", "--timeout", "1", "--apply"]);
    let elapsed = started.elapsed();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not be reached"));
    assert!(
        elapsed < Duration::from_secs(8),
        "timeout took {elapsed:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn appkey_show_stdout_is_only_the_key_and_summary_is_on_stderr() {
    for create in [false, true] {
        for save in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let (_dir, root) = temp("appkey-show");
            profile(&root, listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let request = receive(&mut stream);
                assert!(String::from_utf8_lossy(&request)
                    .starts_with("GET /Thingworx/ApplicationKeys/Acme.Key"));
                answer(
                    &mut stream,
                    if create { "404 Not Found" } else { "200 OK" },
                    "application/json",
                    "{}",
                );
                let (mut stream, _) = listener.accept().unwrap();
                let request = receive(&mut stream);
                let request = String::from_utf8_lossy(&request);
                if create {
                    assert!(request.contains("/CreateApplicationKey"));
                    answer(&mut stream, "200 OK", "application/json", "{}");
                } else {
                    assert!(request.contains("/GetExpirationDate"));
                    answer(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        r#"{"rows":[{"ExpirationDate":1796119200000}]}"#,
                    );
                }
                let (mut stream, _) = listener.accept().unwrap();
                assert!(String::from_utf8_lossy(&receive(&mut stream)).contains("/GetKeyID"));
                answer(
                    &mut stream,
                    "200 OK",
                    "application/json",
                    r#"{"rows":[{"KeyID":"fake-application-key"}]}"#,
                );
            });
            let mut args = vec![
                "appkey",
                if create { "create" } else { "show" },
                "Acme.Key",
                "--show",
            ];
            if create {
                args.extend(["--user", "user", "--apply"]);
            }
            if save {
                args.extend(["--save", "default"]);
            }
            let output = run(&root, &args);
            assert!(output.status.success(), "{output:?}");
            server.join().unwrap();
            assert_eq!(output.stdout, b"fake-application-key\n");
            let stderr = String::from_utf8_lossy(&output.stderr);
            let verb = if create { "created" } else { "found" };
            assert!(
                stderr.contains(&format!("{verb} application key Acme.Key, expiring")),
                "{stderr}"
            );
            assert_eq!(stderr.contains("saved as app_key in"), save, "{stderr}");
            assert!(!stderr.contains("fake-application-key"));
        }
    }
}

#[test]
fn repeated_params_send_strings_without_a_signature_and_refuse_json_mixing() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("params");
    profile(&root, listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = receive(&mut stream);
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\n{}",
            )
            .unwrap();
        request
    });
    let output = run(
        &root,
        &[
            "call", "T.S", "--param", "a=1", "--param", "b=x=y", "--apply",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let request = server.join().unwrap();
    let at = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let parameters: serde_json::Value = serde_json::from_slice(&request[at..]).unwrap();
    assert_eq!(parameters, serde_json::json!({"a":"1", "b":"x=y"}));
    let output = run(&root, &["call", "T.S", "{}", "--param", "a=1"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("use either key=value pairs or one JSON object"));
}

/// Serve `replies` canned answers on a thread and return the request lines it saw.
fn serve_lines(listener: TcpListener, replies: usize) -> std::thread::JoinHandle<Vec<String>> {
    std::thread::spawn(move || {
        let mut lines = Vec::new();
        for _ in 0..replies {
            let (mut stream, _) = listener.accept().unwrap();
            let request = receive(&mut stream);
            lines.push(
                String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
            answer(&mut stream, "200 OK", "application/json", r#"{"ok":1}"#);
        }
        lines
    })
}

fn nothing_reaches(listener: &TcpListener) {
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "a request reached the server"
    );
}

#[test]
fn a_read_by_name_prefix_runs_without_apply_and_prints_the_bare_result() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("read-name");
    profile(&root, listener.local_addr().unwrap());
    let server = serve_lines(listener, 1);
    let output = run(&root, &["call", "T", "GetThing", "--json"]);
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value, serde_json::json!({"ok": 1}));
    assert!(server.join().unwrap()[0].contains("/Things/T/Services/GetThing"));
}

#[test]
fn the_platform_category_team_list_and_read_flag_each_run_an_odd_name() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("read-odd");
    profile(&root, listener.local_addr().unwrap());
    std::fs::write(
        root.join(".twaco/platform.json"),
        r#"{"things":{"T":{"services":{"Compute":{"category":"READ","result":{"baseType":"NOTHING"}}}}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("twaco.toml"),
        "[[project]]\nname = \"Test\"\n[call]\nread = [\"T.Listed\"]\n",
    )
    .unwrap();
    let server = serve_lines(listener, 3);
    for service in ["Compute", "Listed"] {
        assert!(run(&root, &["call", "T", service]).status.success());
    }
    assert!(run(&root, &["call", "T", "Unlisted", "--read"])
        .status
        .success());
    let lines = server.join().unwrap();
    for (line, service) in lines.iter().zip(["Compute", "Listed", "Unlisted"]) {
        assert!(line.contains(&format!("/Services/{service} ")), "{line}");
    }
}

#[test]
fn a_write_without_apply_sends_nothing_and_says_why() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let (_dir, root) = temp("dry");
    profile(&root, listener.local_addr().unwrap());
    let output = run(
        &root,
        &[
            "call",
            "Resources/EntityServices",
            "DeleteDataTagVocabulary",
            "--param",
            "name=X",
            "--json",
        ],
    );
    assert!(output.status.success());
    nothing_reaches(&listener);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["called"], false);
    assert_eq!(value["dry_run"], true);
    assert!(value["reason"].as_str().unwrap().contains("[call] read"));
    assert_eq!(
        value["would_call"],
        "POST Resources/EntityServices/Services/DeleteDataTagVocabulary"
    );
    assert_eq!(value["params"], serde_json::json!({"name": "X"}));
    let text = run(
        &root,
        &[
            "call",
            "Resources/EntityServices",
            "DeleteDataTagVocabulary",
        ],
    );
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.starts_with(
        "not called (dry run); pass --apply to call Resources/EntityServices.DeleteDataTagVocabulary\n"
    ));
    nothing_reaches(&listener);
    listener.set_nonblocking(false).unwrap();
    let server = serve_lines(listener, 1);
    let applied = run(
        &root,
        &[
            "call",
            "Resources/EntityServices",
            "DeleteDataTagVocabulary",
            "--apply",
        ],
    );
    assert!(applied.status.success());
    assert!(server.join().unwrap()[0].contains("/Services/DeleteDataTagVocabulary"));
}

#[test]
fn a_get_named_resource_service_off_the_built_in_list_is_a_write() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let (_dir, root) = temp("resource");
    profile(&root, listener.local_addr().unwrap());
    let output = run(
        &root,
        &[
            "call",
            "Resources/ContentLoaderFunctions",
            "GetJSON",
            "--json",
        ],
    );
    assert!(output.status.success());
    nothing_reaches(&listener);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["called"], false);
}
