//! The editable text: a YAML mapping from each workspace to the list of its
//! tabs. Each existing entry leads with a number (`/2`), which is how an edited
//! line is traced back to what it was: workspaces count from 1 down the file,
//! and so do tabs, across workspaces, so a tab keeps its number when it moves.
//!
//! ```yaml
//! /1 dots:
//!   - /1 herdr plugin
//!   - new tab
//!
//! new workspace:
//!   - /4 moved here
//! ```
//!
//! It is read a line at a time rather than by a YAML parser, so a mistake is
//! reported on its line, two new workspaces may share a name, and a line that
//! is not quite YAML is still read the way it was meant where that is clear.

use std::fmt::Write as _;

use crate::model::Snapshot;

pub const HELP: &str = "\
# herdr-oil: save and quit to apply, or quit
# without saving to cancel.
#
#   name:      a workspace
#     - name   a tab in it
#   /n         which one a line was; keep it
#
# Move lines to move or reorder them. Edit a
# name to rename. Add a line without a number
# to create, or copy a line for a new tab.
# Delete a line to close it; you are asked
# first, since that stops its processes.
";

/// What a mistake's comment starts with, so the next round can remove it.
const ERROR_PREFIX: &str = "# error: ";

#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub workspaces: Vec<DocWorkspace>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocWorkspace {
    /// The workspace's number in the rendered outline.
    pub number: Option<usize>,
    pub label: String,
    pub line: usize,
    pub tabs: Vec<DocTab>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocTab {
    /// The tab's number in the rendered outline.
    pub number: Option<usize>,
    pub label: String,
    pub line: usize,
}

/// A problem with the text, at a 1-based line.
#[derive(Clone, Debug, PartialEq)]
pub struct LineError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// The outline first, where the editor opens, and the help after it, kept
/// narrow enough for a small popup.
#[must_use]
pub fn render(snapshot: &Snapshot) -> String {
    let mut out = String::new();
    let mut tab_number = 0;
    for (index, workspace) in snapshot.workspaces.iter().enumerate() {
        let _ = writeln!(out, "{}:", scalar(index + 1, &workspace.label));
        for tab in &workspace.tabs {
            tab_number += 1;
            let _ = writeln!(out, "  - {}", scalar(tab_number, &tab.label));
        }
        out.push('\n');
    }
    out.push_str(HELP);
    out
}

