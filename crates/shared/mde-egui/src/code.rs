//! The **code-theme tokens** (EDITOR-5, §4): the syntax-highlight color
//! vocabulary every code-rendering surface reads.
//!
//! [`Style`](crate::Style) is the single source of the chrome look; this module
//! is its sibling for **code**: the named Carbon-dark colors a syntax
//! highlighter paints source text with. The palette is IBM-Carbon-inspired
//! (purple keywords, blue functions, teal types, green strings, magenta
//! literals) tuned for [`Style::BG`], and — like `style.rs` — the raw hex lives
//! ONLY here (§4: token-only colors everywhere else; this module IS the token
//! source).
//!
//! The vocabulary is [`CodeToken`]: the small set of semantic kinds a
//! tree-sitter capture (or any other lexer) resolves to, each mapped to one
//! named color by [`CodeToken::color`]. Consumers (the editor's highlight
//! engine) classify grammar captures into `CodeToken`s and paint through
//! `color()` — never a raw hex at the paint site.

use crate::egui::Color32;
use crate::style::Style;

/// Carbon purple 40 — keywords, storage words, control flow (`fn`, `def`, `if`).
pub const KEYWORD: Color32 = Color32::from_rgb(0xBE, 0x95, 0xFF);
/// Carbon blue 40 — function, method, and macro names.
pub const FUNCTION: Color32 = Color32::from_rgb(0x78, 0xA9, 0xFF);
/// Carbon teal 30 — types, constructors, namespaces.
pub const TYPE: Color32 = Color32::from_rgb(0x3D, 0xDB, 0xD9);
/// Carbon green 40 — string and character literals.
pub const STRING: Color32 = Color32::from_rgb(0x42, 0xBE, 0x65);
/// Carbon teal 40 — escape sequences inside strings, and attributes/annotations.
pub const ESCAPE: Color32 = Color32::from_rgb(0x08, 0xBD, 0xBA);
/// Carbon magenta 40 — numeric literals and language constants (`true`, `None`).
pub const LITERAL: Color32 = Color32::from_rgb(0xFF, 0x7E, 0xB6);
/// Carbon cyan 40 — properties, fields, object/TOML keys.
pub const PROPERTY: Color32 = Color32::from_rgb(0x33, 0xB1, 0xFF);
/// Muted comment gray — dimmer than [`Style::TEXT_DIM`] so comments recede
/// behind live code, tinted to the Construct blue-gray ramp.
pub const COMMENT: Color32 = Color32::from_rgb(0x6E, 0x6E, 0x7A);
/// Operators + punctuation share the chrome's dim text tone: present, quiet.
pub const PUNCT: Color32 = Style::TEXT_DIM;
/// Plain code text — identifiers and anything unclassified read as the shared
/// foreground, so an unknown capture is honest default text, never invisible.
pub const PLAIN: Color32 = Style::TEXT;

/// One semantic kind of source token — the vocabulary a syntax highlighter
/// resolves grammar captures into, each painted with its named Carbon-dark
/// color via [`color`](Self::color).
///
/// Deliberately small: it names the kinds an operator's eye actually keys on
/// in code, not the full tree-sitter capture taxonomy (the highlight engine
/// folds the long tail of capture names onto these).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeToken {
    /// Keywords + control flow (`fn`, `def`, `return`, `if`).
    Keyword,
    /// Function / method / macro names.
    Function,
    /// Types, constructors, namespaces.
    Type,
    /// String + char literals; markdown raw/code spans.
    String,
    /// Escape sequences inside strings (`\n`, `{}`).
    Escape,
    /// Numeric literals.
    Number,
    /// Language + user constants (`true`, `None`, `SCREAMING_CASE`).
    Constant,
    /// Properties / fields / object + TOML keys.
    Property,
    /// Attributes, annotations, decorators, lifetimes (`#[derive]`, `@wraps`).
    Attribute,
    /// Operators (`+`, `=>`, `&&`).
    Operator,
    /// Brackets + delimiters.
    Punct,
    /// Comments + doc comments.
    Comment,
    /// Markup headings (markdown `#` titles).
    Heading,
    /// Plain text — the honest default for anything unclassified.
    Text,
}

impl CodeToken {
    /// The named Carbon-dark color this token paints with (§4 — the one
    /// token→color map; no raw hex at any paint site).
    #[must_use]
    pub const fn color(self) -> Color32 {
        match self {
            Self::Keyword => KEYWORD,
            Self::Function | Self::Heading => FUNCTION,
            Self::Type => TYPE,
            Self::String => STRING,
            Self::Escape | Self::Attribute => ESCAPE,
            Self::Number | Self::Constant => LITERAL,
            Self::Property => PROPERTY,
            Self::Operator | Self::Punct => PUNCT,
            Self::Comment => COMMENT,
            Self::Text => PLAIN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CodeToken;
    use crate::style::Style;

    /// Every token variant, so the coverage tests can't silently skip one.
    const ALL: [CodeToken; 14] = [
        CodeToken::Keyword,
        CodeToken::Function,
        CodeToken::Type,
        CodeToken::String,
        CodeToken::Escape,
        CodeToken::Number,
        CodeToken::Constant,
        CodeToken::Property,
        CodeToken::Attribute,
        CodeToken::Operator,
        CodeToken::Punct,
        CodeToken::Comment,
        CodeToken::Heading,
        CodeToken::Text,
    ];

    #[test]
    fn every_token_paints_fully_opaque() {
        for token in ALL {
            assert_eq!(
                token.color().a(),
                0xFF,
                "{token:?} must be opaque — translucent glyphs ghost on the dark bg"
            );
        }
    }

    #[test]
    fn comments_recede_and_plain_matches_the_shared_foreground() {
        assert_eq!(
            CodeToken::Text.color(),
            Style::TEXT,
            "plain code text is the one shared foreground token"
        );
        assert_ne!(
            CodeToken::Comment.color(),
            Style::TEXT,
            "comments must read dimmer than live code"
        );
    }

    #[test]
    fn the_core_kinds_are_visually_distinct() {
        // The kinds an eye separates at a glance must not collapse onto one
        // color (shared colors are deliberate only for paired kinds like
        // Number/Constant).
        let core = [
            CodeToken::Keyword,
            CodeToken::Function,
            CodeToken::Type,
            CodeToken::String,
            CodeToken::Number,
            CodeToken::Comment,
            CodeToken::Text,
        ];
        for (i, a) in core.iter().enumerate() {
            for b in &core[i + 1..] {
                assert_ne!(
                    a.color(),
                    b.color(),
                    "{a:?} and {b:?} must be distinguishable"
                );
            }
        }
    }
}
