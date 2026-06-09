//! Portal-18.c (v6.0, R12 lock 2026-05-26) — smart-tag predicate
//! parser + evaluator.
//!
//! A `Smart` tag carries a `predicate: String` (see
//! [`crate::tags::TagFlavor::Smart`]). At query time the predicate
//! is parsed into an AST + evaluated against the universal card
//! index — each surface (apps / files / peers / contacts /
//! containers / workspaces / tray / activities / zones) supplies
//! its membership predicate via the [`MembershipCtx`] trait.
//!
//! ## Grammar
//!
//! ```text
//! pred   ::= or
//! or     ::= and ("or"  and)*
//! and    ::= not ("and" not)*
//! not    ::= "not" not | term
//! term   ::= atom | "(" pred ")"
//! atom   ::= namespace ":" value
//! ```
//!
//! Where `namespace` is one of: `app`, `peer`, `tag`, `contact`,
//! `workspace`, `container`, `tray`, `activity`, `zone`, `file`.
//! `value` is a bare token matching `[A-Za-z0-9._/+-]+`. The
//! grammar is intentionally Lisp-simple — operators don't need
//! regex, ranges, or wildcards in v1.0; complex predicates either
//! get factored into multiple smart tags or wait for a future
//! grammar extension.
//!
//! Whitespace separates tokens; parentheses group sub-expressions.
//! Boolean precedence: `not` > `and` > `or` (standard).
//!
//! ## Bench-observable acceptance (R12 worklist body)
//!
//! Smart tag with predicate `app:firefox or app:chromium` includes
//! both browser cards. The `MembershipCtx::has` callback returns
//! `true` for those two app_ids; the evaluator combines via `Or`.

use std::collections::HashSet;
use std::fmt;

/// Parsed predicate AST. Cheap to evaluate; cloneable so callers
/// can cache parsed predicates per smart tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pred {
    /// `<namespace>:<value>` atomic membership check.
    Atom {
        /// Surface namespace (`app`, `peer`, `tag`, ...).
        namespace: String,
        /// Surface-specific identifier (app_id, hostname, etc.).
        value: String,
    },
    /// Logical negation.
    Not(Box<Pred>),
    /// Logical conjunction. Left associative.
    And(Box<Pred>, Box<Pred>),
    /// Logical disjunction. Left associative.
    Or(Box<Pred>, Box<Pred>),
}

