//! Progress bars draw on a terminal only. With stderr piped, nothing is drawn and stdout is the
//! same as ever.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace(address: std::net::SocketAddr) -> (tempfile::TempDir, PathBuf) {
    let guard = tempfile::Builder::new()
        .prefix("twaco-progress-")
        .tempdir()
        .unwrap();
    let root = guard.path().to_path_buf();
    std::fs::create_dir_all(root.join(".twaco/profiles")).unwrap();
    std::fs::create_dir_all(root.join("Things")).unwrap();
    std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"Test\"\n").unwrap();
    std::fs::write(
        root.join(".twaco/profiles/default.toml"),
        format!(
            "url = \"http://{address}/Thingworx/\"\nusername = \"user\"\npassword = \"pass\"\n"
        ),
    )
    .unwrap();
    for name in ["A", "B", "C"] {
        std::fs::write(
            root.join(format!("Things/{name}.xml")),
            format!("<Entities><Things><Thing name=\"{name}\" projectName=\"Test\"></Thing></Things></Entities>"),
        )
        .unwrap();
    }
    (guard, root)
}

/// A server that answers every entity read with that entity, until the test ends.
fn serve() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buffer = [0u8; 8192];
            let read = stream.read(&mut buffer).unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let path = request.split_whitespace().nth(1).unwrap_or("");
            let name = path.rsplit('/').next().unwrap_or("");
            let body = format!(
                "<Entities><Things><Thing name=\"{name}\" projectName=\"Test\"></Thing></Things></Entities>"
            );
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    address
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_twaco"))
        .args(args)
        .current_dir(root)
        .env_remove("TWACO_LOG")
        .env_remove("TWACO_LOG_FILE")
        .output()
        .unwrap()
}

#[test]
fn a_piped_stderr_shows_no_progress_and_stdout_is_plain() {
    let (_guard, root) = workspace(serve());
    let output = run(&root, &["entity", "status", "--all"]);
    assert!(
        output.status.success() || output.status.code() == Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // No bar, no spinner, no cursor control: nothing at all on stderr.
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "3 entity status(es)\n  in-sync             0\n  local-changed       0\n  \
         server-changed      0\n  both-changed        0\n  not-on-server       0\n  \
         no-baseline-same    3\n  no-baseline-differs 0\n"
    );
}
