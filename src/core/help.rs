//! The ThingWorx Platform help center, searched and read the way its own pages do it.
//!
//! `support.ptc.com/help/thingworx/platform/<version>/en/` is a static site.
//! Its search runs in the browser over one file, `ThingWorx_sx.js`, which lists every page
//! and, for every word, the pages it appears on and how often. twaco downloads that file
//! once per version and searches it the same way. Pages are plain HTML whose content is the
//! `#page_content` element, read here as Markdown.
//!
//! The content is PTC's. It is fetched on demand, as a browser would, and cached in the
//! user's cache folder; nothing of it is shipped with twaco or written to a repository.

use serde_json::Value;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

pub const BASE: &str = "https://support.ptc.com/help/thingworx/platform";
/// The versions published with this layout, oldest first.
pub const VERSIONS: [&str; 5] = ["r9.6", "r9.7", "r10.0", "r10.1", "r10.2"];
pub const INDEX_FILE: &str = "ThingWorx_sx.js";
/// A word in a page's title counts this many times its rarity: in a help center the title is
/// the strongest sign of what a page is about. At 2, "DataShape" ranked CacheThings (the word
/// 30 times) above the page titled Data Shapes.
const TITLE_WEIGHT: f64 = 5.0;

#[derive(Debug)]
pub enum HelpError {
    Fetch { url: String, why: String },
    Cache { path: PathBuf, why: String },
    Index(String),
    Invalid(String),
}

impl fmt::Display for HelpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HelpError::Fetch { url, why } => write!(f, "{url}: {why}"),
            HelpError::Cache { path, why } => write!(f, "{}: {why}", path.display()),
            HelpError::Index(why) => {
                write!(f, "the help center's search index cannot be read: {why}")
            }
            HelpError::Invalid(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for HelpError {}

/// Fetching a file of the help center, as a trait so this module is tested offline.
pub trait Fetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// The real web, with a timeout. Redirects are followed, since the site moves pages, but only
/// within the base, and each is checked before it is requested.
pub struct Web {
    agent: ureq::Agent,
    base: String,
}

/// Hops followed before giving up; the site uses one or two.
const MOST_REDIRECTS: usize = 5;

impl Default for Web {
    fn default() -> Self {
        Self::new(BASE)
    }
}

impl Web {
    /// A web client which follows redirects only while they remain below `base`.
    pub fn new(base: &str) -> Self {
        // ureq never follows a redirect itself: it hands the 3xx back, so its target is
        // checked before any request is sent there.
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .max_redirects(0)
            .max_redirects_will_error(false)
            .build();
        Web {
            agent: config.into(),
            base: base.trim_end_matches('/').to_string(),
        }
    }

    fn within(&self, url: &str) -> bool {
        url.starts_with(&format!("{}/", self.base))
    }
}

impl Fetch for Web {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        if !self.within(url) {
            return Err(format!("{url} is outside {}", self.base));
        }
        let mut current = url.to_string();
        for _ in 0..=MOST_REDIRECTS {
            let mut response = self.agent.get(&current).call().map_err(|e| e.to_string())?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| format!("{current} redirected without a location"))?;
                let next = url::Url::parse(&current)
                    .and_then(|here| here.join(location))
                    .map_err(|e| format!("{current} redirected to {location:?}: {e}"))?
                    .to_string();
                if !self.within(&next) {
                    return Err(format!(
                        "redirected to {next}, which is outside {}",
                        self.base
                    ));
                }
                current = next;
                continue;
            }
            return response
                .body_mut()
                .with_config()
                .limit(32 * 1024 * 1024)
                .read_to_vec()
                .map_err(|e| e.to_string());
        }
        Err(format!("{url} redirected more than {MOST_REDIRECTS} times"))
    }
}