/// Error surface for parser failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Unexpected end-of-input.
    UnexpectedEof,
    /// Unexpected token at byte offset.
    Unexpected {
        /// Offending token in the source.
        token: String,
        /// Approximate byte offset (0-indexed).
        at: usize,
    },
    /// Atom missing `:` separator.
    AtomMissingColon {
        /// The token that should have been `ns:value`.
        token: String,
    },
    /// Empty namespace or value.
    AtomEmptyPart {
        /// The token that had an empty side of the colon.
        token: String,
    },
    /// Unknown namespace (not one of the 10 accepted).
    UnknownNamespace {
        /// The namespace word as seen.
        namespace: String,
    },
    /// Mismatched parentheses.
    UnclosedGroup,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of predicate"),
            Self::Unexpected { token, at } => {
                write!(f, "unexpected `{token}` at offset {at}")
            }
            Self::AtomMissingColon { token } => {
                write!(f, "atom `{token}` missing `:` separator")
            }
            Self::AtomEmptyPart { token } => {
                write!(f, "atom `{token}` has empty namespace or value")
            }
            Self::UnknownNamespace { namespace } => {
                write!(f, "unknown namespace `{namespace}`")
            }
            Self::UnclosedGroup => write!(f, "unclosed `(` group"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Membership context — surface-specific implementation that
/// answers "does the namespace contain this value for the entity
/// being evaluated."
///
/// Concrete impls live in each surface crate (mde-portal for Hub
/// cards, mde-files for file cards, etc.); this crate stays
/// data-only.
pub trait MembershipCtx {
    /// Return `true` if the entity has the given (namespace, value)
    /// pair. Namespace is one of the 10 accepted forms; value is
    /// the surface-specific id (app_id, hostname, tag name, ...).
    fn has(&self, namespace: &str, value: &str) -> bool;
}

/// Accepted namespace strings. Locked at parse time so a typo
/// surfaces immediately rather than at eval time.
pub const NAMESPACES: &[&str] = &[
    "app",
    "peer",
    "tag",
    "contact",
    "workspace",
    "container",
    "tray",
    "activity",
    "zone",
    "file",
];

/// Parse a predicate source string into a [`Pred`] AST.
pub fn parse(src: &str) -> Result<Pred, ParseError> {
    let tokens = lex(src)?;
    let mut parser = Parser { tokens, pos: 0 };
    let pred = parser.parse_or()?;
    if parser.pos < parser.tokens.len() {
        let tok = &parser.tokens[parser.pos];
        return Err(ParseError::Unexpected {
            token: tok.text.clone(),
            at: tok.byte_offset,
        });
    }
    Ok(pred)
}

/// Evaluate a parsed AST against a membership context.
#[must_use]
pub fn evaluate<C: MembershipCtx>(pred: &Pred, ctx: &C) -> bool {
    match pred {
        Pred::Atom { namespace, value } => ctx.has(namespace, value),
        Pred::Not(inner) => !evaluate(inner, ctx),
        Pred::And(a, b) => evaluate(a, ctx) && evaluate(b, ctx),
        Pred::Or(a, b) => evaluate(a, ctx) || evaluate(b, ctx),
    }
}

// ── Tokenizer ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    text: String,
    byte_offset: usize,
}

fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'(' || c == b')' {
            out.push(Token {
                text: (c as char).to_string(),
                byte_offset: i,
            });
            i += 1;
            continue;
        }
        // Bare-token run — letters, digits, `_`, `.`, `/`, `+`,
        // `-`, `:` (for atom-internal colons).
        let start = i;
        while i < bytes.len() {
            let b = bytes[i];
            let is_word = b.is_ascii_alphanumeric()
                || b == b'_'
                || b == b'.'
                || b == b'/'
                || b == b'+'
                || b == b'-'
                || b == b':';
            if !is_word {
                break;
            }
            i += 1;
        }
        if i == start {
            return Err(ParseError::Unexpected {
                token: (c as char).to_string(),
                at: start,
            });
        }
        let text = src[start..i].to_string();
        out.push(Token {
            text,
            byte_offset: start,
        });
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn bump(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let t = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(t)
        } else {
            None
        }
    }

    fn parse_or(&mut self) -> Result<Pred, ParseError> {
        let mut lhs = self.parse_and()?;
        while matches!(self.peek(), Some(t) if t.text == "or") {
            self.bump();
            let rhs = self.parse_and()?;
            lhs = Pred::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<Pred, ParseError> {
        let mut lhs = self.parse_not()?;
        while matches!(self.peek(), Some(t) if t.text == "and") {
            self.bump();
            let rhs = self.parse_not()?;
            lhs = Pred::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_not(&mut self) -> Result<Pred, ParseError> {
        if matches!(self.peek(), Some(t) if t.text == "not") {
            self.bump();
            let inner = self.parse_not()?;
            return Ok(Pred::Not(Box::new(inner)));
        }
        self.parse_term()
    }

    fn parse_term(&mut self) -> Result<Pred, ParseError> {
        let tok = self.peek().cloned().ok_or(ParseError::UnexpectedEof)?;
        if tok.text == "(" {
            self.bump();
            let inner = self.parse_or()?;
            let close = self.bump().ok_or(ParseError::UnclosedGroup)?;
            if close.text != ")" {
                return Err(ParseError::Unexpected {
                    token: close.text,
                    at: close.byte_offset,
                });
            }
            return Ok(inner);
        }
        if tok.text == ")" {
            return Err(ParseError::Unexpected {
                token: tok.text,
                at: tok.byte_offset,
            });
        }
        if tok.text == "or" || tok.text == "and" || tok.text == "not" {
            return Err(ParseError::Unexpected {
                token: tok.text,
                at: tok.byte_offset,
            });
        }
        self.bump();
        parse_atom(&tok.text)
    }
}

fn parse_atom(token: &str) -> Result<Pred, ParseError> {
    let Some((ns, value)) = token.split_once(':') else {
        return Err(ParseError::AtomMissingColon {
            token: token.to_string(),
        });
    };
    if ns.is_empty() || value.is_empty() {
        return Err(ParseError::AtomEmptyPart {
            token: token.to_string(),
        });
    }
    if !NAMESPACES.contains(&ns) {
        return Err(ParseError::UnknownNamespace {
            namespace: ns.to_string(),
        });
    }
    Ok(Pred::Atom {
        namespace: ns.to_string(),
        value: value.to_string(),
    })
}

// ── Test fixture: simple membership ctx backed by a HashSet ─────────────

/// Convenience [`MembershipCtx`] impl backed by a `HashSet<(ns,
/// value)>`. Useful for tests + as a worked example.
#[derive(Debug, Clone, Default)]
pub struct StaticMembership {
    pairs: HashSet<(String, String)>,
}

impl StaticMembership {
    /// Construct an empty ctx.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a (namespace, value) membership pair.
    pub fn add(&mut self, namespace: &str, value: &str) -> &mut Self {
        self.pairs
            .insert((namespace.to_string(), value.to_string()));
        self
    }
}

impl MembershipCtx for StaticMembership {
    fn has(&self, namespace: &str, value: &str) -> bool {
        self.pairs
            .contains(&(namespace.to_string(), value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(pairs: &[(&str, &str)]) -> StaticMembership {
        let mut m = StaticMembership::new();
        for (ns, v) in pairs {
            m.add(ns, v);
        }
        m
    }

    #[test]
    fn parse_simple_atom() {
        let p = parse("app:firefox").unwrap();
        assert_eq!(
            p,
            Pred::Atom {
                namespace: "app".to_string(),
                value: "firefox".to_string(),
            }
        );
    }

    /// Bench acceptance mirror: `app:firefox or app:chromium`
    /// matches both browsers.
    #[test]
    fn evaluate_or_includes_both_branches() {
        let p = parse("app:firefox or app:chromium").unwrap();
        let firefox_ctx = ctx(&[("app", "firefox")]);
        let chromium_ctx = ctx(&[("app", "chromium")]);
        let helix_ctx = ctx(&[("app", "helix")]);
        assert!(evaluate(&p, &firefox_ctx));
        assert!(evaluate(&p, &chromium_ctx));
        assert!(!evaluate(&p, &helix_ctx));
    }

    #[test]
    fn parse_and_higher_precedence_than_or() {
        // `a:1 or b:2 and c:3` parses as `a:1 or (b:2 and c:3)`.
        let p = parse("app:a or peer:b and tag:c").unwrap();
        let only_a = ctx(&[("app", "a")]);
        let only_b = ctx(&[("peer", "b")]);
        let b_and_c = ctx(&[("peer", "b"), ("tag", "c")]);
        assert!(evaluate(&p, &only_a)); // a:1 matches
        assert!(!evaluate(&p, &only_b)); // b alone doesn't satisfy b and c
        assert!(evaluate(&p, &b_and_c)); // b and c satisfies the second branch
    }

    #[test]
    fn not_negates() {
        let p = parse("not app:firefox").unwrap();
        let firefox_ctx = ctx(&[("app", "firefox")]);
        let helix_ctx = ctx(&[("app", "helix")]);
        assert!(!evaluate(&p, &firefox_ctx));
        assert!(evaluate(&p, &helix_ctx));
    }

    #[test]
    fn parentheses_override_precedence() {
        // `(a:1 or b:2) and c:3` requires c:3 always; lhs must
        // be either a:1 or b:2.
        let p = parse("(app:a or peer:b) and tag:c").unwrap();
        let a_only = ctx(&[("app", "a")]);
        let a_and_c = ctx(&[("app", "a"), ("tag", "c")]);
        let b_and_c = ctx(&[("peer", "b"), ("tag", "c")]);
        let c_only = ctx(&[("tag", "c")]);
        assert!(!evaluate(&p, &a_only));
        assert!(evaluate(&p, &a_and_c));
        assert!(evaluate(&p, &b_and_c));
        assert!(!evaluate(&p, &c_only));
    }

    #[test]
    fn nested_not_double_negates() {
        let p = parse("not not app:firefox").unwrap();
        let ff = ctx(&[("app", "firefox")]);
        let hx = ctx(&[("app", "helix")]);
        assert!(evaluate(&p, &ff));
        assert!(!evaluate(&p, &hx));
    }

    #[test]
    fn parse_all_ten_namespaces() {
        for ns in NAMESPACES {
            let src = format!("{ns}:sample");
            let p = parse(&src).expect("each namespace parses");
            assert_eq!(
                p,
                Pred::Atom {
                    namespace: ns.to_string(),
                    value: "sample".to_string(),
                }
            );
        }
    }

    #[test]
    fn unknown_namespace_rejected_at_parse_time() {
        let err = parse("unknown:firefox").unwrap_err();
        assert!(
            matches!(err, ParseError::UnknownNamespace { ref namespace } if namespace == "unknown")
        );
    }

    #[test]
    fn atom_missing_colon_rejected() {
        let err = parse("firefox").unwrap_err();
        assert!(matches!(err, ParseError::AtomMissingColon { ref token } if token == "firefox"));
    }

    #[test]
    fn atom_empty_parts_rejected() {
        assert!(matches!(
            parse(":firefox").unwrap_err(),
            ParseError::AtomEmptyPart { .. }
        ));
        assert!(matches!(
            parse("app:").unwrap_err(),
            ParseError::AtomEmptyPart { .. }
        ));
    }

    #[test]
    fn unclosed_paren_rejected() {
        assert!(matches!(
            parse("(app:firefox").unwrap_err(),
            ParseError::UnclosedGroup
        ));
    }

    #[test]
    fn empty_input_rejected() {
        assert!(matches!(parse("").unwrap_err(), ParseError::UnexpectedEof));
        assert!(matches!(
            parse("   ").unwrap_err(),
            ParseError::UnexpectedEof
        ));
    }

    #[test]
    fn complex_predicate_parses() {
        let p = parse("(app:firefox or app:chromium) and not tag:archived").unwrap();
        let firefox_live = ctx(&[("app", "firefox")]);
        let firefox_archived = ctx(&[("app", "firefox"), ("tag", "archived")]);
        let helix_archived = ctx(&[("app", "helix"), ("tag", "archived")]);
        assert!(evaluate(&p, &firefox_live));
        assert!(!evaluate(&p, &firefox_archived));
        assert!(!evaluate(&p, &helix_archived));
    }

    /// Values can contain dots, dashes, underscores, slashes, plus
    /// signs, and digits — typical app_id / hostname shapes.
    #[test]
    fn values_accept_full_atom_character_set() {
        let p = parse("app:org.mozilla.firefox").unwrap();
        let ff = ctx(&[("app", "org.mozilla.firefox")]);
        assert!(evaluate(&p, &ff));
        let p = parse("peer:fedora-laptop_1").unwrap();
        let host = ctx(&[("peer", "fedora-laptop_1")]);
        assert!(evaluate(&p, &host));
        let p = parse("file:/home/op/notes.md").unwrap();
        let path = ctx(&[("file", "/home/op/notes.md")]);
        assert!(evaluate(&p, &path));
    }
}