/// `text` with each error as a comment above its line, replacing the comments
/// of the round before.
#[must_use]
pub fn annotate(text: &str, errors: &[LineError]) -> String {
    let mut out = String::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim_start().starts_with(ERROR_PREFIX) {
            continue;
        }
        let indent = &line[..line.len() - line.trim_start().len()];
        for error in errors.iter().filter(|e| e.line == index + 1) {
            let _ = writeln!(out, "{indent}{ERROR_PREFIX}{}", error.message);
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// `/n label` as a YAML scalar: plain when YAML reads it back unchanged,
/// double-quoted otherwise.
fn scalar(number: usize, label: &str) -> String {
    let text = if label.is_empty() {
        format!("/{number}")
    } else {
        format!("/{number} {label}")
    };
    let plain = text == text.trim()
        && !text.ends_with(':')
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.chars().any(char::is_control);
    if plain {
        return text;
    }
    let mut quoted = String::from('"');
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\t' => quoted.push_str("\\t"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(quoted, "\\u{:04x}", u32::from(c));
            }
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

pub fn parse(text: &str) -> Result<Doc, Vec<LineError>> {
    let mut workspaces: Vec<DocWorkspace> = Vec::new();
    let mut errors = Vec::new();
    // Some editors save a byte order mark, which `trim` keeps.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut error = |message: &str| {
            errors.push(LineError {
                line,
                message: message.to_owned(),
            });
        };
        let entry = match strip_item_marker(trimmed) {
            Some(rest) => item(rest).map(Entry::Tab),
            None => key(trimmed).map(|(label, tab)| Entry::Workspace(label, tab)),
        };
        let entry = match entry {
            Ok(entry) => entry,
            // Neither `- tab` nor `workspace:`: the indent says which was
            // meant.
            Err(Malformed::NoMarker(text)) if raw.starts_with(char::is_whitespace) => {
                Entry::Tab(text)
            }
            Err(Malformed::NoMarker(text)) => Entry::Workspace(text, None),
            Err(Malformed::Error(message)) => {
                error(&message);
                continue;
            }
        };

        let tab = |text: &str| {
            let (number, label) = split_number(text);
            DocTab {
                number,
                label,
                line,
            }
        };
        match entry {
            Entry::Tab(text) => match workspaces.last_mut() {
                Some(workspace) => workspace.tabs.push(tab(&text)),
                None => {
                    error("this tab is above every workspace; put it under a `workspace:` line");
                }
            },
            Entry::Workspace(text, inline_tab) => {
                let (number, label) = split_number(&text);
                let tabs = inline_tab.as_deref().map(tab).into_iter().collect();
                workspaces.push(DocWorkspace {
                    number,
                    label,
                    line,
                    tabs,
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(Doc { workspaces })
    } else {
        Err(errors)
    }
}

enum Entry {
    Tab(String),
    /// A workspace, and a tab written after its colon (`workspace: tab`).
    Workspace(String, Option<String>),
}

enum Malformed {
    /// A line that is neither `- tab` nor `workspace:`, and its text.
    NoMarker(String),
    Error(String),
}

/// The rest of a `- item` line, or `None` for any other line.
fn strip_item_marker(line: &str) -> Option<&str> {
    if line == "-" {
        return Some("");
    }
    let rest = line.strip_prefix('-')?;
    rest.starts_with(char::is_whitespace).then_some(rest)
}

/// The scalar of an item: everything up to a comment.
fn item(rest: &str) -> Result<String, Malformed> {
    let (value, after) = leading_scalar(rest.trim_start(), false).map_err(Malformed::Error)?;
    if is_blank_or_comment(after) {
        Ok(value)
    } else {
        Err(Malformed::Error(format!(
            "unexpected `{}` after the quoted name",
            after.trim()
        )))
    }
}

/// The scalar of a `key:` line, and the tab written after its colon if any.
fn key(line: &str) -> Result<(String, Option<String>), Malformed> {
    let (value, after) = leading_scalar(line, true).map_err(Malformed::Error)?;
    let Some(after) = after.trim_start().strip_prefix(':') else {
        return if is_blank_or_comment(after) {
            Err(Malformed::NoMarker(value))
        } else {
            Err(Malformed::Error(format!(
                "unexpected `{}` after the quoted name",
                after.trim()
            )))
        };
    };
    if is_blank_or_comment(after) {
        Ok((value, None))
    } else {
        item(after).map(|tab| (value, Some(tab)))
    }
}

fn is_blank_or_comment(text: &str) -> bool {
    let text = text.trim_start();
    text.is_empty() || text.starts_with('#')
}

/// A quoted or plain scalar at the start of `text`, and what follows it. A
/// plain scalar ends at a comment, and in a key also at `: ` or a final `:`.
fn leading_scalar(text: &str, is_key: bool) -> Result<(String, &str), String> {
    if let Some(quoted) = text.strip_prefix('"') {
        return double_quoted(quoted);
    }
    if let Some(quoted) = text.strip_prefix('\'') {
        return single_quoted(quoted);
    }
    let mut end = text.len();
    let mut previous = None;
    for (i, c) in text.char_indices() {
        let after_space = previous.is_none_or(char::is_whitespace);
        if c == '#' && after_space {
            end = i;
            break;
        }
        if is_key && c == ':' {
            let next = text[i + 1..].chars().next();
            if next.is_none_or(char::is_whitespace) {
                end = i;
                break;
            }
        }
        previous = Some(c);
    }
    Ok((text[..end].trim_end().to_owned(), &text[end..]))
}

fn double_quoted(text: &str) -> Result<(String, &str), String> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Ok((value, &text[i + 1..])),
            '\\' => {
                let Some((_, escape)) = chars.next() else {
                    break;
                };
                match escape {
                    '"' | '\\' | '/' => value.push(escape),
                    't' => value.push('\t'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    '0' => value.push('\0'),
                    'x' | 'u' | 'U' => {
                        let digits = match escape {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let hex: String = chars.by_ref().take(digits).map(|(_, c)| c).collect();
                        let c = u32::from_str_radix(&hex, 16)
                            .ok()
                            .filter(|_| hex.len() == digits)
                            .and_then(char::from_u32)
                            .ok_or_else(|| format!("`\\{escape}{hex}` is not a character"))?;
                        value.push(c);
                    }
                    other => return Err(format!("unknown escape `\\{other}` in quotes")),
                }
            }
            c => value.push(c),
        }
    }
    Err("the quoted name has no closing `\"`".into())
}

fn single_quoted(text: &str) -> Result<(String, &str), String> {
    let mut value = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '\'' {
            value.push(c);
        } else if chars.peek().is_some_and(|&(_, next)| next == '\'') {
            chars.next();
            value.push('\'');
        } else {
            return Ok((value, &text[i + 1..]));
        }
    }
    Err("the quoted name has no closing `'`".into())
}

/// Splits a leading `/n` off a name, the number it had in the rendered
/// outline.
fn split_number(text: &str) -> (Option<usize>, String) {
    let text = text.trim();
    if let Some(candidate) = text.strip_prefix('/') {
        let (token, label) = candidate
            .split_once(char::is_whitespace)
            .unwrap_or((candidate, ""));
        let digits = !token.is_empty() && token.chars().all(|c| c.is_ascii_digit());
        if let (true, Ok(number @ 1..)) = (digits, token.parse::<usize>()) {
            return (Some(number), label.trim().to_owned());
        }
    }
    (None, text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Tab, Workspace};

    type Outline<'a> = Vec<(Option<usize>, &'a str, Vec<(Option<usize>, &'a str)>)>;

    fn tab(id: &str, label: &str) -> Tab {
        Tab {
            id: id.into(),
            label: label.into(),
            pane_count: 1,
            agents: Vec::new(),
        }
    }

    fn outline(doc: &Doc) -> Outline<'_> {
        doc.workspaces
            .iter()
            .map(|w| {
                (
                    w.number,
                    w.label.as_str(),
                    w.tabs
                        .iter()
                        .map(|t| (t.number, t.label.as_str()))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn render_numbers_tabs_across_workspaces_and_parses_back() {
        let snapshot = Snapshot {
            workspaces: vec![
                Workspace {
                    id: "w1".into(),
                    label: "dots".into(),
                    tabs: vec![tab("w1:t1", "herdr plugin"), tab("w1:t2", "2")],
                },
                Workspace {
                    id: "wA".into(),
                    label: "work stuff".into(),
                    tabs: vec![tab("wA:t1C", "fix login bug")],
                },
            ],
        };
        let text = render(&snapshot);
        assert!(text.starts_with(
            "/1 dots:\n  - /1 herdr plugin\n  - /2 2\n\n/2 work stuff:\n  - /3 fix login bug\n\n"
        ));
        assert!(text.ends_with(HELP));
        assert_eq!(
            outline(&parse(&text).unwrap()),
            [
                (
                    Some(1),
                    "dots",
                    vec![(Some(1), "herdr plugin"), (Some(2), "2")]
                ),
                (Some(2), "work stuff", vec![(Some(3), "fix login bug")]),
            ]
        );
    }

    #[test]
    fn lines_without_numbers_are_new_and_slash_labels_survive() {
        let doc = parse("experiments:\n  - /tmp scratch\n  -\n  - /4\n  - /0 zero\n").unwrap();
        assert_eq!(
            outline(&doc),
            [(
                None,
                "experiments",
                vec![
                    (None, "/tmp scratch"),
                    (None, ""),
                    (Some(4), ""),
                    (None, "/0 zero")
                ]
            )]
        );
    }

    #[test]
    fn comments_are_skipped_wherever_they_are() {
        let doc = parse("# top\n/1 a: # trailing\n  # - not a tab\n  - /1 b # note\n  - # empty\n")
            .unwrap();
        assert_eq!(
            outline(&doc),
            [(Some(1), "a", vec![(Some(1), "b"), (None, "")])]
        );
    }

    #[test]
    fn a_line_missing_its_marker_is_read_by_its_indent() {
        let doc = parse("/1 a:\n  /2 b\nloose # note\n  - c\nops: deploy\n").unwrap();
        assert_eq!(
            outline(&doc),
            [
                (Some(1), "a", vec![(Some(2), "b")]),
                (None, "loose", vec![(None, "c")]),
                (None, "ops", vec![(None, "deploy")]),
            ]
        );
    }

    #[test]
    fn only_what_cannot_be_read_is_an_error() {
        let errors =
            parse("- orphan\n\"open:\n/1 a:\n  - \"no end\n  - \"bad \\q\"\n  - \"x\" y\n")
                .unwrap_err();
        let lines: Vec<usize> = errors.iter().map(|e| e.line).collect();
        assert_eq!(lines, [1, 2, 4, 5, 6]);
    }

    #[test]
    fn errors_become_comments_that_the_next_round_replaces() {
        let text = "- orphan\n/1 a:\n  - \"open\n";
        let errors = parse(text).unwrap_err();
        let annotated = annotate(text, &errors);
        assert_eq!(
            annotated,
            format!(
                "{ERROR_PREFIX}{}\n- orphan\n/1 a:\n  {ERROR_PREFIX}{}\n  - \"open\n",
                errors[0].message, errors[1].message
            )
        );

        // Fix one; the next round has only the other's comment.
        let fixed = annotated.replace("- orphan\n", "");
        let errors = parse(&fixed).unwrap_err();
        assert_eq!(errors.len(), 1);
        let again = annotate(&fixed, &errors);
        assert_eq!(again.matches(ERROR_PREFIX).count(), 1);
        assert!(again.starts_with("/1 a:\n  # error: "), "{again}");
    }

    #[test]
    fn tolerates_yaml_variations() {
        let text = "\u{feff}# help\r\n  /1   spaced  out  :\r\n- /1\tdeep\r\n\t-   日本語 タブ\r\n'it''s': \r\n  - \"tab \\u00e9\"\r\n";
        assert_eq!(
            outline(&parse(text).unwrap()),
            [
                (
                    Some(1),
                    "spaced  out",
                    vec![(Some(1), "deep"), (None, "日本語 タブ")]
                ),
                (None, "it's", vec![(None, "tab é")]),
            ]
        );
    }

    #[test]
    fn labels_that_look_like_syntax_survive_a_round_trip() {
        let labels = [
            "fix: the bug",
            "issue #42",
            "ends with:",
            "- not an item",
            "# not a comment",
            "\"quoted\" and \\ back",
            "tab\there",
            "/9 not a number?",
            "  ",
        ];
        let snapshot = Snapshot {
            workspaces: vec![Workspace {
                id: "w1".into(),
                label: "key: value #1".into(),
                tabs: labels
                    .iter()
                    .enumerate()
                    .map(|(i, label)| tab(&format!("w1:t{i}"), label))
                    .collect(),
            }],
        };
        let doc = parse(&render(&snapshot)).unwrap();
        assert_eq!(doc.workspaces[0].label, "key: value #1");
        let parsed: Vec<&str> = doc.workspaces[0]
            .tabs
            .iter()
            .map(|t| t.label.as_str())
            .collect();
        let mut expected = labels.to_vec();
        expected[labels.len() - 1] = "";
        assert_eq!(parsed, expected);
    }

    #[test]
    fn the_help_fits_a_small_popup() {
        for line in HELP.lines() {
            assert!(
                line.chars().count() <= 45,
                "too wide for a small popup: {line}"
            );
        }
    }
}
