//! Blocking ThingWorx HTTP transport.
//!
//! The transport covers entity export/import, the live Rhino parse used by deploy, and generic
//! service calls, plus the two guarded entity-delete transports. There is deliberately no PUT
//! to reach for. HTTPS uses the operating
//! system trust store through ureq's platform verifier, and redirects are refused so basic-auth
//! credentials are never carried to a host the server names.

use super::entity_key::{EntityKey, ServiceTarget};
use super::profile::Profile;
use serde::Deserialize;
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const XSRF_VALUE: &str = "TWX-XSRF-TOKEN-VALUE";

/// The Importer's observed flags. Without
/// `overwriteConfigurationTableValues` an edited configuration table imports and silently keeps
/// its old rows.
const IMPORT_QUERY: &str =
    "purpose=import&usedefaultdataprovider=false&usedefaultqueueprovider=false\
&WithSubsystems=false&IgnoreBadValueStreamData=false&overwriteConfigurationTableValues=true\
&overwritePropertyValues=true";

#[derive(Clone, Debug)]
pub struct Client {
    profile: Profile,
    /// Built once, so every request reuses its connection pool. A deploy makes hundreds of
    /// requests, and a new agent per request meant a new connection each time.
    agent: ureq::Agent,
    /// What an error message must never repeat: see [`secrets_of`].
    secrets: Vec<String>,
}

