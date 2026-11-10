//! Building the tree from the parser's events.

use std::collections::HashMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Span};

use crate::diagnostic::source::{Loc, SourceId};
use crate::diagnostic::{Diagnostic, Diagnostics};

use super::{Entry, Kind, Node, Scalar, Str, schema};

/// Reads one YAML document. A file that is not valid YAML gives one error and
/// no tree. Duplicate keys, tags and extra documents are errors too, but the
/// tree is still returned so that the rest of the file can be checked.
pub fn parse(text: &str, source: SourceId, d: &mut Diagnostics) -> Option<Node> {
    let mut builder = Builder {
        source,
        text,
        char_starts: text
            .char_indices()
            .map(|(at, _)| at)
            .chain([text.len()])
            .collect(),
        anchors: HashMap::new(),
        stack: Vec::new(),
        root: None,
        documents: 0,
        d,
    };
    for event in Parser::new_from_str(text) {
        match event {
            Ok((event, span)) => builder.event(event, span),
            Err(error) => {
                let marker = error.marker();
                let loc = Loc::new(source, line_of(marker.line()), column_of(marker.col()));
                builder.d.push(Diagnostic::new(
                    loc,
                    format!("this file is not valid YAML: {}", error.info()),
                ));
                return None;
            }
        }
    }
    Some(builder.root.unwrap_or(Node {
        loc: Loc::new(source, 1, 1),
        kind: Kind::Scalar(Scalar::Null),
    }))
}

