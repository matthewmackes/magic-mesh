//! **Symbol outline** (EDITOR-12): the file's functions / types / impls, plus the
//! toggleable side panel that lists them and jumps the caret.
//!
//! Symbols are derived from the *same* tree-sitter tree the EDITOR-5
//! [`Highlighter`](crate::highlight::Highlighter) already parses — no second
//! parser.
//!
//! [`symbols_from_tree`] walks the parsed tree once and pulls the named,
//! structural declarations — functions, structs/enums/type aliases, traits,
//! impls, modules — with each symbol's name and the rope char offset to jump to.
//! [`show`] renders the panel: clicking a row hands its `char_start` back to the
//! surface, which reuses the EDITOR-8 / LSP jump seam
//! ([`EditorView::place_cursor`](crate::widget::EditorView::place_cursor)) to
//! reveal it. A language with no grammar, or a file with no symbols, gets an
//! honest empty state — never a fabricated list (§7).
//!
//! The derivation is pure (no egui), so it is unit-testable without a frame.

use mde_egui::code::CodeToken;
use mde_egui::egui::{self, RichText, ScrollArea, Ui};
use mde_egui::Style;
use ropey::Rope;
use tree_sitter::{Node, Tree};

/// The kind of a top-level declaration the outline surfaces — enough to pick an
/// icon + color, mapped from the vendored grammars' node kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    /// A function / method / macro definition.
    Function,
    /// A struct / enum / class / type-alias / interface.
    Type,
    /// A Rust trait.
    Trait,
    /// A Rust `impl` block.
    Impl,
    /// A module.
    Module,
}

impl SymbolKind {
    /// The tree-sitter node kind → outline kind map (curated per vendored
    /// grammar), or `None` for a node that is not a listed declaration.
    fn from_node_kind(kind: &str) -> Option<Self> {
        Some(match kind {
            "function_item"
            | "function_definition"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "macro_definition" => Self::Function,
            "struct_item"
            | "union_item"
            | "enum_item"
            | "type_item"
            | "type_alias_declaration"
            | "enum_declaration"
            | "class_definition"
            | "class_declaration"
            | "interface_declaration" => Self::Type,
            "trait_item" => Self::Trait,
            "impl_item" => Self::Impl,
            "mod_item" | "module" => Self::Module,
            _ => return None,
        })
    }

    /// A small leading glyph for the row (decorative; the color carries meaning).
    const fn glyph(self) -> &'static str {
        match self {
            Self::Function => "\u{0192}", // ƒ
            Self::Type => "\u{25C7}",     // ◇
            Self::Trait => "\u{25B3}",    // △
            Self::Impl => "\u{25C8}",     // ◈
            Self::Module => "\u{25A4}",   // ▤
        }
    }

    /// The Carbon code-token whose color paints this kind's glyph (§4 — reuse the
    /// one token→color map, no raw hex).
    const fn token(self) -> CodeToken {
        match self {
            Self::Function => CodeToken::Function,
            Self::Type | Self::Trait => CodeToken::Type,
            Self::Impl | Self::Module => CodeToken::Keyword,
        }
    }
}

/// One outline entry: a named declaration with its jump target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// The display name (for an `impl`, `Trait for Type` when it names a trait).
    pub name: String,
    /// The declaration kind (icon + color).
    pub kind: SymbolKind,
    /// The 0-based line the name sits on — shown as the `Ln` hint.
    pub line: usize,
    /// The rope **char** offset of the name — the caret jump target (reused by
    /// [`EditorView::place_cursor`](crate::widget::EditorView::place_cursor)).
    pub char_start: usize,
    /// Nesting depth (methods inside an impl indent under it).
    pub depth: usize,
}

/// The text of `node` off the rope, by its byte span (clamped) — no whole-doc
/// materialization.
fn node_text(rope: &Rope, node: Node) -> String {
    let end = node.end_byte().min(rope.len_bytes());
    let start = node.start_byte().min(end);
    rope.byte_slice(start..end).to_string()
}