impl Client {
    pub fn new(profile: Profile) -> Self {
        use ureq::tls::{RootCerts, TlsConfig};

        let tls_config = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .max_redirects(0)
            .tls_config(tls_config)
            .user_agent(concat!("twaco/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let secrets = secrets_of(&profile);
        Self {
            profile,
            agent,
            secrets,
        }
    }

    /// Fetch `GET <base>/<Collection>/<Name>` as raw, validated UTF-8 XML bytes.
    pub fn fetch_entity(&self, key: &EntityKey) -> Result<Vec<u8>, ServerError> {
        // MediaEntities' ordinary REST resource is the media bytes, even with `Accept: text/xml`.
        // ThingWorx's read-only Exporter route returns the entity XML when the entity
        // representation (rather than its REST view) is required.
        let exporter = if key.collection() == "MediaEntities" {
            "Exporter/"
        } else {
            ""
        };
        let url = format!("{}/{exporter}{}", self.base(), key.url_path());
        let authorization = self.authorization();
        let headers = [
            ("Accept", "text/xml"),
            ("Authorization", authorization.as_str()),
            // Composer sends its JSON content-negotiation defaults even on a GET.
            // MediaEntities otherwise return their media payload instead of an XML export.
            ("Content-Type", "application/json"),
            ("X-XSRF-TOKEN", XSRF_VALUE),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport(&self.agent, Method::Get, &url, &headers, None)?;
        checked(&self.secrets, Method::Get, url, response)
    }

    /// Fetch the ordinary REST representation of an entity as JSON. This is distinct from
    /// `fetch_entity`, whose export XML deliberately excludes live configuration values.
    pub fn fetch_entity_json(&self, key: &EntityKey) -> Result<serde_json::Value, ServerError> {
        let url = format!("{}/{}", self.base(), key.url_path());
        let authorization = self.authorization();
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("Content-Type", "application/json"),
            ("X-XSRF-TOKEN", XSRF_VALUE),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport(&self.agent, Method::Get, &url, &headers, None)?;
        let bytes = checked(&self.secrets, Method::Get, url.clone(), response)?;
        serde_json::from_slice(&bytes).map_err(|error| ServerError::InvalidResponse {
            url,
            why: error.to_string(),
        })
    }

    /// The names of every entity of a collection, from its REST listing.
    pub fn list_entity_names(&self, collection: &str) -> Result<Vec<String>, ServerError> {
        let url = format!("{}/{}", self.base(), encode_path_segment(collection));
        let authorization = self.authorization();
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("X-XSRF-TOKEN", XSRF_VALUE),
        ];
        let response = transport(&self.agent, Method::Get, &url, &headers, None)?;
        let bytes = checked(&self.secrets, Method::Get, url.clone(), response)?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| ServerError::InvalidResponse {
                url: url.clone(),
                why: error.to_string(),
            })?;
        let rows = value
            .get("rows")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| ServerError::InvalidResponse {
                url,
                why: "the listing has no rows".to_string(),
            })?;
        Ok(rows
            .iter()
            .filter_map(|row| row.get("name").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect())
    }

    /// Whether the server has an entity: its REST address answers 404 when it does not. The
    /// Exporter cannot say (it answers 200 with an empty export), and for MediaEntities
    /// `fetch_entity` goes through the Exporter, so this asks the plain address for JSON.
    pub fn entity_exists(&self, key: &EntityKey) -> Result<bool, ServerError> {
        let url = format!("{}/{}", self.base(), key.url_path());
        let authorization = self.authorization();
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("X-XSRF-TOKEN", XSRF_VALUE),
        ];
        let response = transport(&self.agent, Method::Get, &url, &headers, None)?;
        match checked_bytes(&self.secrets, Method::Get, url, response) {
            Ok(_) => Ok(true),
            Err(error) if error.is_not_found() => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Delete through Composer's REST route. The repeated content-negotiation query parameters
    /// and XMLHttpRequest header are required: without them DataShapes and Mashups answer 500.
    pub fn delete_entity_rest(&self, key: &EntityKey) -> Result<(), ServerError> {
        let url = format!(
            "{}/{}?Accept=application%2Fjson&Content-Type=application%2Fjson",
            self.base(),
            key.url_path()
        );
        let authorization = self.authorization();
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("Content-Type", "application/json"),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport(&self.agent, Method::Delete, &url, &headers, None)?;
        checked(&self.secrets, Method::Delete, url, response).map(|_| ())
    }

    /// A file of a FileRepository, exactly as stored: the `FileRepositories` servlet that
    /// Composer's download links point at. `path` is slash-separated from the repository root.
    pub fn download_file(&self, repository: &str, path: &str) -> Result<Vec<u8>, ServerError> {
        let segments: Vec<String> = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(encode_path_segment)
            .collect();
        let url = format!(
            "{}/FileRepositories/{}/{}",
            self.base(),
            encode_path_segment(repository),
            segments.join("/")
        );
        let authorization = self.authorization();
        let headers = [
            ("Authorization", authorization.as_str()),
            ("X-XSRF-TOKEN", XSRF_VALUE),
        ];
        let response = transport(&self.agent, Method::Get, &url, &headers, None)?;
        checked_bytes(&self.secrets, Method::Get, url, response)
    }

    /// Import one entity document through the Importer, sending its bytes exactly as given.
    ///
    /// The observed failure for an import that cannot succeed is an HTTP error (406), but a 2xx
    /// is not taken on trust either: anything other than the body `success` is a failure.
    /// Neither says the server *kept* what was sent, since a configuration table
    /// its template does not define is dropped while the import still says `success`. Only
    /// reading the entity back can show that, which is the caller's job.
    pub fn import_entity(&self, file_name: &str, xml: &[u8]) -> Result<(), ServerError> {
        self.import_with(file_name, xml, IMPORT_QUERY)
    }

    /// An export file (XML, or a zip of them) through the Importer, as Composer's Import from
    /// File does. Composer's default is to keep the server's property values and configuration
    /// table rows; the flags say otherwise.
    pub fn import_file(
        &self,
        file_name: &str,
        bytes: &[u8],
        overwrite_property_values: bool,
        overwrite_configuration_tables: bool,
    ) -> Result<(), ServerError> {
        let query = format!(
            "purpose=import&usedefaultdataprovider=false&usedefaultqueueprovider=false&WithSubsystems=false\
             &IgnoreBadValueStreamData=false&overwritePropertyValues={overwrite_property_values}\
             &overwriteConfigurationTableValues={overwrite_configuration_tables}"
        );
        self.import_with(file_name, bytes, &query)
    }

    fn import_with(&self, file_name: &str, xml: &[u8], query: &str) -> Result<(), ServerError> {
        let url = format!("{}/Importer?{query}", self.base());
        let boundary = boundary_for(xml);
        let body = multipart(&boundary, file_name, xml);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let authorization = self.authorization();
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("Content-Type", content_type.as_str()),
            ("X-XSRF-TOKEN", XSRF_VALUE),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport(&self.agent, Method::Post, &url, &headers, Some(&body))?;
        let reply = checked(&self.secrets, Method::Post, url.clone(), response)?;
        let reply = String::from_utf8(reply).expect("checked validated UTF-8");
        if reply.trim().eq_ignore_ascii_case("success") {
            Ok(())
        } else {
            Err(ServerError::Rejected {
                url,
                body: scrub(&self.secrets, &excerpt(&reply)),
            })
        }
    }

    /// The `Exporter`'s XML for a route: `Things/X` (one entity), `Things` (a collection), or
    /// empty (everything), with `project` narrowing either of the last two to one project.
    pub fn export_xml(
        &self,
        collection: Option<&str>,
        name: Option<&str>,
        project: Option<&str>,
    ) -> Result<Vec<u8>, ServerError> {
        let mut url = format!("{}/Exporter", self.base());
        if let Some(collection) = collection {
            url.push('/');
            url.push_str(&encode_path_segment(collection));
            if let Some(name) = name {
                url.push('/');
                url.push_str(&encode_path_segment(name));
            }
        }
        if let Some(project) = project {
            url.push_str("?projectName=");
            url.push_str(&encode_path_segment(project));
        }
        let authorization = self.authorization();
        let headers = [
            ("Accept", "text/xml"),
            ("Authorization", authorization.as_str()),
            ("Content-Type", "application/json"),
            ("X-XSRF-TOKEN", XSRF_VALUE),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport_with_timeout(
            &self.agent,
            Method::Get,
            &url,
            &headers,
            None,
            Some(Duration::from_secs(600)),
        )?;
        checked(&self.secrets, Method::Get, url, response)
    }

    /// Send an extension package to `ExtensionPackageUploader`, as Composer's import dialog does.
    /// With `validate` the server checks the package and installs nothing; without, it installs.
    /// The answer is the server's JSON report. A package it cannot read is a bare 406.
    pub fn upload_extension(
        &self,
        file_name: &str,
        zip: &[u8],
        validate: bool,
    ) -> Result<serde_json::Value, ServerError> {
        let url = format!(
            "{}/ExtensionPackageUploader?purpose=import&validate={validate}",
            self.base()
        );
        let boundary = boundary_for(zip);
        let body = multipart(&boundary, file_name, zip);
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let authorization = self.authorization();
        let headers = [
            ("Accept", "*/*"),
            ("Authorization", authorization.as_str()),
            ("Content-Type", content_type.as_str()),
            ("X-XSRF-TOKEN", XSRF_VALUE),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport_with_timeout(
            &self.agent,
            Method::Post,
            &url,
            &headers,
            Some(&body),
            Some(Duration::from_secs(600)),
        )?;
        let reply = checked(&self.secrets, Method::Post, url.clone(), response)?;
        serde_json::from_slice(&reply).map_err(|error| ServerError::InvalidResponse {
            url,
            why: error.to_string(),
        })
    }

    /// Ask ThingWorx's Rhino parser to validate one service body.
    ///
    /// This is intentionally not a generic service-call escape hatch. Deploy is required to
    /// fail closed on this exact gate, and its four response fields are part of the contract.
    pub fn check_script(&self, script: &str) -> Result<ScriptCheck, ServerError> {
        const ROUTE: &str = "Resources/ScriptServices/Services/CheckScriptWithLinesAndColumns";
        let url = format!("{}/{ROUTE}", self.base());
        let body = serde_json::to_vec(&serde_json::json!({ "script": script }))
            .expect("a string always serialises as JSON");
        let authorization = self.authorization();
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("Content-Type", "application/json"),
            ("X-XSRF-TOKEN", XSRF_VALUE),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport(&self.agent, Method::Post, &url, &headers, Some(&body))?;
        let reply = checked(&self.secrets, Method::Post, url.clone(), response)?;
        let parsed: ScriptCheckResponse =
            serde_json::from_slice(&reply).map_err(|error| ServerError::InvalidResponse {
                url: url.clone(),
                why: error.to_string(),
            })?;
        if parsed.rows.len() != 1 {
            return Err(ServerError::InvalidResponse {
                url,
                why: format!("expected one row, got {}", parsed.rows.len()),
            });
        }
        Ok(parsed.rows.into_iter().next().expect("length checked"))
    }

    /// Execute one opaque ThingWorx service call.
    ///
    /// This call has no dry run: the command line names exactly what to execute, and the client
    /// cannot know whether an arbitrary service writes. The MCP surface must make its
    /// own, separate decision about how to expose service execution.
    pub fn call_service(
        &self,
        target: &ServiceTarget,
        service: &str,
        parameters: &serde_json::Value,
        timeout: Duration,
    ) -> Result<Option<serde_json::Value>, ServerError> {
        let url = format!(
            "{}/{}/Services/{}",
            self.base(),
            target.url_path(),
            encode_path_segment(service)
        );
        let body = serde_json::to_vec(parameters).expect("JSON values always serialise");
        let authorization = self.authorization();
        let headers = [
            ("Accept", "application/json"),
            ("Authorization", authorization.as_str()),
            ("Content-Type", "application/json"),
            ("X-XSRF-TOKEN", XSRF_VALUE),
            ("X-Requested-With", "XMLHttpRequest"),
        ];
        let response = transport_with_timeout(
            &self.agent,
            Method::Post,
            &url,
            &headers,
            Some(&body),
            Some(timeout),
        )?;
        let reply = checked(&self.secrets, Method::Post, url.clone(), response)?;
        if reply.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        serde_json::from_slice(&reply)
            .map(Some)
            .map_err(|error| ServerError::InvalidResponse {
                url,
                why: error.to_string(),
            })
    }

    fn base(&self) -> &str {
        self.profile.url.trim_end_matches('/')
    }

    fn authorization(&self) -> String {
        format!(
            "Basic {}",
            base64(format!("{}:{}", self.profile.username, self.profile.password).as_bytes())
        )
    }
}

/// The one row returned by `CheckScriptWithLinesAndColumns`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ScriptCheck {
    pub status: bool,
    pub line_number: usize,
    pub column_number: usize,
    pub message: String,
}

#[derive(Deserialize)]
struct ScriptCheckResponse {
    rows: Vec<ScriptCheck>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Delete,
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Delete => "DELETE",
        })
    }
}