/// A version as the site spells it: `10.1`, `r10.1` and `10.1.0-b47` are all `r10.1`.
pub fn version(text: &str) -> Result<String, HelpError> {
    let text = text.trim().trim_start_matches(['r', 'R']);
    let mut parts = text.split(['.', '-']);
    let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
        return Err(HelpError::Invalid(format!(
            "a help version looks like 10.1, not {text:?}"
        )));
    };
    if major.parse::<u32>().is_err() || minor.parse::<u32>().is_err() {
        return Err(HelpError::Invalid(format!(
            "a help version looks like 10.1, not {text:?}"
        )));
    }
    let wanted = format!("r{major}.{minor}");
    if VERSIONS.contains(&wanted.as_str()) {
        Ok(wanted)
    } else {
        Err(HelpError::Invalid(format!(
            "there is no ThingWorx Platform help for {wanted}; there is for {}",
            VERSIONS.join(", ")
        )))
    }
}

pub fn newest() -> &'static str {
    VERSIONS[VERSIONS.len() - 1]
}

/// Where the help is cached: the user's cache folder, shared by every project.
pub fn cache_root() -> Result<PathBuf, HelpError> {
    dirs::cache_dir()
        .map(|dir| dir.join("twaco").join("help"))
        .ok_or_else(|| {
            HelpError::Invalid(
                "this machine has no user cache folder to keep the help in".to_string(),
            )
        })
}

/// A file of one version of the help, from the cache, or fetched into it.
pub fn cached(
    fetch: &dyn Fetch,
    cache: &std::path::Path,
    version: &str,
    path: &str,
    refresh: bool,
) -> Result<Vec<u8>, HelpError> {
    let local = cache
        .join(version)
        .join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
    let url = format!("{BASE}/{version}/en/{path}");
    cached_file(fetch, &local, &url, refresh)
}

/// One web file at an explicitly chosen cache path. This is shared by the help center and
/// other public PTC documentation with the same fetch-once semantics.
pub fn cached_file(
    fetch: &dyn Fetch,
    local: &std::path::Path,
    url: &str,
    refresh: bool,
) -> Result<Vec<u8>, HelpError> {
    if !refresh {
        if let Ok(bytes) = std::fs::read(local) {
            return Ok(bytes);
        }
    }
    let bytes = fetch.get(url).map_err(|why| HelpError::Fetch {
        url: url.to_string(),
        why,
    })?;
    let io = |e: std::io::Error| HelpError::Cache {
        path: local.to_path_buf(),
        why: e.to_string(),
    };
    std::fs::create_dir_all(local.parent().expect("a cached file has a folder")).map_err(io)?;
    // Through a temporary, so an interrupted download never leaves half a file behind.
    let temporary = local.with_extension(format!(
        "{}.{}.part",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&temporary, &bytes).map_err(io)?;
    std::fs::rename(&temporary, local).map_err(io)?;
    Ok(bytes)
}

/// The help version to read: one asked for, then the one a page address names, then
/// `[help] version`, then the server's own, then the newest. `notes` says which was chosen
/// when it was not the obvious one, and why.
pub fn choose_version(
    asked: Option<&str>,
    named: Option<String>,
    solution: Option<&super::config::Solution>,
    profile_name: &str,
    notes: &mut Vec<String>,
) -> Result<String, String> {
    if let Some(text) = asked {
        return version(text).map_err(|e| e.to_string());
    }
    if let Some(version) = named {
        return Ok(version);
    }
    let Some(solution) = solution else {
        notes.push(format!(
            "no solution here, so no server to ask its version; reading the newest help, {}",
            newest()
        ));
        return Ok(newest().to_string());
    };
    if let Some(text) = &solution.help.version {
        return version(text).map_err(|e| format!("[help] version: {e}"));
    }
    let detected = super::profile::load(&solution.root, profile_name)
        .map_err(|e| e.to_string())
        .and_then(|profile| server_version(&super::server::Client::new(profile)));
    match detected.and_then(|text| version(&text).map_err(|e| e.to_string())) {
        Ok(version) => Ok(version),
        Err(why) => {
            notes.push(format!(
                "the server's version is unknown ({why}); reading the newest help, {}",
                newest()
            ));
            Ok(newest().to_string())
        }
    }
}

/// The server's ThingWorx version, such as `10.1.0-b47`. Read-only.
pub fn server_version(client: &super::server::Client) -> Result<String, String> {
    let reply = client
        .call_service(
            &super::entity_key::ServiceTarget::platform("Subsystems", "PlatformSubsystem"),
            "GetPlatformStats",
            &serde_json::json!({}),
            Duration::from_secs(20),
        )
        .map_err(|e| e.to_string())?;
    reply
        .as_ref()
        .and_then(|value| value.pointer("/rows/0/thingworxSoftwareVersion"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "GetPlatformStats did not say".to_string())
}

// ---- search ---------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub path: String,
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Default)]
pub struct Index {
    pub pages: Vec<Page>,
    /// For each word, the pages it is on and how often.
    words: HashMap<String, Vec<(usize, u64)>>,
}

