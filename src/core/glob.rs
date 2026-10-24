//! The one glob dialect of `twaco.toml`: `ignore_paths`, `server_owned`, `[unused] keep`,
//! `[call] read_only`, permission policies and the bundle's exclusions all match with it.

/// Whether `pattern` matches all of `text`: `*` matches within one path step, `**` across
/// steps, and every other character, brackets included, is literal. `fnmatch` would read
/// `ConfigurationTable[ConnectionInfo]` as a character class and silently match nothing.
pub(crate) fn matches(pattern: &str, text: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum Token {
        Char(char),
        Star,
        DoubleStar,
    }
    let mut tokens = Vec::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            tokens.push(Token::DoubleStar);
            i += 2;
        } else if chars[i] == '*' {
            tokens.push(Token::Star);
            i += 1;
        } else {
            tokens.push(Token::Char(chars[i]));
            i += 1;
        }
    }
    let text: Vec<char> = text.chars().collect();
    // matches[t][p]: text[t..] matches tokens[p..]. Filled from the end, so no recursion.
    let mut matches = vec![vec![false; tokens.len() + 1]; text.len() + 1];
    matches[text.len()][tokens.len()] = true;
    for t in (0..=text.len()).rev() {
        for p in (0..tokens.len()).rev() {
            matches[t][p] = match tokens[p] {
                Token::Char(c) => t < text.len() && text[t] == c && matches[t + 1][p + 1],
                Token::Star => {
                    matches[t][p + 1] || (t < text.len() && text[t] != '/' && matches[t + 1][p])
                }
                Token::DoubleStar => matches[t][p + 1] || (t < text.len() && matches[t + 1][p]),
            };
        }
    }
    matches[0][0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_keep_brackets_literal_and_star_within_a_step() {
        let pattern = "Things/*/ConfigurationTables/ConfigurationTable[ConnectionInfo]/**";
        assert!(matches(pattern, "Things/Db/ConfigurationTables/ConfigurationTable[ConnectionInfo]/Rows/Row/password#text"));
        assert!(!matches(
            pattern,
            "Things/Db/ConfigurationTables/ConfigurationTable[Other]/Rows"
        ));
        assert!(
            !matches(
                pattern,
                "Things/a/b/ConfigurationTables/ConfigurationTable[ConnectionInfo]/x"
            ),
            "* stays in one step"
        );
        assert!(matches(
            "Things/*/ThingProperties/**",
            "Things/T/ThingProperties/p/Value#text"
        ));
        assert!(
            !matches("Things/*", "Things/T/extra"),
            "anchored at the end"
        );
    }
}
