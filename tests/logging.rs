//! Diagnostic logs: off by default, never on stdout, never holding a credential.
//! Servers are local listeners; nothing here touches the network.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn temp(label: &str) -> (tempfile::TempDir, PathBuf) {
    let guard = tempfile::Builder::new()
        .prefix(&format!("twaco-log-{label}-"))
        .tempdir()
        .unwrap();
    let root = guard.path().to_path_buf();
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Test\"\n").unwrap();
    (guard, root)
}

fn command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_twaco"));
    command
        .args(args)
        .current_dir(root)
        .env("TWACO_NO_UPDATE_CHECK", "1")
        .env_remove("TWACO_LOG")
        .env_remove("TWACO_LOG_FILE")
        .env_remove("TWACO_URL")
        .env_remove("TWACO_USERNAME")
        .env_remove("TWACO_PASSWORD")
        .env_remove("TWX_URL")
        .env_remove("TWX_USERNAME")
        .env_remove("TWX_PASSWORD");
    command
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn lines_with(output: &Output, needle: &str) -> usize {
    text(&output.stderr)
        .lines()
        .filter(|line| line.contains(needle))
        .count()
}

#[test]
fn logs_are_off_unless_asked_for() {
    let (_dir, root) = temp("off");
    let output = command(&root, &["projects"]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(text(&output.stderr), "");
}

#[test]
fn mcp_keeps_stdout_pure_json_with_logs_at_trace() {
    let (_dir, root) = temp("mcp");
    let mut child = command(&root, &["mcp"])
        .env("TWACO_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"projects","arguments":{"marker":"do-not-log"}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#,
        "not json",
    ];
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = text(&output.stdout);
    let replies: Vec<&str> = stdout.lines().collect();
    assert_eq!(replies.len(), requests.len(), "{stdout}");
    for line in &replies {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line}"));
    }
    let logs = text(&output.stderr);
    assert!(logs.contains("method=\"initialize\""), "{logs}");
    assert!(logs.contains("tool=\"projects\""), "{logs}");
    assert!(logs.contains("code=-32601"), "{logs}");
    assert!(
        !logs.contains("do-not-log"),
        "arguments are never logged: {logs}"
    );
}

#[test]
fn a_log_file_takes_the_logs_and_stderr_stays_empty() {
    let (_dir, root) = temp("file");
    let output = command(&root, &["projects", "--log-file", "run.log"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(text(&output.stderr), "");
    let logged = std::fs::read_to_string(root.join("run.log")).unwrap();
    // `--log-file` alone means debug, and the command line says it ran.
    assert!(logged.contains("command finished"), "{logged}");
    assert!(!text(&output.stdout).contains("command finished"));

    let again = command(&root, &["projects"])
        .env("TWACO_LOG_FILE", "run.log")
        .output()
        .unwrap();
    assert!(again.status.success());
    assert_eq!(text(&again.stderr), "");
    let logged = std::fs::read_to_string(root.join("run.log")).unwrap();
    assert_eq!(logged.matches("command finished").count(), 2, "{logged}");
}

#[test]
fn a_flag_beats_its_variable() {
    let (_dir, root) = temp("flag");
    let quiet = command(&root, &["projects", "--log", "warn"])
        .env("TWACO_LOG", "debug")
        .output()
        .unwrap();
    assert_eq!(lines_with(&quiet, "command finished"), 0);
    let loud = command(&root, &["projects", "--log", "debug"])
        .env("TWACO_LOG", "warn")
        .output()
        .unwrap();
    assert_eq!(lines_with(&loud, "command finished"), 1);

    let file = command(&root, &["projects", "--log-file", "flag.log"])
        .env("TWACO_LOG_FILE", "variable.log")
        .output()
        .unwrap();
    assert!(file.status.success());
    assert!(root.join("flag.log").exists());
    assert!(!root.join("variable.log").exists());
}

#[test]
fn a_bare_level_hides_dependency_events() {
    let (_dir, root) = temp("bare");
    let output = command(&root, &["projects", "--log", "trace"])
        .output()
        .unwrap();
    for line in text(&output.stderr).lines() {
        assert!(line.contains(" twaco"), "a dependency logged: {line}");
    }
}

#[test]
fn an_invalid_filter_warns_once_and_the_command_succeeds() {
    let (_dir, root) = temp("invalid");
    let plain = command(&root, &["projects"]).output().unwrap();
    let output = command(&root, &["projects", "--log", "twaco=loud"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, plain.stdout);
    let said = text(&output.stderr);
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(said.starts_with("twaco: ignoring the log filter"), "{said}");
}

#[test]
fn an_unopenable_log_file_warns_once_and_the_command_succeeds() {
    let (_dir, root) = temp("unopenable");
    let plain = command(&root, &["projects"]).output().unwrap();
    let output = command(
        &root,
        &["projects", "--log-file", "missing-directory/run.log"],
    )
    .output()
    .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, plain.stdout);
    let said = text(&output.stderr);
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(said.starts_with("twaco: ignoring the log file"), "{said}");
}

fn read_request(stream: &mut std::net::TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0u8; 8192];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0);
        request.extend_from_slice(&buffer[..read]);
    }
}