impl Index {
    /// `ThingWorx_sx.js`: `var info = { "pages": [[path, title, summary, ...]], "words": {word:
    /// [page, count, page, count, ...]} }`.
    pub fn parse(text: &str) -> Result<Index, HelpError> {
        let start = text
            .find('{')
            .ok_or_else(|| HelpError::Index("no object".to_string()))?;
        let end = text
            .rfind('}')
            .ok_or_else(|| HelpError::Index("no object".to_string()))?;
        let info: Value = serde_json::from_str(&text[start..=end])
            .map_err(|e| HelpError::Index(e.to_string()))?;
        let pages = info["pages"]
            .as_array()
            .ok_or_else(|| HelpError::Index("no pages".to_string()))?
            .iter()
            .map(|page| {
                let field = |i: usize| {
                    page.get(i)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                Page {
                    path: field(0),
                    title: field(1),
                    summary: field(2),
                }
            })
            .collect();
        let words = info["words"]
            .as_object()
            .ok_or_else(|| HelpError::Index("no words".to_string()))?
            .iter()
            .map(|(word, postings)| {
                let numbers: Vec<u64> = postings
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .collect();
                let postings = numbers
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|[page, count]| (*page as usize, *count))
                    .collect();
                (word.clone(), postings)
            })
            .collect();
        Ok(Index { pages, words })
    }
}

/// A query's words as the index spells them: lower case, split on what the index splits on.
/// The index keeps `-`, `.`, `_`, `:` and `'` inside a word (`server-side`, `me.name`), so
/// those stay, trimmed from the ends; brackets, quotes and commas separate words.
pub fn words(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| {
            !(c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '\'' | '@' | '$' | '&'))
        })
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub page: Page,
    pub url: String,
    /// How well the page matches, in thousandths: rarer words, repeated or in the title, score more.
    pub score: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub hits: Vec<Hit>,
    /// Pages that contain every word, of which `hits` are the best.
    pub matched: usize,
    /// Words the help never uses, which is why nothing matched when something did not.
    pub unknown: Vec<String>,
}

