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
fn entity_get_never_creates_or_changes_the_baseline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("get-baseline");
    profile(&root, listener.local_addr().unwrap());
    std::fs::create_dir_all(root.join("Things")).unwrap();
    let xml =
        "<Entities><Things><Thing name=\"T\" projectName=\"Test\"></Thing></Things></Entities>";
    std::fs::write(root.join("Things/T.xml"), xml).unwrap();
    let baseline_path = root.join(".twaco/baseline.json");
    let baseline = b"{\n  \"sentinel\": \"must stay byte-identical\"\n}\n";
    std::fs::write(&baseline_path, baseline).unwrap();
    let response = xml.to_string();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = receive(&mut stream);
        assert!(String::from_utf8_lossy(&request).starts_with("GET /Thingworx/Things/T "));
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

    assert!(run(&root, &["call", "A? Thing", "Do/It"]).status.success());
    assert!(run(
        &root,
        &[
            "call",
            "Resources/Entity Services",
            "List Things",
            r#"{"type":"Project"}"#
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
    let short = run(&root, &["call", "Manager", "Do"]);
    let platform = run(&root, &["call", "Things/Manager", "Do"]);
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

    let void = run(&root, &["call", "T", "Void"]);
    assert!(void.status.success());
    assert_eq!(String::from_utf8(void.stdout).unwrap(), "done\n");

    let summary = run(&root, &["call", "T", "Table"]);
    let summary = String::from_utf8(summary.stdout).unwrap();
    assert!(summary.contains("2 row(s)"));
    assert!(summary.contains("fields: name, ok"));
    assert!(summary.contains("first row:"));
    assert!(summary.contains(r#""name": "one""#));
    assert!(!summary.contains("two"));

    let detail = run(&root, &["call", "T", "Table", "--detail"]);
    let detail = String::from_utf8(detail.stdout).unwrap();
    assert!(detail.contains("fieldDefinitions"));
    assert!(detail.contains(r#""rows": ["#));

    let invalid = run(&root, &["call", "T", "Text"]);
    let invalid_error = String::from_utf8(invalid.stderr).unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid_error.contains("invalid response"));

    let http = run(&root, &["call", "T", "Missing"]);
    let http_error = String::from_utf8(http.stderr).unwrap();
    assert_eq!(http.status.code(), Some(2));
    assert!(http_error.contains("HTTP 500"));
    assert!(http_error.contains("No service handler defined"));
    assert!(!http_error.contains("pass"));
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
    let output = run(&root, &["call", "T", "Slow", "--timeout", "1"]);
    let elapsed = started.elapsed();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not be reached"));
    assert!(
        elapsed < Duration::from_secs(8),
        "timeout took {elapsed:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