#[derive(Debug)]
pub enum ServerError {
    InvalidUrl(String),
    Transport {
        method: Method,
        url: String,
        why: String,
    },
    Http {
        method: Method,
        status: u16,
        url: String,
        body: String,
    },
    /// A 2xx whose body is not the success the endpoint promises.
    Rejected {
        url: String,
        body: String,
    },
    UnsupportedCharset(String),
    InvalidUtf8 {
        at: usize,
    },
    InvalidResponse {
        url: String,
        why: String,
    },
}

impl ServerError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, ServerError::Http { status: 404, .. })
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerError::InvalidUrl(why) => write!(f, "invalid server URL: {why}"),
            ServerError::Transport { method, url, why } => {
                write!(f, "{method} {url} could not be reached: {why}")
            }
            ServerError::Http {
                method,
                status,
                url,
                body,
            } => {
                write!(f, "{method} {url} failed with HTTP {status}")?;
                if !body.is_empty() {
                    write!(f, ": {body}")?;
                }
                Ok(())
            }
            ServerError::Rejected { url, body } => {
                write!(
                    f,
                    "POST {url} answered success status but not success: {body:?}"
                )
            }
            ServerError::UnsupportedCharset(charset) => write!(
                f,
                "response charset {charset:?} is not supported; entity XML must be UTF-8"
            ),
            ServerError::InvalidUtf8 { at } => {
                write!(f, "response declared UTF-8 but is invalid at byte {at}")
            }
            ServerError::InvalidResponse { url, why } => {
                write!(f, "POST {url} returned an invalid response: {why}")
            }
        }
    }
}

