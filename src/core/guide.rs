//! The knowledge an agent needs besides the CLI and the XML.
//!
//! Two kinds of topic, read and searched alike. The built-in ones ship inside twaco: how to work
//! with it (`workflow`), the platform's verified-live quirks (`quirks`) and the service-code
//! reference (`service-code`). The solution's own are its markdown files, by default AGENTS.md,
//! CLAUDE.md and everything under `docs/`, which `[knowledge] paths` in twaco.toml replaces.
//! A topic is searched per `##` section, because a search should find the one section about
//! `AddMember`, not a document of sixty thousand characters.

use super::config::Solution;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const BUILTIN: [(&str, &str); 3] = [
    ("workflow", include_str!("../../knowledge/workflow.md")),
    ("quirks", include_str!("../../knowledge/quirks.md")),
    (
        "service-code",
        include_str!("../../knowledge/service-code.md"),
    ),
];
const DEFAULT_PATHS: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "docs"];
const HEADING_WEIGHT: f64 = 3.0;
/// Words a question is made of that say nothing about what it asks.
const STOPWORDS: [&str; 40] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "how",
    "i", "if", "in", "is", "it", "my", "of", "on", "or", "should", "so", "that", "the", "this",
    "to", "was", "what", "when", "where", "which", "why", "will", "with", "would", "you", "we",
];
/// A markdown file larger than this is not knowledge but data, and is left out.
const MAX_FILE: u64 = 2 * 1024 * 1024;
/// A topic read whole up to this size; a longer one gives its outline unless a section is named.
pub const WHOLE: usize = 24_000;

#[derive(Debug, thiserror::Error)]
pub enum GuideError {
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topic {
    /// `workflow`, or a solution file's path from the root without `.md`: `docs/DEVELOPER_GUIDE`.
    pub id: String,
    pub title: String,
    /// None for a built-in topic.
    pub file: Option<PathBuf>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Empty for the text before the first `##`.
    pub heading: String,
    pub text: String,
}

/// The built-in topics, then the solution's, and what could not be read.
pub fn topics(solution: Option<&Solution>) -> (Vec<Topic>, Vec<String>) {
    let mut topics: Vec<Topic> = BUILTIN
        .iter()
        .map(|(id, text)| Topic {
            id: id.to_string(),
            title: title_of(text, id),
            file: None,
            text: text.to_string(),
        })
        .collect();
    let mut problems = Vec::new();
    if let Some(solution) = solution {
        let configured = solution.knowledge.paths.as_ref();
        let paths: Vec<String> = match configured {
            Some(paths) => paths.clone(),
            None => DEFAULT_PATHS.iter().map(|p| p.to_string()).collect(),
        };
        let mut files = Vec::new();
        for path in &paths {
            let plain = Path::new(path).components().all(|c| {
                matches!(
                    c,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )
            });
            if !plain {
                problems.push(format!(
                    "[knowledge] {path}: must be a path inside the solution"
                ));
                continue;
            }
            let full = solution.root.join(path);
            // Every folder on the way must be a real folder too, not a link leading out.
            let mut through = solution.root.clone();
            let linked = Path::new(path)
                .parent()
                .into_iter()
                .flat_map(|p| p.components())
                .find_map(|part| {
                    through.push(part);
                    std::fs::symlink_metadata(&through)
                        .ok()
                        .filter(|m| m.file_type().is_symlink())
                        .map(|_| through.clone())
                });
            if let Some(link) = linked {
                problems.push(format!(
                    "[knowledge] {path}: {} is a link; not followed",
                    link.display()
                ));
                continue;
            }
            match std::fs::symlink_metadata(&full) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    problems.push(format!("{path}: a link; not followed"))
                }
                Ok(meta) if meta.is_dir() => markdown_under(&full, &mut files, &mut problems),
                Ok(_) => files.push(full),
                // The defaults are only where knowledge usually is; a path someone named must exist.
                Err(_) if configured.is_none() => {}
                Err(e) => problems.push(format!("[knowledge] {path}: {e}")),
            }
        }
        files.sort();
        files.dedup();
        for file in files {
            let id = file
                .strip_prefix(&solution.root)
                .unwrap_or(&file)
                .with_extension("")
                .to_string_lossy()
                .replace('\\', "/");
            match std::fs::metadata(&file) {
                Ok(meta) if meta.len() > MAX_FILE => {
                    problems.push(format!("{}: larger than 2 MB; left out", file.display()));
                    continue;
                }
                Err(e) => {
                    problems.push(format!("{}: {e}", file.display()));
                    continue;
                }
                Ok(_) => {}
            }
            match std::fs::read_to_string(&file) {
                Ok(text) => {
                    let id = if topics.iter().any(|t| t.id.eq_ignore_ascii_case(&id)) {
                        format!("project/{id}")
                    } else {
                        id
                    };
                    topics.push(Topic {
                        title: title_of(&text, &id),
                        id,
                        file: Some(file),
                        text,
                    })
                }
                Err(e) => problems.push(format!("{}: {e}", file.display())),
            }
        }
    }
    (topics, problems)
}