/// Pages holding every word, best first. A word with a dot the index does not know, such as
/// `Resources.InfoTableFunctions`, is looked for as its parts instead.
pub fn search(index: &Index, version: &str, query: &str, limit: usize) -> Found {
    let mut terms: Vec<String> = Vec::new();
    for word in words(query) {
        if index.words.contains_key(&word) || !word.contains('.') {
            terms.push(word);
        } else {
            terms.extend(
                word.split('.')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            );
        }
    }
    terms.sort();
    terms.dedup();
    let unknown: Vec<String> = terms
        .iter()
        .filter(|t| !index.words.contains_key(*t))
        .cloned()
        .collect();
    if terms.is_empty() || !unknown.is_empty() {
        return Found {
            hits: Vec::new(),
            matched: 0,
            unknown,
        };
    }
    // TF-IDF: a word counts for more the fewer pages use it, and repeats on one page count
    // for less each time, so a long page that mentions a common word often does not outrank
    // the page about it. A word in the title counts again: the page is likely about it.
    let pages = index.pages.len().max(1) as f64;
    let mut scores: HashMap<usize, (usize, f64)> = HashMap::new();
    // Code says DataShape where titles say Data Shapes, and such a page may never use the joined
    // word, so a title matches with its spaces taken out, and a title alone is enough to match.
    let in_title = |page: &Page, term: &str| {
        let title = words(&page.title);
        title.iter().any(|w| w == term)
            || title.windows(2).any(|pair| {
                let joined = format!("{}{}", pair[0], pair[1]);
                joined == term || joined.strip_suffix('s') == Some(term)
            })
    };
    for term in &terms {
        let postings = &index.words[term];
        let rarity = (pages / postings.len().max(1) as f64).ln().max(0.01);
        let mut counted = std::collections::HashSet::new();
        for (page, count) in postings {
            let titled = index.pages.get(*page).is_some_and(|p| in_title(p, term));
            let entry = scores.entry(*page).or_default();
            entry.0 += 1;
            entry.1 += rarity * (1.0 + (*count as f64).ln())
                + if titled { TITLE_WEIGHT * rarity } else { 0.0 };
            counted.insert(*page);
        }
        for (page, info) in index.pages.iter().enumerate() {
            if !counted.contains(&page) && in_title(info, term) {
                let entry = scores.entry(page).or_default();
                entry.0 += 1;
                entry.1 += TITLE_WEIGHT * rarity;
            }
        }
    }
    let mut matching: Vec<(usize, u64)> = scores
        .into_iter()
        .filter(|(page, (seen, _))| *seen == terms.len() && *page < index.pages.len())
        .map(|(page, (_, score))| (page, (score * 1000.0).round() as u64))
        .collect();
    matching.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| index.pages[a.0].title.cmp(&index.pages[b.0].title))
    });
    let matched = matching.len();
    let hits = matching
        .into_iter()
        .take(limit)
        .map(|(page, score)| {
            let page = index.pages[page].clone();
            let url = page_url(version, &page.path);
            Hit { page, url, score }
        })
        .collect();
    Found {
        hits,
        matched,
        unknown,
    }
}

/// The address a person opens: the site's own frame, at that page.
pub fn page_url(version: &str, path: &str) -> String {
    format!("{BASE}/{version}/en/#page/{path}")
}

// ---- a page ---------------------------------------------------------------------------------

/// A page named any way a person or a search result might: its path (`ThingWorx/Help/X.html`),
/// or a help-center address with or without `#page/`. Returns the version named in the
/// address, if any, and the path. Anything outside the help center is refused.
pub fn page_path(text: &str) -> Result<(Option<String>, String), HelpError> {
    let text = text.trim();
    let (version, path) = match text.strip_prefix(BASE) {
        Some(rest) => {
            let rest = rest.trim_start_matches('/');
            let (version, rest) = rest.split_once('/').ok_or_else(|| bad_page(text))?;
            let rest = rest.strip_prefix("en/").ok_or_else(|| bad_page(text))?;
            let rest = rest.strip_prefix("#page/").unwrap_or(rest);
            (Some(self::version(version)?), rest.to_string())
        }
        None if text.contains("://") => return Err(bad_page(text)),
        None => (
            None,
            text.trim_start_matches('/')
                .strip_prefix("#page/")
                .unwrap_or(text)
                .to_string(),
        ),
    };
    let path = path
        .split(['#', '?'])
        .next()
        .unwrap_or_default()
        .to_string();
    // Plain `/`-separated segments only. A backslash, a drive or UNC prefix, a colon or a
    // percent-escape could climb out once the path becomes a cache file or a URL on Windows.
    let plain = |part: &str| {
        !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':', '%'])
    };
    if path.is_empty() || !path.ends_with(".html") || !path.split('/').all(plain) {
        return Err(bad_page(text));
    }
    Ok((version, path))
}

