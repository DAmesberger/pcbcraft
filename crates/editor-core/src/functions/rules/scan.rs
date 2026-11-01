//! A static look at a rule's source: every helper call whose arguments are
//! all string literals, so publishing can check references a script names
//! on a branch the default assignment never takes. Anything the scanner
//! does not understand (a computed name, a method call, a string with
//! interpolation) is skipped; evaluation still checks it at run time.

/// One helper call with literal arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LiteralCall {
    pub helper: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    Str(String),
    /// A string the scanner does not read literally (character literals,
    /// backtick strings with possible interpolation).
    Opaque,
    Punct(char),
}

/// The helpers whose literal arguments are references.
pub(super) const HELPERS: &[&str] = &[
    "active",
    "active_count",
    "function",
    "used",
    "kind",
    "role",
    "option",
    "excludes",
    "requires",
];

fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && next == Some('/') {
            while chars.get(i).is_some_and(|c| *c != '\n') {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0_usize;
            while let Some(&c) = chars.get(i) {
                let n = chars.get(i + 1).copied();
                if c == '/' && n == Some('*') {
                    depth += 1;
                    i += 2;
                } else if c == '*' && n == Some('/') {
                    depth = depth.saturating_sub(1);
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == '"' || c == '\'' {
            let quote = c;
            let mut text = String::new();
            i += 1;
            while let Some(&c) = chars.get(i) {
                i += 1;
                match c {
                    '\\' => {
                        if let Some(&e) = chars.get(i) {
                            i += 1;
                            text.push(match e {
                                'n' => '\n',
                                't' => '\t',
                                'r' => '\r',
                                other => other,
                            });
                        }
                    }
                    c if c == quote => break,
                    c => text.push(c),
                }
            }
            out.push(if quote == '"' {
                Token::Str(text)
            } else {
                Token::Opaque
            });
        } else if c == '`' {
            i += 1;
            while chars.get(i).is_some_and(|c| *c != '`') {
                i += 1;
            }
            i += 1;
            out.push(Token::Opaque);
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while chars
                .get(i)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                i += 1;
            }
            let word = chars
                .get(start..i)
                .map(|w| w.iter().collect())
                .unwrap_or_default();
            out.push(Token::Ident(word));
        } else {
            out.push(Token::Punct(c));
            i += 1;
        }
    }
    out
}

/// Every free call of a [`HELPERS`] function whose arguments are all string
/// literals (or, for `active_count`, one array of string literals).
pub(super) fn literal_calls(source: &str) -> Vec<LiteralCall> {
    let tokens = tokens(source);
    let mut out = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        let Token::Ident(name) = token else {
            continue;
        };
        if !HELPERS.contains(&name.as_str()) {
            continue;
        }
        let before = at.checked_sub(1).and_then(|b| tokens.get(b));
        if matches!(before, Some(Token::Punct('.')))
            || matches!(before, Some(Token::Ident(word)) if word == "fn")
        {
            continue;
        }
        if tokens.get(at + 1) != Some(&Token::Punct('(')) {
            continue;
        }
        let array = name == "active_count";
        let mut j = at + 2;
        if array {
            if tokens.get(j) != Some(&Token::Punct('[')) {
                continue;
            }
            j += 1;
        }
        let close = if array { ']' } else { ')' };
        let mut args = Vec::new();
        let mut literal = true;
        loop {
            match tokens.get(j) {
                Some(Token::Punct(c)) if *c == close && args.is_empty() => break,
                Some(Token::Str(text)) => {
                    args.push(text.clone());
                    j += 1;
                    match tokens.get(j) {
                        Some(Token::Punct(',')) => j += 1,
                        Some(Token::Punct(c)) if *c == close => break,
                        _ => {
                            literal = false;
                            break;
                        }
                    }
                }
                _ => {
                    literal = false;
                    break;
                }
            }
        }
        if literal && !args.is_empty() {
            out.push(LiteralCall {
                helper: name.clone(),
                args,
            });
        }
    }
    out
}
