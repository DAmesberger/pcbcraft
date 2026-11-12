//! A bounded S-expression reader for KiCad's text files: lists in
//! parentheses of bare atoms and double-quoted strings. One pass over the
//! bytes with an explicit stack, so nesting costs no recursion; the input
//! size, nesting depth, node count and string length are limited before
//! anything is allocated past them. Errors name a byte offset, never the
//! text, so a refusal cannot quote the design.
use std::fmt;

/// Largest input, bytes.
pub const MAX_INPUT: usize = 64 * 1024 * 1024;
/// Deepest nesting.
pub const MAX_DEPTH: usize = 64;
/// Most atoms and lists in one file.
pub const MAX_NODES: usize = 16 * 1024 * 1024;
/// Longest atom or string, bytes.
pub const MAX_ATOM: usize = 64 * 1024;

/// One node: a list or an atom. `quoted` atoms were written in double
/// quotes (names, values); bare atoms are keywords and numbers.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    List(Vec<Node>),
    Atom { text: String, quoted: bool },
}

/// Why a file was refused, with the byte offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub reason: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.reason, self.offset)
    }
}

impl std::error::Error for ParseError {}

impl Node {
    /// The list's first atom (its keyword).
    pub fn head(&self) -> Option<&str> {
        match self {
            Node::List(items) => items.first().and_then(Node::atom),
            Node::Atom { .. } => None,
        }
    }

    /// The atom's text.
    pub fn atom(&self) -> Option<&str> {
        match self {
            Node::Atom { text, .. } => Some(text),
            Node::List(_) => None,
        }
    }

    /// The list's items after the keyword (empty for an atom).
    pub fn args(&self) -> &[Node] {
        match self {
            Node::List(items) => items.get(1..).unwrap_or_default(),
            Node::Atom { .. } => &[],
        }
    }

    /// The child lists whose keyword is `head`.
    pub fn children<'a>(&'a self, head: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.args().iter().filter(move |n| n.head() == Some(head))
    }

    /// The first child list whose keyword is `head`.
    pub fn child(&self, head: &str) -> Option<&Node> {
        self.args().iter().find(|n| n.head() == Some(head))
    }

    /// The `index`th atom after the keyword.
    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args().get(index).and_then(Node::atom)
    }
}

/// Parse one file holding exactly one top-level list.
pub fn parse(bytes: &[u8]) -> Result<Node, ParseError> {
    let fail = |offset: usize, reason: &'static str| ParseError { offset, reason };
    if bytes.len() > MAX_INPUT {
        return Err(fail(0, "the file is larger than 64 MiB"));
    }
    let mut stack: Vec<Vec<Node>> = Vec::new();
    let mut done: Option<Node> = None;
    let mut nodes = 0usize;
    let mut i = 0usize;
    let count = |nodes: &mut usize, at: usize| -> Result<(), ParseError> {
        *nodes += 1;
        if *nodes > MAX_NODES {
            return Err(fail(at, "the file has more than 16 M nodes"));
        }
        Ok(())
    };
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'(' => {
                if done.is_some() {
                    return Err(fail(i, "content after the top-level list"));
                }
                if stack.len() >= MAX_DEPTH {
                    return Err(fail(i, "nesting deeper than 64"));
                }
                count(&mut nodes, i)?;
                stack.push(Vec::new());
                i += 1;
            }
            b')' => {
                let items = stack.pop().ok_or_else(|| fail(i, "an unbalanced ')'"))?;
                let list = Node::List(items);
                match stack.last_mut() {
                    Some(parent) => parent.push(list),
                    None => done = Some(list),
                }
                i += 1;
            }
            b'"' => {
                let start = i;
                let mut text = Vec::new();
                i += 1;
                loop {
                    let Some(&c) = bytes.get(i) else {
                        return Err(fail(start, "an unterminated string"));
                    };
                    i += 1;
                    match c {
                        b'"' => break,
                        b'\\' => {
                            let Some(&e) = bytes.get(i) else {
                                return Err(fail(start, "an unterminated string"));
                            };
                            i += 1;
                            text.push(match e {
                                b'n' => b'\n',
                                b't' => b'\t',
                                b'r' => b'\r',
                                other => other,
                            });
                        }
                        c => text.push(c),
                    }
                    if text.len() > MAX_ATOM {
                        return Err(fail(start, "a string longer than 64 KiB"));
                    }
                }
                let text =
                    String::from_utf8(text).map_err(|_| fail(start, "a string is not UTF-8"))?;
                count(&mut nodes, start)?;
                stack
                    .last_mut()
                    .ok_or_else(|| fail(start, "an atom outside a list"))?
                    .push(Node::Atom { text, quoted: true });
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'"')
                {
                    i += 1;
                    if i - start > MAX_ATOM {
                        return Err(fail(start, "an atom longer than 64 KiB"));
                    }
                }
                let text = std::str::from_utf8(&bytes[start..i])
                    .map_err(|_| fail(start, "an atom is not UTF-8"))?
                    .to_owned();
                count(&mut nodes, start)?;
                stack
                    .last_mut()
                    .ok_or_else(|| fail(start, "an atom outside a list"))?
                    .push(Node::Atom {
                        text,
                        quoted: false,
                    });
            }
        }
    }
    if !stack.is_empty() {
        return Err(fail(bytes.len(), "an unclosed '('"));
    }
    done.ok_or_else(|| fail(0, "no top-level list"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_atoms_and_strings_with_escapes() -> Result<(), ParseError> {
        let node = parse(b"(a 1.5 \"two \\\"q\\\"\" (b (c x)) ())")?;
        assert_eq!(node.head(), Some("a"));
        assert_eq!(node.arg(0), Some("1.5"));
        assert_eq!(node.arg(1), Some("two \"q\""));
        assert_eq!(
            node.child("b")
                .and_then(|b| b.child("c"))
                .and_then(|c| c.arg(0)),
            Some("x")
        );
        assert_eq!(node.args().len(), 4);
        Ok(())
    }

    #[test]
    fn malformed_and_oversized_input_is_refused_without_quoting_it() {
        for bad in [
            &b"(a"[..],
            b"a)",
            b"(a))",
            b"(a \"open)",
            b"(a) (b)",
            b"",
            b"plain",
        ] {
            assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        let deep = format!("{}{}", "(".repeat(MAX_DEPTH + 1), ")".repeat(MAX_DEPTH + 1));
        let error = parse(deep.as_bytes()).err();
        assert_eq!(error.map(|e| e.reason), Some("nesting deeper than 64"));
        let long = format!("(a \"{}\")", "x".repeat(MAX_ATOM + 1));
        assert!(parse(long.as_bytes()).is_err());
        let secret = parse(b"(a \"SECRET")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(!secret.contains("SECRET") && !secret.is_empty());
    }
}