fn bad_page(text: &str) -> HelpError {
    HelpError::Invalid(format!(
        "{text:?} is not a page of the ThingWorx Platform help: give a path such as \
         ThingWorx/Welcome.html, or an address under {BASE}/"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub title: String,
    pub url: String,
    /// Every heading on the page, in order, so a reader can ask for one section.
    pub headings: Vec<String>,
    pub markdown: String,
}

/// A page's content as Markdown, its links made absolute. With `section`, only the part under
/// the first heading containing that text, down to the next heading of the same or a higher
/// level.
pub fn read(
    html: &[u8],
    version: &str,
    path: &str,
    section: Option<&str>,
) -> Result<Read, HelpError> {
    let document = scraper::Html::parse_document(&String::from_utf8_lossy(html));
    let content = scraper::Selector::parse("#page_content").expect("a valid selector");
    let element = document.select(&content).next().ok_or_else(|| {
        HelpError::Invalid(format!(
            "{path} has no page content; it may not be a help page"
        ))
    })?;
    let inner = headings_as_tags(element);
    let markdown = htmd::convert(&inner).map_err(|e| HelpError::Invalid(format!("{path}: {e}")))?;
    let source = format!("{BASE}/{version}/en/{path}");
    let markdown = absolute_links(&markdown, &source);
    let headings: Vec<String> = markdown
        .lines()
        .filter(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .collect();
    let title = headings
        .first()
        .cloned()
        .unwrap_or_else(|| path.to_string());
    let markdown = match section {
        None => markdown,
        Some(wanted) => self::section(&markdown, wanted).ok_or_else(|| {
            HelpError::Invalid(format!(
                "{path} has no section {wanted:?}; its headings are: {}",
                headings.join("; ")
            ))
        })?,
    };
    Ok(Read {
        title,
        url: page_url(version, path),
        headings,
        markdown,
    })
}

/// The level a help-page heading class stands for. The site marks headings with classes on
/// `div`s, not with `h1`–`h6` (sampled across 25 pages: Section_Title, Heading_2 to Heading_7,
/// PubsSection_Title, Related_Topics_Title, and Title for the page's own).
fn heading_level(class: &str) -> Option<usize> {
    match class {
        "Title" => Some(1),
        "Section_Title" | "PubsSection_Title" | "Related_Topics_Title" => Some(2),
        _ => class
            .strip_prefix("Heading_")
            .and_then(|n| n.parse::<usize>().ok())
            .map(|n| n.clamp(2, 6)),
    }
}

/// The content's HTML with each heading `div` written as the `hN` it stands for, so the
/// Markdown has headings a reader can ask for by name.
fn headings_as_tags(content: scraper::ElementRef) -> String {
    let mut html = content.inner_html();
    let any = scraper::Selector::parse("[class]").expect("a valid selector");
    for element in content.select(&any) {
        let Some(level) = element
            .value()
            .attr("class")
            .and_then(|c| c.split_whitespace().find_map(heading_level))
        else {
            continue;
        };
        let text: String = element
            .text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            continue;
        }
        html = html.replacen(
            &element.html(),
            &format!("<h{level}>{}</h{level}>", escape(&text)),
            1,
        );
    }
    // Code is a `pre` whose lines are `<br />`s, which Markdown would turn into soft line
    // breaks in a paragraph; written as `pre > code` with real newlines it stays a code block.
    // Inline code is a `codeph` span.
    let pre = scraper::Selector::parse("pre").expect("a valid selector");
    for element in content.select(&pre) {
        let mut code = String::new();
        for node in element.descendants() {
            match node.value() {
                scraper::Node::Text(text) => code.push_str(text),
                scraper::Node::Element(e) if e.name() == "br" => code.push('\n'),
                _ => {}
            }
        }
        html = html.replacen(
            &element.html(),
            &format!("<pre><code>{}</code></pre>", escape(&code)),
            1,
        );
    }
    let inline = scraper::Selector::parse("span.codeph").expect("a valid selector");
    for element in content.select(&inline) {
        let text: String = element.text().collect();
        html = html.replacen(
            &element.html(),
            &format!("<code>{}</code>", escape(&text)),
            1,
        );
    }
    html
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn section(markdown: &str, wanted: &str) -> Option<String> {
    let wanted = wanted.to_lowercase();
    let level = |line: &str| line.chars().take_while(|c| *c == '#').count();
    let lines: Vec<&str> = markdown.lines().collect();
    let start = lines
        .iter()
        .position(|line| level(line) > 0 && line.to_lowercase().contains(&wanted))?;
    let depth = level(lines[start]);
    let end = lines[start + 1..]
        .iter()
        .position(|line| (1..=depth).contains(&level(line)))
        .map_or(lines.len(), |offset| start + 1 + offset);
    Some(lines[start..end].join("\n"))
}

/// Every `](target)` in Markdown, resolved against the page's own address, so a link can be
/// followed from where the text ends up.
pub(crate) fn absolute_links(markdown: &str, source: &str) -> String {
    let Ok(base) = url::Url::parse(source) else {
        return markdown.to_string();
    };
    let mut out = String::with_capacity(markdown.len());
    let mut rest = markdown;
    while let Some(at) = rest.find("](") {
        out.push_str(&rest[..at + 2]);
        rest = &rest[at + 2..];
        let Some(close) = rest.find(')') else { break };
        let target = &rest[..close];
        let (link, title) = match target.split_once(' ') {
            Some((link, title)) => (link, Some(title)),
            None => (target, None),
        };
        match base.join(link) {
            Ok(resolved) => out.push_str(resolved.as_str()),
            Err(_) => out.push_str(link),
        }
        if let Some(title) = title {
            out.push(' ');
            out.push_str(title);
        }
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A server answering each request with the next of `answers`, then closing.
    fn serve(answers: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap();
                asked.push(
                    String::from_utf8_lossy(&buffer[..read])
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                );
                stream.write_all(answer.as_bytes()).unwrap();
            }
            asked
        });
        (address, handle)
    }

    #[test]
    fn a_redirect_is_followed_inside_the_base_and_refused_before_leaving_it() {
        let (address, server) = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /docs/moved.html\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".into(),
        ]);
        let web = Web::new(&format!("{address}/docs"));
        assert_eq!(
            web.get(&format!("{address}/docs/old.html")).unwrap(),
            b"hello"
        );
        assert_eq!(
            server.join().unwrap(),
            [
                "GET /docs/old.html HTTP/1.1",
                "GET /docs/moved.html HTTP/1.1"
            ]
        );

        // Port 1 is closed: had twaco followed the redirect, the error would be a refused
        // connection, not this.
        let (address, server) = serve(vec![
            "HTTP/1.1 301 Moved\r\nLocation: http://127.0.0.1:1/docs/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        ]);
        let web = Web::new(&format!("{address}/docs"));
        let error = web.get(&format!("{address}/docs/a.html")).unwrap_err();
        assert!(error.contains("outside"), "{error}");
        server.join().unwrap();
        assert!(
            web.get("http://127.0.0.1:1/elsewhere")
                .unwrap_err()
                .contains("outside"),
            "never asked"
        );
    }

    const INDEX: &str = r#"var info =
{
"pages":
[
["ThingWorx/Welcome.html","Welcome","Start here.","p1.js","content-page"]
,["ThingWorx/Help/InfoTables.html","InfoTable Functions","Sorting and filtering.","p2.js","content-page"]
,["ThingWorx/Help/Logging.html","Logging","Use the logger.","p3.js","content-page"]
],
"words":
{"infotablefunctions": [1, 5, 2, 1], "sort": [1, 3], "logger": [2, 4, 0, 1], "me.name": [2, 1], "server-side": [0, 2]},
"synonyms": {}
};"#;

    fn index() -> Index {
        Index::parse(INDEX).unwrap()
    }

    #[test]
    fn the_index_is_read_as_the_site_writes_it() {
        let index = index();
        assert_eq!(index.pages.len(), 3);
        assert_eq!(index.pages[1].path, "ThingWorx/Help/InfoTables.html");
        assert_eq!(index.words["logger"], vec![(2, 4), (0, 1)]);
    }

    #[test]
    fn a_query_is_split_as_the_index_is() {
        assert_eq!(
            words("Resources[\"InfoTableFunctions\"].Sort"),
            ["resources", "infotablefunctions", "sort"]
        );
        assert_eq!(
            words("server-side me.name, (logger)"),
            ["server-side", "me.name", "logger"]
        );
    }

    #[test]
    fn every_word_must_appear_and_the_best_page_comes_first() {
        let found = search(&index(), "r10.1", "InfoTableFunctions sort", 10);
        assert_eq!(found.matched, 1);
        assert_eq!(found.hits[0].page.title, "InfoTable Functions");
        assert!(found.hits[0].score > 0);
        assert_eq!(found.hits[0].url, "https://support.ptc.com/help/thingworx/platform/r10.1/en/#page/ThingWorx/Help/InfoTables.html");

        let found = search(&index(), "r10.1", "logger", 10);
        assert_eq!(
            found
                .hits
                .iter()
                .map(|h| h.page.title.as_str())
                .collect::<Vec<_>>(),
            ["Logging", "Welcome"]
        );
        assert_eq!(
            search(&index(), "r10.1", "logger", 1).matched,
            2,
            "matched counts beyond the limit"
        );
    }

    #[test]
    fn a_page_titled_for_the_word_outranks_one_that_merely_repeats_it() {
        // "Data Shapes" never uses the joined word; another page uses it thirty times.
        let index = Index::parse(
            r#"var info = { "pages": [["a.html","CacheThings","",""],["b.html","Data Shapes","",""],["c.html","Other","",""]],
            "words": { "datashape": [0, 30, 2, 1] } };"#,
        )
        .unwrap();
        let titles: Vec<String> = search(&index, "r10.1", "DataShape", 3)
            .hits
            .into_iter()
            .map(|h| h.page.title)
            .collect();
        assert_eq!(titles, ["Data Shapes", "CacheThings", "Other"]);
    }

    #[test]
    fn a_short_word_does_not_match_inside_a_title() {
        // "at" is in "Data Shapes" only as letters; it must not make that page match.
        let index = Index::parse(
            r#"var info = { "pages": [["a.html","Data Shapes","",""],["b.html","Look at this","",""]],
            "words": { "at": [1, 1] } };"#,
        )
        .unwrap();
        let titles: Vec<String> = search(&index, "r10.1", "at", 5)
            .hits
            .into_iter()
            .map(|h| h.page.title)
            .collect();
        assert_eq!(titles, ["Look at this"]);
    }

    #[test]
    fn an_unknown_word_is_named_rather_than_silently_matching_nothing() {
        let found = search(&index(), "r10.1", "logger QueryLogEntries", 10);
        assert!(found.hits.is_empty());
        assert_eq!(found.unknown, ["querylogentries"]);
        // A dotted word the index lacks is looked for as its parts.
        assert_eq!(
            search(&index(), "r10.1", "InfoTableFunctions.Sort", 10).matched,
            1
        );
        assert_eq!(search(&index(), "r10.1", "me.name", 10).matched, 1);
    }

    #[test]
    fn versions_are_spelt_as_the_site_spells_them() {
        assert_eq!(version("10.1").unwrap(), "r10.1");
        assert_eq!(version("r10.2").unwrap(), "r10.2");
        assert_eq!(version("10.1.0-b47").unwrap(), "r10.1");
        assert!(version("8.5").is_err());
        assert!(version("ten").is_err());
    }

    #[test]
    fn only_help_center_pages_are_read() {
        assert_eq!(
            page_path("ThingWorx/Welcome.html").unwrap(),
            (None, "ThingWorx/Welcome.html".to_string())
        );
        assert_eq!(
            page_path("https://support.ptc.com/help/thingworx/platform/r10.2/en/#page/ThingWorx/Help/X.html").unwrap(),
            (Some("r10.2".to_string()), "ThingWorx/Help/X.html".to_string())
        );
        assert_eq!(
            page_path("https://support.ptc.com/help/thingworx/platform/r10.1/en/ThingWorx/Help/X.html#anchor").unwrap().1,
            "ThingWorx/Help/X.html"
        );
        for bad in [
            "https://example.com/x.html",
            "ThingWorx/../../etc/passwd.html",
            "ThingWorx/x.txt",
            "",
            "C:\\outside.html",
            "..\\..\\outside.html",
            "\\\\server\\share\\x.html",
            "ThingWorx/%2e%2e/x.html",
            "ThingWorx/./x.html",
        ] {
            assert!(page_path(bad).is_err(), "{bad:?}");
        }
    }

    const PAGE: &str = r#"<html><body><div id="ww_skin_page_toolbar">toolbar</div>