fn line_of(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

fn column_of(col0: usize) -> u32 {
    u32::try_from(col0 + 1).unwrap_or(u32::MAX)
}

enum Frame {
    Map {
        loc: Loc,
        anchor: usize,
        entries: Vec<Entry>,
        key: Option<Node>,
    },
    Seq {
        loc: Loc,
        anchor: usize,
        items: Vec<Node>,
    },
}

struct Builder<'t, 'd> {
    source: SourceId,
    text: &'t str,
    /// The byte offset of each character, and the text's length at the end:
    /// the parser counts positions in characters.
    char_starts: Vec<usize>,
    anchors: HashMap<usize, Node>,
    stack: Vec<Frame>,
    root: Option<Node>,
    documents: usize,
    d: &'d mut Diagnostics,
}

impl Builder<'_, '_> {
    fn loc(&self, span: Span) -> Loc {
        Loc::new(
            self.source,
            line_of(span.start.line()),
            column_of(span.start.col()),
        )
    }

    fn event(&mut self, event: Event<'_>, span: Span) {
        let loc = self.loc(span);
        match event {
            Event::DocumentStart(_) => {
                self.documents += 1;
                if self.documents == 2 {
                    self.d.error(
                        loc,
                        "a definition file holds one YAML document; this is a second one",
                    );
                }
            }
            Event::Scalar(value, style, anchor, tag) => {
                if tag.is_some() {
                    self.refuse_tag(loc);
                }
                let scalar = match style {
                    ScalarStyle::Plain => match schema::resolve_plain(&value) {
                        Scalar::Str(_) => self.string(&value, style, span),
                        other => other,
                    },
                    _ => self.string(&value, style, span),
                };
                self.complete(
                    Node {
                        loc,
                        kind: Kind::Scalar(scalar),
                    },
                    anchor,
                );
            }
            Event::Alias(anchor) => match self.anchors.get(&anchor) {
                Some(node) => {
                    let node = node.clone();
                    self.complete(node, 0);
                }
                None => self.d.error(loc, "this alias names no anchor"),
            },
            Event::MappingStart(anchor, tag) => {
                if tag.is_some() {
                    self.refuse_tag(loc);
                }
                self.stack.push(Frame::Map {
                    loc,
                    anchor,
                    entries: Vec::new(),
                    key: None,
                });
            }
            Event::SequenceStart(anchor, tag) => {
                if tag.is_some() {
                    self.refuse_tag(loc);
                }
                self.stack.push(Frame::Seq {
                    loc,
                    anchor,
                    items: Vec::new(),
                });
            }
            Event::MappingEnd => {
                if let Some(Frame::Map {
                    loc,
                    anchor,
                    entries,
                    ..
                }) = self.stack.pop()
                {
                    self.complete(
                        Node {
                            loc,
                            kind: Kind::Map(entries),
                        },
                        anchor,
                    );
                }
            }
            Event::SequenceEnd => {
                if let Some(Frame::Seq { loc, anchor, items }) = self.stack.pop() {
                    self.complete(
                        Node {
                            loc,
                            kind: Kind::Seq(items),
                        },
                        anchor,
                    );
                }
            }
            Event::StreamStart | Event::StreamEnd | Event::DocumentEnd | Event::Nothing => {}
        }
    }

    fn refuse_tag(&mut self, loc: Loc) {
        self.d.error(
            loc,
            "tags (`!...`) are not used in tay definitions; quote a value to make it text",
        );
    }

    fn string(&self, value: &str, style: ScalarStyle, span: Span) -> Scalar {
        let quote = match style {
            ScalarStyle::Plain => None,
            ScalarStyle::SingleQuoted => Some('\''),
            ScalarStyle::DoubleQuoted => Some('"'),
            ScalarStyle::Literal | ScalarStyle::Folded => {
                return Scalar::Str(Str {
                    text: value.to_string(),
                    content_column: None,
                });
            }
        };
        // Compared from the start and the value's length: the parser's end of
        // a quoted scalar inside a flow mapping runs one character long.
        let from = span.start.index() + usize::from(quote.is_some());
        let to = from + value.chars().count();
        let written = self
            .char_starts
            .get(from)
            .zip(self.char_starts.get(to))
            .and_then(|(from, to)| self.text.get(*from..*to));
        let closed = quote.is_none_or(|quote| {
            self.char_starts
                .get(to)
                .and_then(|at| self.text.get(*at..))
                .is_some_and(|rest| rest.starts_with(quote))
        });
        let content_column = (written == Some(value) && closed && !value.contains('\n'))
            .then(|| column_of(span.start.col() + usize::from(quote.is_some())));
        Scalar::Str(Str {
            text: value.to_string(),
            content_column,
        })
    }

    fn complete(&mut self, node: Node, anchor: usize) {
        if anchor != 0 {
            self.anchors.insert(anchor, node.clone());
        }
        match self.stack.last_mut() {
            None => {
                if self.documents <= 1 {
                    self.root = Some(node);
                }
            }
            Some(Frame::Seq { items, .. }) => items.push(node),
            Some(Frame::Map { entries, key, .. }) => match key.take() {
                None => *key = Some(node),
                Some(key_node) => {
                    let Some(text) = key_node.key_text() else {
                        self.d
                            .error(key_node.loc, "a key must be a name or a number");
                        return;
                    };
                    let first = entries
                        .iter()
                        .find(|entry| entry.key.key_text().as_deref() == Some(text.as_str()));
                    if let Some(first) = first {
                        self.d.push(
                            Diagnostic::new(
                                key_node.loc,
                                format!("`{text}` appears twice in this mapping"),
                            )
                            .with(first.key.loc, "the first one is here"),
                        );
                        return;
                    }
                    entries.push(Entry {
                        key: key_node,
                        value: node,
                    });
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::source::Sources;

    fn read(text: &str) -> (Option<Node>, Vec<Diagnostic>) {
        let mut sources = Sources::default();
        let id = sources.add_argument("test");
        let mut d = Diagnostics::default();
        let node = parse(text, id, &mut d);
        (node, d.into_sorted())
    }

    fn at(node: &Node) -> (u32, u32) {
        (node.loc.line, node.loc.column)
    }

    #[test]
    fn keys_and_values_keep_their_positions() {
        let (node, errors) = read("a: 1\nlist:\n  - x\n  - { y: 'z' }\n");
        assert!(errors.is_empty());
        let entries = node.unwrap();
        let entries = entries.as_map().unwrap();
        assert_eq!(at(&entries[0].key), (1, 1));
        assert_eq!(at(&entries[0].value), (1, 4));
        let list = entries[1].value.as_seq().unwrap();
        assert_eq!(at(&list[0]), (3, 5));
        let inner = list[1].as_map().unwrap();
        assert_eq!(at(&inner[0].key), (4, 7));
        assert_eq!(at(&inner[0].value), (4, 10));
    }

    #[test]
    fn quoting_keeps_text_and_plain_words_follow_yaml_1_2() {
        let (node, _) = read("a: \"5432\"\nb: 5432\nc: no\n");
        let node = node.unwrap();
        let entries = node.as_map().unwrap();
        assert_eq!(entries[0].value.as_str().unwrap().text, "5432");
        assert_eq!(entries[1].value.kind, Kind::Scalar(Scalar::Int(5432)));
        assert_eq!(entries[2].value.as_str().unwrap().text, "no");
    }

    #[test]
    fn a_duplicate_key_names_both_places_and_reading_goes_on() {
        let (node, errors) = read("a: 1\nb: 2\na: 3\nc: 4\n");
        assert_eq!(errors.len(), 1);
        assert_eq!((errors[0].loc.line, errors[0].loc.column), (3, 1));
        assert_eq!(errors[0].related[0].loc.line, 1);
        assert_eq!(node.unwrap().as_map().unwrap().len(), 3);
    }

    #[test]
    fn a_syntax_error_gives_its_line_and_no_tree() {
        let (node, errors) = read("a: 1\n b: 2\n");
        assert!(node.is_none());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].loc.line, 2);
        assert!(errors[0].message.starts_with("this file is not valid YAML"));
    }

    #[test]
    fn content_column_is_known_only_when_the_value_is_written_out_as_is() {
        let (node, _) = read(
            "a: x ${y}\nb: \"x ${y}\"\nc: \"x\\t${y}\"\nd: |\n  x ${y}\ne: { f: \"x ${y}\" }\ng: 'it''s ${y}'\n",
        );
        let node = node.unwrap();
        let entries = node.as_map().unwrap();
        let column = |i: usize| entries[i].value.as_str().map(|s| s.content_column);
        assert_eq!(column(0), Some(Some(4)));
        assert_eq!(column(1), Some(Some(5)));
        assert_eq!(column(2), Some(None));
        assert_eq!(column(3), Some(None));
        let flow = entries[4].value.as_map().unwrap();
        assert_eq!(flow[0].value.as_str().unwrap().content_column, Some(10));
        assert_eq!(column(5), Some(None));
    }

    #[test]
    fn an_alias_repeats_its_anchor_and_a_tag_is_refused() {
        let (node, errors) = read("a: &x [1, 2]\nb: *x\nc: !!str 5\n");
        let node = node.unwrap();
        let entries = node.as_map().unwrap();
        assert_eq!(entries[1].value.as_seq().unwrap().len(), 2);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].loc.line, 3);
    }

    #[test]
    fn a_mapping_laid_over_another_replaces_by_key_and_joins_named_entries_one_by_one() {
        let (base, _) =
            read("env: { A: 1, B: 2 }\nbuild: { context: a, args: { X: 1 } }\nkeep: 1\n");
        let (over, _) = read("env: { B: 3, C: 4 }\nbuild: { context: b }\nnew: 5\n");
        let joined = base.unwrap().joined(&over.unwrap(), &["env"]);
        let entries = joined.as_map().unwrap();
        let keys: Vec<_> = entries
            .iter()
            .map(|e| e.key.as_str().unwrap().text.as_str())
            .collect();
        assert_eq!(keys, ["env", "build", "keep", "new"]);
        let env: Vec<_> = entries[0]
            .value
            .as_map()
            .unwrap()
            .iter()
            .map(|e| (e.key.as_str().unwrap().text.clone(), e.value.kind.clone()))
            .collect();
        assert_eq!(
            env,
            [
                ("A".to_string(), Kind::Scalar(Scalar::Int(1))),
                ("B".to_string(), Kind::Scalar(Scalar::Int(3))),
                ("C".to_string(), Kind::Scalar(Scalar::Int(4))),
            ]
        );
        // `build` is not joined entry by entry: the later one replaces it whole.
        assert_eq!(entries[1].value.as_map().unwrap().len(), 1);
    }

    #[test]
    fn a_second_document_is_an_error() {
        let (_, errors) = read("a: 1\n---\nb: 2\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].loc.line, 2);
    }
}