impl std::error::Error for ServerError {}

struct Response {
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// Validate a response's encoding, and turn a non-2xx into an error carrying the server's own
/// explanation, which is where ThingWorx puts the useful part of a failure.
fn checked(
    secrets: &[String],
    method: Method,
    url: String,
    response: Response,
) -> Result<Vec<u8>, ServerError> {
    validate_charset(response.content_type.as_deref(), &response.body)?;
    if !(200..300).contains(&response.status) {
        let detail = std::str::from_utf8(&response.body).expect("validate_charset checked UTF-8");
        return Err(ServerError::Http {
            method,
            status: response.status,
            url,
            body: scrub(secrets, &excerpt(detail)),
        });
    }
    Ok(response.body)
}

/// `checked` for a file's bytes, which are not text whatever the response's charset says: the
/// server labels a repository PNG `UTF-8`. Only an error body is read as (lossy) text.
fn checked_bytes(
    secrets: &[String],
    method: Method,
    url: String,
    response: Response,
) -> Result<Vec<u8>, ServerError> {
    if !(200..300).contains(&response.status) {
        return Err(ServerError::Http {
            method,
            status: response.status,
            url,
            body: scrub(secrets, &excerpt(&String::from_utf8_lossy(&response.body))),
        });
    }
    Ok(response.body)
}

/// Every shape in which a server might echo this profile's credentials back in an error page: the
/// password, the app key and any secret-looking profile value as given, JSON-escaped and
/// percent-encoded, and `user:password` and the Basic token twaco sends. Longest first, so a long
/// form is replaced before a shorter one it contains.
fn secrets_of(profile: &Profile) -> Vec<String> {
    let mut raw: Vec<&str> = vec![profile.password.as_str()];
    raw.extend(profile.app_key.as_deref());
    for (key, value) in &profile.extra {
        let key = key.to_ascii_lowercase();
        if ["password", "secret", "key", "token"]
            .iter()
            .any(|word| key.contains(word))
        {
            raw.extend(value.as_str());
        }
    }
    let mut forms: Vec<String> = Vec::new();
    for secret in raw.into_iter().filter(|secret| !secret.is_empty()) {
        forms.push(secret.to_string());
        let json = serde_json::to_string(secret).expect("a string serialises");
        forms.push(json.trim_matches('"').to_string());
        forms.push(encode_path_segment(secret));
    }
    if !profile.password.is_empty() {
        let pair = format!("{}:{}", profile.username, profile.password);
        forms.push(base64(pair.as_bytes()));
        forms.push(pair);
    }
    forms.retain(|form| !form.is_empty());
    forms.sort();
    forms.dedup();
    forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
    forms
}

/// `text` with every secret replaced by `<redacted>`. A long secret is replaced wherever it
/// occurs; a short one only as a whole word, because replacing `pass` inside `bypass` would
/// mangle the message without protecting anything.
fn scrub(secrets: &[String], text: &str) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        if secret.len() >= 8 {
            out = out.replace(secret.as_str(), "<redacted>");
            continue;
        }
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let mut replaced = String::with_capacity(out.len());
        let mut last = 0;
        for (at, found) in out.match_indices(secret.as_str()) {
            let before = out[..at].chars().next_back();
            let after = out[at + found.len()..].chars().next();
            if before.is_some_and(word) || after.is_some_and(word) {
                continue;
            }
            replaced.push_str(&out[last..at]);
            replaced.push_str("<redacted>");
            last = at + found.len();
        }
        replaced.push_str(&out[last..]);
        out = replaced;
    }
    out
}