<div id="page_content">
<div class="Title">Logging</div>
<p class="Body">Use the <a href="../Other/Logger.html">logger</a> wisely.</p>
<div id="wwID0E" class="Section_Title">Levels</div>
<p class="Body">WARN is the default.</p>
<ul><li>DEBUG</li><li>INFO</li></ul>
<p class="Body">Call <span class="codeph codephbreak">logger.warn</span> like this:</p>
<pre class="Preformatted">var a = 1;<br />logger.warn("a &lt; b");</pre>
<div class="Heading_2">Script <span>Log</span></div>
<p class="Body">Scripts write here.</p>
</div></body></html>"#;

    #[test]
    fn a_page_is_its_content_as_markdown_with_links_made_absolute() {
        let read = read(
            PAGE.as_bytes(),
            "r10.1",
            "ThingWorx/Help/Logging.html",
            None,
        )
        .unwrap();
        assert_eq!(read.title, "Logging");
        assert_eq!(read.headings, ["Logging", "Levels", "Script Log"]);
        assert!(
            !read.markdown.contains("toolbar"),
            "only the page content: {}",
            read.markdown
        );
        assert!(
            read.markdown.contains("(https://support.ptc.com/help/thingworx/platform/r10.1/en/ThingWorx/Other/Logger.html)"),
            "{}",
            read.markdown
        );
        assert!(read.markdown.contains("DEBUG"));
        assert!(
            read.markdown.contains("`logger.warn`"),
            "inline code: {}",
            read.markdown
        );
        assert!(
            read.markdown
                .contains("```\nvar a = 1;\nlogger.warn(\"a < b\");\n```"),
            "a code block: {}",
            read.markdown
        );
    }

    #[test]
    fn a_section_runs_to_the_next_heading_of_its_level() {
        let read = read(
            PAGE.as_bytes(),
            "r10.1",
            "ThingWorx/Help/Logging.html",
            Some("levels"),
        )
        .unwrap();
        assert!(read.markdown.starts_with("## Levels"), "{}", read.markdown);
        assert!(
            read.markdown.contains("INFO") && !read.markdown.contains("Scripts write here"),
            "{}",
            read.markdown
        );
        let error =
            super::read(PAGE.as_bytes(), "r10.1", "x.html", Some("nothing like it")).unwrap_err();
        assert!(error.to_string().contains("Levels; Script Log"), "{error}");
    }

    struct Counting {
        calls: RefCell<Vec<String>>,
    }

    impl Fetch for Counting {
        fn get(&self, url: &str) -> Result<Vec<u8>, String> {
            self.calls.borrow_mut().push(url.to_string());
            Ok(b"fetched".to_vec())
        }
    }

    #[test]
    fn a_file_is_fetched_once_and_then_read_from_the_cache() {
        let cache = std::env::temp_dir().join(format!(
            "twaco-help-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let web = Counting {
            calls: RefCell::new(Vec::new()),
        };
        for _ in 0..2 {
            assert_eq!(
                cached(&web, &cache, "r10.1", "ThingWorx/Welcome.html", false).unwrap(),
                b"fetched"
            );
        }
        assert_eq!(
            *web.calls.borrow(),
            ["https://support.ptc.com/help/thingworx/platform/r10.1/en/ThingWorx/Welcome.html"]
        );
        cached(&web, &cache, "r10.1", "ThingWorx/Welcome.html", true).unwrap();
        assert_eq!(web.calls.borrow().len(), 2, "refresh fetches again");
        let _ = std::fs::remove_dir_all(cache);
    }
}
