use super::profile::Secret;
use std::io::Read;
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;

/// What running a child produced.
pub(crate) struct Finished {
    pub(crate) status: Option<ExitStatus>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) timed_out: bool,
    pub(crate) truncated: bool,
}

/// Run a child to completion, killing it if it outstays the deadline.
///
/// Both pipes are drained on their own threads from the moment the child starts. Waiting first
/// and reading afterwards deadlocks as soon as a child writes more than the operating system's
/// pipe buffer, which is far smaller than the output cap: the child blocks writing, the wait
/// blocks on the child, and the timeout fires on a program that was only being chatty.
pub(crate) fn run_to_completion(
    mut child: Child,
    timeout: Duration,
    output_cap: usize,
) -> Result<Finished, String> {
    let out_reader = spawn_reader(child.stdout.take(), output_cap);
    let err_reader = spawn_reader(child.stderr.take(), output_cap);

    let deadline = std::time::Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    // Killed rather than abandoned: a detached child would outlive the run and
                    // overlap whatever ran next.
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.to_string()),
        }
    };

    let (stdout, out_truncated) = out_reader
        .join()
        .map_err(|_| "reader thread panicked".to_string())?;
    let (stderr, err_truncated) = err_reader
        .join()
        .map_err(|_| "reader thread panicked".to_string())?;
    Ok(Finished {
        status,
        stdout,
        stderr,
        timed_out,
        truncated: out_truncated || err_truncated,
    })
}

/// Drain one pipe on its own thread, keeping at most `output_cap` bytes.
///
/// Past the cap the bytes are read and discarded rather than left in the pipe, so a child that
/// will not stop talking still reaches its own exit instead of blocking there forever.
pub(crate) fn spawn_reader<R: Read + Send + 'static>(
    source: Option<R>,
    output_cap: usize,
) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut truncated = false;
        let Some(mut source) = source else {
            return (kept, truncated);
        };
        let mut buffer = [0u8; 8192];
        loop {
            match source.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if kept.len() < output_cap {
                        let room = output_cap - kept.len();
                        kept.extend_from_slice(&buffer[..n.min(room)]);
                        if n > room {
                            truncated = true;
                        }
                    } else {
                        truncated = true;
                    }
                }
            }
        }
        (kept, truncated)
    })
}

/// The secret values twaco reads from its `TWACO_` and `TWX_` environment variables, including
/// URL userinfo whole, its password alone, and a decoded password.
pub(crate) fn credential_values() -> Vec<Secret> {
    let mut values = Vec::new();
    let mut userinfo = Vec::new();
    for (key, value) in std::env::vars() {
        let value = Secret::new(value);
        let key = key.to_ascii_uppercase();
        if !(key.starts_with("TWACO_") || key.starts_with("TWX_")) {
            continue;
        }
        if let Ok(url) = url::Url::parse(value.expose()) {
            if let Some(password) = url.password() {
                // The pair is replaced whatever its length: with the colon it cannot be mistaken.
                userinfo.push(Secret::new(format!("{}:{password}", url.username())));
                values.push(Secret::from(password));
                if let Ok(decoded) = percent_encoding::percent_decode_str(password).decode_utf8() {
                    values.push(Secret::new(decoded.into_owned()));
                }
            }
            // The parsed address holds the password too.
            let mut text: String = url.into();
            zeroize::Zeroize::zeroize(&mut text);
        }
        if ["PASSWORD", "KEY", "SECRET", "TOKEN"]
            .iter()
            .any(|word| key.contains(word))
        {
            values.push(value);
        }
    }
    // A short value alone would be replaced wherever those letters appear.
    values.retain(|value| value.expose().len() >= 4);
    values.extend(userinfo);
    values.sort_by(|a, b| a.expose().cmp(b.expose()));
    values.dedup();
    // Longest first, so a secret that contains another is replaced whole.
    values.sort_by_key(|value| std::cmp::Reverse(value.expose().len()));
    values
}

/// Remove the environment variables that may contain twaco credentials from a child command.
pub(crate) fn strip_credentials(command: &mut Command) {
    for (key, _) in std::env::vars() {
        let upper = key.to_ascii_uppercase();
        if upper.starts_with("TWACO_") || upper.starts_with("TWX_") {
            command.env_remove(key);
        }
    }
}

/// Replace every known secret with a stable marker before text can leave the process.
pub(crate) fn redact(text: &str, secrets: &[Secret]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        out = out.replace(secret.expose(), "<redacted>");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENVIRONMENT: Mutex<()> = Mutex::new(());

    struct EnvironmentVariable {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvironmentVariable {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvironmentVariable {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    #[test]
    fn strip_credentials_removes_twaco_and_twx_variables() {
        let _environment = ENVIRONMENT.lock().unwrap();
        let _password = EnvironmentVariable::set("TWACO_CHILD_TEST_PASSWORD", "password");
        let _appkey = EnvironmentVariable::set("TWX_CHILD_TEST_APPKEY", "appkey");
        let _other = EnvironmentVariable::set("CHILD_TEST_KEPT", "kept");
        let mut command = Command::new("test");

        strip_credentials(&mut command);

        let environment = command
            .get_envs()
            .map(|(key, value)| (key.to_os_string(), value.map(|value| value.to_os_string())))
            .collect::<Vec<_>>();
        assert!(environment
            .iter()
            .any(|(key, value)| { key == "TWACO_CHILD_TEST_PASSWORD" && value.is_none() }));
        assert!(environment
            .iter()
            .any(|(key, value)| key == "TWX_CHILD_TEST_APPKEY" && value.is_none()));
        assert!(!environment.iter().any(|(key, _)| key == "CHILD_TEST_KEPT"));
    }

    #[test]
    fn redact_replaces_every_secret() {
        let secrets = vec![Secret::from("first-secret"), Secret::from("second-secret")];
        let redacted = redact(
            "first-secret then second-secret then first-secret",
            &secrets,
        );
        assert_eq!(redacted, "<redacted> then <redacted> then <redacted>");
    }
}