/// At most 4096 bytes of a server message, cut on a character boundary: slicing a multi-byte
/// character in half panics.
fn excerpt(text: &str) -> String {
    let text = text.trim();
    let mut cut = text.len().min(4096);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_string()
}

/// A multipart boundary that does not occur in the payload. Time and process id make it
/// unpredictable enough for a delimiter, and the loop makes a collision impossible rather than
/// unlikely: a boundary inside the payload would cut the uploaded file short.
fn boundary_for(payload: &[u8]) -> String {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    boundary_from(seed ^ u128::from(std::process::id()), payload)
}

fn boundary_from(seed: u128, payload: &[u8]) -> String {
    let mut attempt = 0u32;
    loop {
        let boundary = format!("twaco-{seed:x}-{attempt}");
        if !payload
            .windows(boundary.len())
            .any(|window| window == boundary.as_bytes())
        {
            return boundary;
        }
        attempt += 1;
    }
}

/// One `file` field, which is the only name the Importer accepts, carrying the bytes untouched.
fn multipart(boundary: &str, file_name: &str, payload: &[u8]) -> Vec<u8> {
    // A quote or line break in the filename would end the header early.
    let file_name: String = file_name
        .chars()
        .map(|c| {
            if c == '"' || c == '\r' || c == '\n' {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut body = Vec::with_capacity(payload.len() + 256);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: text/xml\r\n\r\n");
    body.extend_from_slice(payload);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// Percent-encode one complete URL path segment, retaining only RFC 3986 unreserved bytes.
pub fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(*byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

fn validate_charset(content_type: Option<&str>, body: &[u8]) -> Result<(), ServerError> {
    if let Some(content_type) = content_type {
        for parameter in content_type.split(';').skip(1) {
            let Some((name, value)) = parameter.split_once('=') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("charset") {
                let charset = value.trim().trim_matches(['"', '\'']);
                if !charset.eq_ignore_ascii_case("utf-8") && !charset.eq_ignore_ascii_case("utf8") {
                    return Err(ServerError::UnsupportedCharset(charset.to_string()));
                }
            }
        }
    }
    std::str::from_utf8(body)
        .map(|_| ())
        .map_err(|error| ServerError::InvalidUtf8 {
            at: error.valid_up_to(),
        })
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (chunk[0] as u32) << 16
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[((value >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((value >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn transport(
    agent: &ureq::Agent,
    method: Method,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<Response, ServerError> {
    transport_with_timeout(agent, method, url, headers, body, None)
}

fn transport_with_timeout(
    agent: &ureq::Agent,
    method: Method,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Option<Duration>,
) -> Result<Response, ServerError> {
    let unreachable = |error: ureq::Error| match error {
        ureq::Error::BadUri(why) => ServerError::InvalidUrl(why),
        error => ServerError::Transport {
            method,
            url: url.to_string(),
            why: error.to_string(),
        },
    };
    let mut response = match method {
        Method::Get => {
            let mut request = agent.get(url);
            if let Some(timeout) = timeout {
                request = request.config().timeout_global(Some(timeout)).build();
            }
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            request.call().map_err(unreachable)?
        }
        Method::Post => {
            let mut request = agent.post(url);
            if let Some(timeout) = timeout {
                request = request.config().timeout_global(Some(timeout)).build();
            }
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            request
                .send(body.unwrap_or_default())
                .map_err(unreachable)?
        }
        Method::Delete => {
            let mut request = agent.delete(url);
            if let Some(timeout) = timeout {
                request = request.config().timeout_global(Some(timeout)).build();
            }
            for (name, value) in headers {
                request = request.header(*name, *value);
            }
            request.call().map_err(unreachable)?
        }
    };
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .body_mut()
        .with_config()
        .limit(u64::MAX)
        .read_to_vec()
        .map_err(|error| ServerError::Transport {
            method,
            url: url.to_string(),
            why: error.to_string(),
        })?;
    Ok(Response {
        status,
        content_type,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_and_service_names_are_encoded_as_whole_segments() {
        assert_eq!(encode_path_segment("A/B service"), "A%2FB%20service");
        assert_eq!(encode_path_segment("Café.Thing"), "Caf%C3%A9.Thing");
        assert_eq!(encode_path_segment("name?x=1"), "name%3Fx%3D1");
    }

    #[test]
    fn response_charset_is_honoured_and_non_utf8_is_named() {
        validate_charset(Some("text/xml; charset=UTF-8"), "café".as_bytes()).unwrap();
        assert!(matches!(
            validate_charset(Some("text/xml; charset=ISO-8859-1"), b"plain"),
            Err(ServerError::UnsupportedCharset(name)) if name == "ISO-8859-1"
        ));
        assert!(matches!(
            validate_charset(Some("text/xml"), &[0xff]),
            Err(ServerError::InvalidUtf8 { .. })
        ));
    }

    #[test]
    fn basic_auth_encoding_is_correct() {
        assert_eq!(base64(b"user:password"), "dXNlcjpwYXNzd29yZA==");
    }

    #[test]
    fn get_sends_required_headers_and_surfaces_the_error_body() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            let headers = request.to_ascii_lowercase();
            assert!(request.starts_with("GET /Thingworx/Things/A%20B%20Thing HTTP/1.1"));
            assert!(headers.contains("accept: text/xml"));
            assert!(headers.contains("content-type: application/json"));
            assert!(headers.contains("x-xsrf-token: twx-xsrf-token-value"));
            assert!(headers.contains("x-requested-with: xmlhttprequest"));
            assert!(headers.contains("authorization: basic dxnlcjpwyxnz"));
            let body = "server says no";
            write!(
                stream,
                "HTTP/1.1 418 Teapot\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let client = Client::new(Profile {
            url: format!("http://{address}/Thingworx/"),
            username: "user".to_string(),
            password: "pass".to_string(),
            app_key: None,
            extra: Default::default(),
        });
        let error = client
            .fetch_entity(&EntityKey::new("Things", "A B Thing").unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 418"));
        assert!(error.contains("server says no"));
        server.join().unwrap();
    }

    #[test]
    fn a_long_error_body_is_cut_on_a_character_boundary() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer).unwrap();
            // 4095 ASCII bytes then a two-byte character straddling byte 4096.
            let body = format!("{}é tail", "a".repeat(4095));
            write!(
                stream,
                "HTTP/1.1 500 Error\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let client = Client::new(Profile {
            url: format!("http://{address}/Thingworx/"),
            username: "user".to_string(),
            password: "pass".to_string(),
            app_key: None,
            extra: Default::default(),
        });
        let error = client
            .fetch_entity(&EntityKey::new("Things", "T").unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 500"));
        assert!(!error.contains('é'));
        server.join().unwrap();
    }

    /// Accept one request, read it whole (headers and a Content-Length body), answer with
    /// `status` and `reply`, and hand the raw request back.
    fn serve_once(
        status: &'static str,
        reply: &'static str,
    ) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            let header_end = loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
                assert!(read > 0, "connection closed before the headers ended");
            };
            let head = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map(|value| value.trim().parse::<usize>().unwrap())
                .unwrap_or(0);
            while request.len() < header_end + length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "connection closed before the body ended");
                request.extend_from_slice(&buffer[..read]);
            }
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            )
            .unwrap();
            request
        });
        (format!("http://{address}/Thingworx/"), handle)
    }

    fn client_for(url: String) -> Client {
        Client::new(Profile {
            url,
            username: "user".to_string(),
            password: "pass".to_string(),
            app_key: None,
            extra: Default::default(),
        })
    }

    fn profile_with(password: &str, app_key: Option<&str>) -> Profile {
        Profile {
            url: "http://127.0.0.1:1/Thingworx".to_string(),
            username: "user".to_string(),
            password: password.to_string(),
            app_key: app_key.map(str::to_string),
            extra: Default::default(),
        }
    }

    /// Replacing `pass` inside `bypass` would mangle a message without protecting anything, so a
    /// short secret goes only where it stands alone; a long one goes wherever it is.
    #[test]
    fn a_short_secret_is_scrubbed_as_a_whole_word_and_a_long_one_anywhere() {
        let short = secrets_of(&profile_with("pass", None));
        assert_eq!(
            scrub(&short, "bypass the pass; pass=pass! passing"),
            "bypass the <redacted>; <redacted>=<redacted>! passing"
        );
        let long = secrets_of(&profile_with("correct-horse-battery", None));
        assert_eq!(
            scrub(&long, "x correct-horse-battery-staple y"),
            "x <redacted>-staple y"
        );
    }

    #[test]
    fn a_profile_without_secrets_scrubs_nothing() {
        let none = secrets_of(&profile_with("", None));
        assert!(none.is_empty(), "{none:?}");
        assert_eq!(scrub(&none, "user: and a message"), "user: and a message");
    }

    /// Only an error is scrubbed. A successful body is data (an entity export may well contain a
    /// string equal to the password) and rewriting it would corrupt what is returned.
    #[test]
    fn a_successful_response_is_returned_exactly() {
        let (url, server) =
            serve_once("200 OK", "<Entities>pass dXNlcjpwYXNz user:pass</Entities>");
        let client = Client::new(Profile {
            url,
            ..profile_with("pass", None)
        });
        let body = client
            .fetch_entity(&EntityKey::new("Things", "T").unwrap())
            .unwrap();
        server.join().unwrap();
        assert_eq!(
            body,
            b"<Entities>pass dXNlcjpwYXNz user:pass</Entities>".to_vec()
        );
    }

    /// A server, or a proxy in front of one, can reflect the request in its error page. Whatever
    /// it reflects, the credentials must not come out of twaco again: not as given, not escaped for
    /// JSON or a URL, not as the Basic token twaco sent.
    #[test]
    fn credentials_a_server_echoes_in_an_error_are_not_printed() {
        let password = "p\"a'ss\\w\u{f6}rd#1";
        let app_key = "ak-4f9c1d2e-7b3a";
        let token = base64(format!("user:{password}").as_bytes());
        let escaped = serde_json::to_string(password).unwrap();
        let encoded = encode_path_segment(password);
        let reply: &'static str = Box::leak(
            format!(
                "denied user=user password={password} json={escaped} url={encoded} \
                 appKey={app_key} Authorization: Basic {token}"
            )
            .into_boxed_str(),
        );
        let (url, server) = serve_once("401 Unauthorized", reply);
        let client = Client::new(Profile {
            url,
            username: "user".to_string(),
            password: password.to_string(),
            app_key: Some(app_key.to_string()),
            extra: Default::default(),
        });
        let error = client
            .fetch_entity(&EntityKey::new("Things", "T").unwrap())
            .unwrap_err();
        server.join().unwrap();
        let shown = format!("{error} | {error:?}");
        for (what, secret) in [
            ("the password", password.to_string()),
            (
                "the password, JSON-escaped",
                escaped.trim_matches('"').to_string(),
            ),
            ("the password, percent-encoded", encoded),
            ("the app key", app_key.to_string()),
            ("the Basic token", token),
        ] {
            assert!(!shown.contains(&secret), "{what} was printed: {shown}");
        }
        assert!(
            shown.contains("401") && shown.contains("denied"),
            "the rest of the answer survives: {shown}"
        );
    }

    #[test]
    fn import_posts_the_file_field_with_its_bytes_untouched() {
        let (url, server) = serve_once("200 OK", "success");
        // CRLF inside a CDATA must arrive as CRLF: the Importer stores a script verbatim.
        let payload = b"<Entities><Things><Thing name=\"T\"><code><![CDATA[a();\r\nb();]]></code></Thing></Things></Entities>";
        client_for(url).import_entity("T.xml", payload).unwrap();
        let request = server.join().unwrap();

        let text = String::from_utf8_lossy(&request).into_owned();
        let head_end = text.find("\r\n\r\n").unwrap();
        let (head, body) = (&text[..head_end], &request[head_end + 4..]);
        assert!(head.starts_with(&format!("POST /Thingworx/Importer?{IMPORT_QUERY} HTTP/1.1")));
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("authorization: basic dxnlcjpwyxnz"));
        assert!(lower.contains("x-xsrf-token: twx-xsrf-token-value"));
        let boundary = lower
            .lines()
            .find_map(|line| line.strip_prefix("content-type: multipart/form-data; boundary="))
            .expect("a multipart content type")
            .to_string();

        // Built by hand, not with `multipart`, so a framing bug cannot hide on both sides.
        let mut expected = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"T.xml\"\r\n\
             Content-Type: text/xml\r\n\r\n"
        )
        .into_bytes();
        expected.extend_from_slice(payload);
        expected.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        assert_eq!(
            body,
            expected.as_slice(),
            "the body is exactly one framed file field"
        );
        let body_text = String::from_utf8_lossy(body);
        assert!(body_text.contains("name=\"file\"; filename=\"T.xml\""));
        assert!(body.windows(payload.len()).any(|window| window == payload));
    }

    #[test]
    fn check_script_posts_the_typed_request_and_reads_the_one_row_response() {
        let reply =
            r#"{"rows":[{"status":false,"lineNumber":7,"columnNumber":11,"message":"bad token"}]}"#;
        let (url, server) = serve_once("200 OK", reply);
        let checked = client_for(url).check_script("var x = ;\n").unwrap();
        assert_eq!(
            checked,
            ScriptCheck {
                status: false,
                line_number: 7,
                column_number: 11,
                message: "bad token".to_string(),
            }
        );

        let request = server.join().unwrap();
        let split = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        let head = String::from_utf8_lossy(&request[..split]).to_ascii_lowercase();
        assert!(head.starts_with(
            "post /thingworx/resources/scriptservices/services/checkscriptwithlinesandcolumns http/1.1"
        ));
        assert!(head.contains("content-type: application/json"));
        assert!(head.contains("authorization: basic dxnlcjpwyxnz"));
        assert_eq!(&request[split..], br#"{"script":"var x = ;\n"}"#);
    }

    #[test]
    fn rest_delete_matches_composers_exact_request_shape() {
        let (url, server) = serve_once("200 OK", "");
        client_for(url)
            .delete_entity_rest(&EntityKey::new("DataShapes", "A B shape").unwrap())
            .unwrap();
        let request = server.join().unwrap();
        let end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap()
            + 4;
        let head = String::from_utf8_lossy(&request[..end]);
        assert!(head.starts_with(
            "DELETE /Thingworx/DataShapes/A%20B%20shape?Accept=application%2Fjson&Content-Type=application%2Fjson HTTP/1.1\r\n"
        ));
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("authorization: basic dxnlcjpwyxnz\r\n"));
        assert!(lower.contains("accept: application/json\r\n"));
        assert!(lower.contains("content-type: application/json\r\n"));
        assert!(lower.contains("x-requested-with: xmlhttprequest\r\n"));
        assert_eq!(&request[end..], b"");
    }

    #[test]
    fn a_2xx_that_does_not_say_success_is_a_failure() {
        let (url, server) = serve_once("200 OK", "Import failed: nothing imported");
        let error = client_for(url)
            .import_entity("T.xml", b"<Entities/>")
            .unwrap_err();
        server.join().unwrap();
        assert!(matches!(error, ServerError::Rejected { .. }), "{error}");
    }

    #[test]
    fn an_import_the_server_refuses_carries_its_status() {
        let (url, server) = serve_once("406 Not Acceptable", "Not Acceptable");
        let error = client_for(url)
            .import_entity("T.xml", b"<Entities/>")
            .unwrap_err();
        server.join().unwrap();
        assert!(
            matches!(
                error,
                ServerError::Http {
                    method: Method::Post,
                    status: 406,
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn the_multipart_body_matches_the_wire_format_byte_for_byte() {
        let golden: &[u8] = b"--B\r\n\
Content-Disposition: form-data; name=\"file\"; filename=\"T.xml\"\r\n\
Content-Type: text/xml\r\n\
\r\n\
<x>\r\n</x>\r\n\
--B--\r\n";
        assert_eq!(multipart("B", "T.xml", b"<x>\r\n</x>"), golden);
    }

    #[test]
    fn the_boundary_never_occurs_in_the_payload() {
        let first = boundary_from(7, b"");
        // A payload that contains the boundary that would otherwise be chosen.
        let hostile = format!("x{first}y");
        let chosen = boundary_from(7, hostile.as_bytes());
        assert_ne!(chosen, first);
        assert!(!hostile.contains(&chosen));
    }

    #[test]
    fn a_call_target_cannot_climb_out_of_its_collection() {
        // Refused before any request: the URL points nowhere a request could reach.
        for bad in [
            "",
            "Things/",
            "/X",
            "Things/../Users",
            "..",
            "Things/X/Y",
            "Things/.",
        ] {
            let error: ServerError = super::super::entity_key::ServiceTarget::parse(bad)
                .unwrap_err()
                .into();
            assert!(
                matches!(error, ServerError::InvalidUrl(_)),
                "{bad:?}: {error}"
            );
        }
    }

    #[test]
    fn service_calls_keep_their_existing_urls_for_typed_targets() {
        for (target, expected) in [
            (
                super::super::entity_key::ServiceTarget::parse("A Thing").unwrap(),
                "/Thingworx/Things/A%20Thing/Services/S%20name",
            ),
            (
                super::super::entity_key::ServiceTarget::entity("Widgets", "%#?é").unwrap(),
                "/Thingworx/Widgets/%25%23%3F%C3%A9/Services/S%20name",
            ),
            (
                super::super::entity_key::ServiceTarget::platform(
                    "Resources",
                    "SourceControlFunctions",
                ),
                "/Thingworx/Resources/SourceControlFunctions/Services/S%20name",
            ),
        ] {
            let (url, server) = serve_once("200 OK", "");
            client_for(url)
                .call_service(
                    &target,
                    "S name",
                    &serde_json::json!({}),
                    Duration::from_secs(1),
                )
                .unwrap();
            let request = server.join().unwrap();
            let line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap()
                .to_string();
            assert!(
                line.starts_with(&format!("POST {expected} HTTP/1.1")),
                "{line}"
            );
        }
    }

    #[test]
    fn a_filename_cannot_break_out_of_its_header() {
        let body = multipart("b", "evil\"\r\nX: y.xml", b"p");
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("filename=\"evil___X: y.xml\""));
        assert_eq!(text.matches("\r\n").count(), 6);
    }
}