fn markdown_under(dir: &Path, out: &mut Vec<PathBuf>, problems: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            problems.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_symlink() => {}
            Ok(kind) if kind.is_dir() => markdown_under(&path, out, problems),
            Ok(_)
                if path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md")) =>
            {
                out.push(path)
            }
            _ => {}
        }
    }
}

/// The first `# ` heading, or the id.
fn title_of(text: &str, id: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(|t| t.trim().to_string())
        .unwrap_or_else(|| id.to_string())
}

/// A topic's `##` sections, the text before the first one included when it holds anything
/// but the title. A `##` inside a fenced code block is code, not a heading.
pub fn sections(text: &str) -> Vec<Section> {
    let mut out = vec![Section {
        heading: String::new(),
        text: String::new(),
    }];
    // The fence that opened the block: its character and length. Only a fence of the same
    // character, at least as long, closes it (CommonMark).
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start();
        let marker = ['`', '~'].into_iter().find_map(|c| {
            let run = trimmed.chars().take_while(|x| *x == c).count();
            (run >= 3).then_some((c, run))
        });
        let was_fenced = fence.is_some();
        match (fence, marker) {
            (None, Some(opening)) => fence = Some(opening),
            (Some((c, n)), Some((m, run)))
                if c == m && run >= n && trimmed[run * m.len_utf8()..].trim().is_empty() =>
            {
                fence = None
            }
            _ => {}
        }
        if !was_fenced && fence.is_none() {
            if let Some(heading) = line.strip_prefix("## ") {
                out.push(Section {
                    heading: heading.trim().to_string(),
                    text: String::new(),
                });
            }
        }
        let current = out.last_mut().expect("never empty");
        current.text.push_str(line);
        current.text.push('\n');
    }
    let preamble_matters = out[0]
        .text
        .lines()
        .any(|l| !l.trim().is_empty() && !l.starts_with("# "));
    if !preamble_matters {
        out.remove(0);
    }
    out
}