#[test]
fn no_form_of_any_secret_reaches_a_log() {
    const PASSWORD: &str = "Zq9!x&y z/7\"q";
    const APP_KEY: &str = "AppKey-9f8e7d6c";
    const TOKEN: &str = "tok-3c4b5a29";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (_dir, root) = temp("secrets");
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url = \"http://{}/Thingworx/\"\nusername = \"alice\"\npassword = '{PASSWORD}'\n\
             app_key = \"{APP_KEY}\"\napi_token = \"{TOKEN}\"\n",
            listener.local_addr().unwrap()
        ),
    )
    .unwrap();
    // The server echoes the password in its error page, and the request URL holds it too.
    let page = format!("denied for {PASSWORD} with {APP_KEY} and {TOKEN}");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        write!(
            stream,
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
            page.len()
        )
        .unwrap();
    });
    let thing = format!("T-{PASSWORD}-{APP_KEY}-{TOKEN}");
    // The log file holds only log lines. Stderr also holds twaco's own error message.
    let output = command(&root, &["call", &thing, "Do"])
        .env("TWACO_LOG", "trace")
        .env("TWACO_LOG_FILE", "run.log")
        .output()
        .unwrap();
    server.join().unwrap();
    assert_eq!(output.status.code(), Some(2));

    let logs = std::fs::read_to_string(root.join("run.log")).unwrap();
    assert!(
        logs.contains("server request"),
        "the request is logged: {logs}"
    );
    assert!(logs.contains("<redacted>"), "the URL is scrubbed: {logs}");
    let encode = |text: &str| -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect()
    };
    let json = serde_json::to_string(PASSWORD).unwrap();
    let pair = format!("alice:{PASSWORD}");
    let basic = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&pair)
    };
    let forms = [
        PASSWORD.to_string(),
        json.trim_matches('"').to_string(),
        encode(PASSWORD),
        pair,
        basic,
        APP_KEY.to_string(),
        encode(APP_KEY),
        TOKEN.to_string(),
    ];
    for form in forms {
        assert!(!logs.contains(&form), "{form:?} reached the log:\n{logs}");
    }
}

#[test]
fn a_string_message_id_never_reaches_a_log() {
    let (_dir, root) = temp("mcp-id");
    let mut child = command(&root, &["mcp"])
        .env("TWACO_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":"id-secret-4e7a","method":"tools/list"}}"#
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":7,"method":"tools/list"}}"#).unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let logs = text(&output.stderr);
    assert!(!logs.contains("id-secret-4e7a"), "{logs}");
    assert!(logs.contains("id=<string>"), "{logs}");
    assert!(logs.contains("id=7"), "{logs}");
}

#[cfg(unix)]
#[test]
fn a_log_file_that_is_standard_output_is_refused() {
    let (_dir, root) = temp("stdout");
    let mut child = command(&root, &["mcp", "--log-file", "/dev/stdout"])
        .env("TWACO_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
    )
    .unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = text(&output.stdout);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    for line in stdout.lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|error| panic!("stdout line is not JSON ({error}): {line}"));
    }
    let said = text(&output.stderr);
    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(
        said.starts_with("twaco: ignoring the log file /dev/stdout"),
        "{said}"
    );
}

#[test]
fn a_failed_connection_logs_no_form_of_any_secret() {
    const PASSWORD: &str = "Zq9!x&y z/7\"q";
    const APP_KEY: &str = "AppKey-9f8e7d6c";
    const TOKEN: &str = "tok-3c4b5a29";
    // Nothing listens on this port once the listener is gone.
    let address = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let (_dir, root) = temp("transport");
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url = \"http://{address}/Thingworx/\"\nusername = \"alice\"\n\
             password = '{PASSWORD}'\napp_key = \"{APP_KEY}\"\napi_token = \"{TOKEN}\"\n"
        ),
    )
    .unwrap();
    let thing = format!("T-{PASSWORD}-{APP_KEY}-{TOKEN}");
    let output = command(&root, &["call", &thing, "Do"])
        .env("TWACO_LOG", "trace")
        .env("TWACO_LOG_FILE", "run.log")
        .output()
        .unwrap();
    assert!(!output.status.success());

    let logs = std::fs::read_to_string(root.join("run.log")).unwrap();
    assert!(logs.contains("server request failed"), "{logs}");
    let encode = |text: &str| -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect()
    };
    let json = serde_json::to_string(PASSWORD).unwrap();
    let forms = [
        PASSWORD.to_string(),
        json.trim_matches('"').to_string(),
        encode(PASSWORD),
        APP_KEY.to_string(),
        encode(APP_KEY),
        TOKEN.to_string(),
    ];
    for form in forms {
        assert!(!logs.contains(&form), "{form:?} reached the log:\n{logs}");
    }
}