/// The first identifier-like child of `node` (the name fallback when no `name`
/// field exists — e.g. bash's `word`).
fn first_identifier_child(node: Node) -> Option<Node> {
    let mut cursor = node.walk();
    // Bind before returning so the `children` iterator (which borrows `cursor`)
    // drops before `cursor` does — the found node keeps only the tree lifetime.
    let found = node.children(&mut cursor).find(|c| {
        let k = c.kind();
        k.contains("identifier") || k.contains("name") || k == "word"
    });
    found
}

/// Build the [`Symbol`] for a declaration `node` of `kind`, or `None` when it has
/// no nameable identifier (an anonymous declaration is not a useful jump target).
fn make_symbol(node: Node, kind: SymbolKind, rope: &Rope, depth: usize) -> Option<Symbol> {
    // `impl` names its type (and optionally a trait), not a `name` field.
    let name_node = match kind {
        SymbolKind::Impl => node.child_by_field_name("type"),
        _ => node.child_by_field_name("name"),
    }
    .or_else(|| first_identifier_child(node))?;

    let base = node_text(rope, name_node);
    let name = if kind == SymbolKind::Impl {
        node.child_by_field_name("trait").map_or_else(
            || base.clone(),
            |tr| format!("{} for {base}", node_text(rope, tr)),
        )
    } else {
        base
    };
    if name.trim().is_empty() {
        return None;
    }
    let start_byte = name_node.start_byte().min(rope.len_bytes());
    Some(Symbol {
        name,
        kind,
        line: name_node.start_position().row,
        char_start: rope.byte_to_char(start_byte),
        depth,
    })
}

/// Walk `node` (pre-order), appending each declaration symbol and recursing with a
/// deeper indent under it (so a method lists indented beneath its impl/class).
fn collect(node: Node, rope: &Rope, depth: usize, out: &mut Vec<Symbol>) {
    let mut child_depth = depth;
    if let Some(kind) = SymbolKind::from_node_kind(node.kind()) {
        if let Some(symbol) = make_symbol(node, kind, rope, depth) {
            out.push(symbol);
            child_depth = depth + 1;
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, rope, child_depth, out);
    }
}

/// The document's outline symbols in source order — pure over the already-parsed
/// tree (no re-parse). Reads names straight off the rope by byte span.
#[must_use]
pub fn symbols_from_tree(tree: &Tree, rope: &Rope) -> Vec<Symbol> {
    let mut out = Vec::new();
    collect(tree.root_node(), rope, 0, &mut out);
    out
}

/// Render the outline panel and return the char offset of a clicked symbol (the
/// caret jump target), or `None`.
///
/// Honest empty states (§7): no open document, a language with no grammar, or a
/// grammar that parsed no symbols each show a dimmed note instead of a fake list.
pub fn show(ui: &mut Ui, symbols: &[Symbol], has_grammar: bool, has_doc: bool) -> Option<usize> {
    ui.add_space(Style::SP_XS);
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new("Outline")
                .size(Style::SMALL)
                .color(Style::TEXT_DIM)
                .strong(),
        );
    });
    ui.add_space(Style::SP_XS);
    ui.separator();

    if !has_doc {
        return empty_note(ui, "No file open");
    }
    if !has_grammar {
        return empty_note(ui, "No outline for this file type");
    }
    if symbols.is_empty() {
        return empty_note(ui, "No symbols");
    }

    let mut jump = None;
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for symbol in symbols {
                if symbol_row(ui, symbol) {
                    jump = Some(symbol.char_start);
                }
            }
        });
    jump
}

/// A dimmed honest empty-state note; always returns `None`.
fn empty_note(ui: &mut Ui, text: &str) -> Option<usize> {
    ui.add_space(Style::SP_S);
    ui.horizontal(|ui| {
        ui.add_space(Style::SP_S);
        ui.label(
            RichText::new(text)
                .size(Style::SMALL)
                .color(Style::TEXT_DIM)
                .italics(),
        );
    });
    None
}