/// The topic an id means: exactly, any case, or by its last part when only one topic has it.
pub fn find<'a>(topics: &'a [Topic], wanted: &str) -> Result<&'a Topic, GuideError> {
    let wanted = wanted.trim_end_matches(".md");
    if let Some(topic) = topics.iter().find(|t| t.id.eq_ignore_ascii_case(wanted)) {
        return Ok(topic);
    }
    let by_name: Vec<&Topic> = topics
        .iter()
        .filter(|t| {
            t.id.rsplit('/')
                .next()
                .is_some_and(|last| last.eq_ignore_ascii_case(wanted))
        })
        .collect();
    match by_name.as_slice() {
        [one] => Ok(one),
        _ => Err(GuideError::Invalid(format!(
            "no topic {wanted:?}; there are: {}",
            topics
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// What reading a topic gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// The whole topic, or one section of it.
    Text(String),
    /// A topic too long to read whole: its title and section headings, to pick one from.
    Outline {
        title: String,
        headings: Vec<String>,
    },
}

/// A topic whole when it is short, its outline when it is long, or one section by its heading
/// (exactly, else the one heading that contains the text, any case).
pub fn read(topic: &Topic, section: Option<&str>) -> Result<Reading, GuideError> {
    let all = sections(&topic.text);
    let Some(wanted) = section else {
        if topic.text.len() <= WHOLE {
            return Ok(Reading::Text(topic.text.clone()));
        }
        let headings = all
            .iter()
            .filter(|s| !s.heading.is_empty())
            .map(|s| s.heading.clone())
            .collect();
        return Ok(Reading::Outline {
            title: topic.title.clone(),
            headings,
        });
    };
    let lower = wanted.to_lowercase();
    if let Some(found) = all.iter().find(|s| s.heading.to_lowercase() == lower) {
        return Ok(Reading::Text(found.text.clone()));
    }
    let close: Vec<&Section> = all
        .iter()
        .filter(|s| s.heading.to_lowercase().contains(&lower))
        .collect();
    match close.as_slice() {
        [one] => Ok(Reading::Text(one.text.clone())),
        [] => Err(GuideError::Invalid(format!(
            "{} has no section {wanted:?}",
            topic.id
        ))),
        many => Err(GuideError::Invalid(format!(
            "{wanted:?} matches {} sections of {}: {}",
            many.len(),
            topic.id,
            many.iter()
                .map(|s| s.heading.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        ))),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub topic: String,
    pub heading: String,
    /// How many of the query's words the section holds.
    pub matched: usize,
    pub of: usize,
    pub score: f64,
    /// The first line holding a query word, shortened.
    pub line: String,
}

/// Sections best first: those holding more of the words first, then by TF-IDF, a word in the
/// heading counting again. Long questions rarely share every word with the answer, so a
/// section holding only some of them still ranks, below the ones holding all.
pub fn search(topics: &[Topic], query: &str, limit: usize) -> Vec<Hit> {
    let asked: Vec<String> = super::help::words(query)
        .into_iter()
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .flat_map(|w| stems(&w))
        .collect();
    if asked.is_empty() {
        return Vec::new();
    }
    // Code joins what a question spells apart: "add member" is `AddMember`. A joined pair is
    // an alternative spelling, matched as one more word, never required.
    let joined: Vec<String> = asked
        .windows(2)
        .map(|pair| format!("{}{}", pair[0], pair[1]))
        .collect();
    let mut terms = asked.clone();
    terms.extend(joined.iter().cloned());
    terms.sort();
    terms.dedup();
    struct Indexed<'a> {
        topic: &'a str,
        section: Section,
        counts: HashMap<String, usize>,
        heading: Vec<String>,
    }
    let mut indexed = Vec::new();
    for topic in topics {
        for section in sections(&topic.text) {
            let mut counts = HashMap::new();
            for word in stems(&section.text) {
                *counts.entry(word).or_insert(0) += 1;
            }
            let heading = stems(&section.heading);
            indexed.push(Indexed {
                topic: &topic.id,
                section,
                counts,
                heading,
            });
        }
    }
    let total = indexed.len().max(1) as f64;
    let rarity: HashMap<&str, f64> = terms
        .iter()
        .map(|term| {
            let having = indexed
                .iter()
                .filter(|i| i.counts.contains_key(term))
                .count()
                .max(1) as f64;
            (term.as_str(), (total / having).ln().max(0.01))
        })
        .collect();
    let mut hits: Vec<Hit> = indexed
        .iter()
        .filter_map(|item| {
            // Coverage is of the words asked: a joined spelling covers the two words it joins.
            let mut covered = vec![false; asked.len()];
            let mut score = 0.0;
            for term in &terms {
                let count = item.counts.get(term).copied().unwrap_or(0);
                let in_heading = item.heading.iter().any(|w| w == term);
                if count == 0 && !in_heading {
                    continue;
                }
                for (at, word) in asked.iter().enumerate() {
                    if word == term {
                        covered[at] = true;
                    }
                }
                if let Some(at) = joined.iter().position(|j| j == term) {
                    covered[at] = true;
                    covered[at + 1] = true;
                }
                let rare = rarity[term.as_str()];
                if count > 0 {
                    score += rare * (1.0 + (count as f64).ln());
                }
                if in_heading {
                    score += HEADING_WEIGHT * rare;
                }
            }
            let matched = covered.iter().filter(|c| **c).count();
            if matched == 0 {
                return None;
            }
            let line = item
                .section
                .text
                .lines()
                .skip(1)
                .find(|line| stems(line).iter().any(|w| terms.contains(w)))
                .map(|line| shorten(line.trim(), 160))
                .unwrap_or_default();
            Some(Hit {
                topic: item.topic.to_string(),
                heading: item.section.heading.clone(),
                matched,
                of: asked.len(),
                score,
                line,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.matched
            .cmp(&a.matched)
            .then(
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then_with(|| a.topic.cmp(&b.topic))
            .then_with(|| a.heading.cmp(&b.heading))
    });
    // A solution may keep its own copy of a built-in topic: one section, once, the best.
    let mut seen = std::collections::HashSet::new();
    hits.retain(|hit| hit.heading.is_empty() || seen.insert(hit.heading.to_lowercase()));
    hits.truncate(limit);
    hits
}

/// A text's words, lower case, with the commonest English endings taken off, so that
/// "replaced" finds "replaces". Crude, and enough: both sides are stemmed alike.
fn stems(text: &str) -> Vec<String> {
    super::help::words(text)
        .into_iter()
        .map(|word| {
            for ending in ["ing", "ies", "es", "ed", "s"] {
                if let Some(stem) = word.strip_suffix(ending) {
                    if stem.chars().count() >= 4 && stem.chars().all(char::is_alphanumeric) {
                        return stem.to_string();
                    }
                }
            }
            word
        })
        .collect()
}

fn shorten(text: &str, most: usize) -> String {
    if text.chars().count() <= most {
        return text.to_string();
    }
    let cut: String = text.chars().take(most - 1).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_builtin_topics_are_there_and_split_into_sections() {
        let (topics, problems) = super::topics(None);
        assert!(problems.is_empty());
        assert_eq!(
            topics.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["workflow", "quirks", "service-code"]
        );
        let quirks = find(&topics, "quirks").unwrap();
        assert!(sections(&quirks.text).len() >= 50, "one section per quirk");
        for topic in &topics {
            let python_tool = [
                ".py`",
                "tools/twx",
                "tools/sync",
                "tools/import",
                "tools/bundle",
                "tools\\",
            ]
            .iter()
            .find(|t| topic.text.contains(*t));
            assert!(
                python_tool.is_none(),
                "{} still names the Python tools: {python_tool:?}",
                topic.id
            );
        }
    }

    #[test]
    fn a_heading_inside_code_is_not_a_section() {
        let text = "# T\n\n## One\nx\n```\n## not a heading\n```\n## Two\ny\n";
        let found = sections(text);
        assert_eq!(
            found.iter().map(|s| s.heading.as_str()).collect::<Vec<_>>(),
            ["One", "Two"]
        );
        assert!(found[0].text.contains("## not a heading"));
        // A tilde fence, and a longer fence that a shorter one inside does not close.
        let text = "## One\n~~~\n## code\n~~~\n````md\n```\n## still code\n```\n````\n## Two\n";
        let found = sections(text);
        assert_eq!(
            found.iter().map(|s| s.heading.as_str()).collect::<Vec<_>>(),
            ["One", "Two"]
        );
    }

    #[test]
    fn search_finds_the_section_about_it_first() {
        let (topics, _) = super::topics(None);
        let hits = search(&topics, "AddMember group", 5);
        assert_eq!(hits[0].topic, "quirks");
        assert!(hits[0].heading.contains("AddMember"), "{hits:?}");
        let hits = search(&topics, "how do I add a group member?", 3);
        assert!(
            hits[0].heading.contains("AddMember"),
            "joined words and no stopwords: {hits:?}"
        );
        let hits = search(&topics, "importer app key session", 3);
        assert!(hits[0].heading.contains("needs a session"), "{hits:?}");
        assert!(search(&topics, "zzzunknownword", 3).is_empty());
    }

    #[test]
    fn a_section_holding_more_of_the_words_ranks_first_whatever_its_score() {
        let topic = |id: &str, text: &str| Topic {
            id: id.into(),
            title: id.into(),
            file: None,
            text: text.into(),
        };
        // B repeats one rare joined word in its heading; A holds all three words once.
        let topics = [
            topic("a", "## A\nalpha beta gamma\n"),
            topic(
                "b",
                "## alphabeta alphabeta\nalphabeta alphabeta alphabeta\n",
            ),
            topic("c", "## C\nnothing\n"),
        ];
        let hits = search(&topics, "alpha beta gamma", 5);
        assert_eq!(
            hits.iter()
                .map(|h| (h.topic.as_str(), h.matched))
                .collect::<Vec<_>>(),
            [("a", 3), ("b", 2)]
        );
        assert!(search(&topics, "how do I", 5).is_empty(), "only stopwords");
        assert!(search(&topics, "ünïcödé — 日本", 5).is_empty());
    }

    #[test]
    fn a_long_topic_reads_as_its_outline_and_a_section_by_part_of_its_heading() {
        let (topics, _) = super::topics(None);
        let quirks = find(&topics, "QUIRKS.md").unwrap();
        match read(quirks, None).unwrap() {
            Reading::Outline { headings, .. } => assert!(headings.len() >= 50),
            other => panic!("{other:?}"),
        }
        let Reading::Text(text) = read(quirks, Some("addmember")).unwrap() else {
            panic!()
        };
        assert!(text.starts_with("## A Group's `AddMember`"));
        assert!(
            read(quirks, Some("import"))
                .unwrap_err()
                .to_string()
                .contains("matches"),
            "ambiguous"
        );
        assert!(read(quirks, Some("no such thing")).is_err());
        assert!(
            matches!(
                read(find(&topics, "workflow").unwrap(), None).unwrap(),
                Reading::Text(_)
            ),
            "short: whole"
        );
    }

    #[test]
    fn the_solutions_own_markdown_is_a_topic_too() {
        let root_guard = tempfile::Builder::new()
            .prefix("twaco-guide-")
            .tempdir()
            .unwrap();
        let root = root_guard.path().to_path_buf();
        std::fs::create_dir_all(root.join("docs/deep")).unwrap();
        std::fs::write(root.join("twaco.toml"), "[[project]]\nname = \"P\"\n").unwrap();
        std::fs::write(
            root.join("AGENTS.md"),
            "# Agents\n\n## Dashboards live in Postgres\nNot in DataTables.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("docs/deep/GUIDE.md"),
            "# Guide\n\n## Setup\nRun it.\n",
        )
        .unwrap();
        std::fs::write(root.join("docs/notes.txt"), "not markdown").unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let (topics, problems) = super::topics(Some(&solution));
        assert!(problems.is_empty(), "{problems:?}");
        let ids: Vec<&str> = topics.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(&ids[3..], ["AGENTS", "docs/deep/GUIDE"]);
        assert_eq!(
            find(&topics, "guide").unwrap().id,
            "docs/deep/GUIDE",
            "by its last part"
        );
        assert_eq!(search(&topics, "postgres dashboards", 1)[0].topic, "AGENTS");
        // A path someone names must exist, and stay inside the solution.
        std::fs::write(
            root.join("twaco.toml"),
            "[[project]]\nname = \"P\"\n\n[knowledge]\npaths = [\"missing.md\", \"../outside\"]\n",
        )
        .unwrap();
        let solution = Solution::load(&root.join("twaco.toml")).unwrap();
        let (topics, problems) = super::topics(Some(&solution));
        assert_eq!(topics.len(), 3);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }
}