/// One symbol row: an indent for depth, the kind glyph in its token color, the
/// clickable name, and a dimmed line hint. Returns whether the name was clicked.
/// Every color is a `Style`/`CodeToken` token (§4).
fn symbol_row(ui: &mut Ui, symbol: &Symbol) -> bool {
    ui.horizontal(|ui| {
        #[allow(clippy::cast_precision_loss)]
        let indent = (symbol.depth as f32).mul_add(Style::SP_M, Style::SP_S);
        ui.add_space(indent);
        ui.label(
            RichText::new(symbol.kind.glyph())
                .size(Style::SMALL)
                .color(symbol.kind.token().color()),
        );
        let clicked = ui
            .selectable_label(
                false,
                RichText::new(&symbol.name)
                    .size(Style::SMALL)
                    .color(Style::TEXT),
            )
            .on_hover_text(format!("Go to line {}", symbol.line + 1))
            .clicked();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(Style::SP_S);
            ui.label(
                RichText::new(format!("{}", symbol.line + 1))
                    .size(Style::SMALL)
                    .color(Style::TEXT_DIM),
            );
        });
        clicked
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::SymbolKind;
    use crate::buffer::Buffer;
    use crate::highlight::{Highlighter, Language};
    use crate::widget::EditorView;

    const SAMPLE: &str = "\
struct Point {
    x: i32,
}

trait Shape {
    fn area(&self) -> i32;
}

impl Shape for Point {
    fn area(&self) -> i32 {
        self.x
    }
}

fn main() {
    let p = Point { x: 3 };
}
";

    fn sample_symbols() -> (Buffer, Vec<super::Symbol>) {
        let mut buf = Buffer::from_text(SAMPLE);
        let mut hl = Highlighter::new(Language::Rust).expect("rust grammar");
        hl.sync(&mut buf);
        let symbols = hl.symbols(buf.rope());
        (buf, symbols)
    }

    #[test]
    fn lists_the_struct_trait_impl_and_functions() {
        let (_buf, symbols) = sample_symbols();
        let names: Vec<(&str, SymbolKind)> =
            symbols.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert!(
            names.contains(&("Point", SymbolKind::Type)),
            "struct Point listed: {names:?}"
        );
        assert!(
            names.contains(&("Shape", SymbolKind::Trait)),
            "trait Shape listed: {names:?}"
        );
        assert!(
            names.contains(&("Shape for Point", SymbolKind::Impl)),
            "the impl names its trait + type: {names:?}"
        );
        assert!(
            names.contains(&("main", SymbolKind::Function)),
            "fn main listed: {names:?}"
        );
        // The method inside the impl is listed too, indented under it.
        let area = symbols
            .iter()
            .find(|s| s.name == "area")
            .expect("the method is listed");
        assert_eq!(area.kind, SymbolKind::Function);
        assert!(area.depth >= 1, "the method indents under its impl");
    }

    #[test]
    fn each_symbol_char_start_lands_on_its_name() {
        let (buf, symbols) = sample_symbols();
        for symbol in &symbols {
            let tail = buf.rope().slice(symbol.char_start..).to_string();
            // The impl's display name is synthetic ("Shape for Point") and its jump
            // target is the type it implements, so the landed word is the last one.
            let target_word = symbol
                .name
                .split_whitespace()
                .last()
                .unwrap_or(&symbol.name);
            assert!(
                tail.starts_with(target_word),
                "{:?} jump char_start must land on its name/type; found {:?}",
                symbol.name,
                &tail.chars().take(12).collect::<String>()
            );
        }
    }

    #[test]
    fn clicking_a_symbol_jumps_the_caret_through_the_place_cursor_seam() {
        let (buf, symbols) = sample_symbols();
        let main = symbols.iter().find(|s| s.name == "main").expect("fn main");
        // The panel hands `char_start` back to the surface, which jumps via the
        // shared EDITOR-8/LSP seam. Exercise that seam directly.
        let mut view = EditorView::new();
        view.place_cursor(&buf, main.char_start);
        assert_eq!(
            view.cursor(),
            main.char_start,
            "the caret jumped to the symbol"
        );
        let (line, _col) = view.line_col(&buf);
        assert_eq!(line - 1, main.line, "the caret is on the symbol's line");
    }

    #[test]
    fn plain_text_yields_no_symbols() {
        // A buffer with no grammar has no highlighter → the panel shows the honest
        // empty state; there is no tree to walk.
        assert!(Highlighter::for_path(std::path::Path::new("notes.txt")).is_none());
    }
}
