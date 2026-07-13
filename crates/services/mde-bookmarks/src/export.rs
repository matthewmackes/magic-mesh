//! Netscape-HTML bookmark exporter — the round-trip inverse of
//! [`crate::import::netscape::parse`] (BOOKMARKS-3 export leg).
//!
//! [`to_netscape_html`] renders a converged [`Collection`] as the same
//! `<DL>/<DT>/<H3>/<A HREF>` shape the importer (and every major browser)
//! reads, so a file round-trips: export, then re-import, recovers the same
//! folder names, urls, titles, and nesting order. Pure `&Collection -> String`
//! — no I/O, no credentials (mirrors the importers' own no-I/O contract).

use crate::crdt::Collection;
use crate::model::Item;
use uuid::Uuid;

/// Render `collection` as a standard Netscape bookmark-HTML document — the
/// `File > Export Bookmarks` counterpart to the `File > Import` pickers.
///
/// Walks the tree from [`Collection::roots`], recursing through
/// [`Collection::children`], so nesting and sibling order (the fractional-index
/// `order_key`) match the live tree exactly. Titles, folder names, and URLs are
/// HTML-escaped. `ADD_DATE` is emitted only when a bookmark actually carries a
/// non-zero `added` timestamp (never fabricated); `TAGS` only when non-empty.
/// The output is accepted back by [`crate::import::netscape::parse`] and by
/// every major browser's bookmark importer.
#[must_use]
pub fn to_netscape_html(collection: &Collection) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n");
    out.push_str("<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n");
    out.push_str("<TITLE>Bookmarks</TITLE>\n");
    out.push_str("<H1>Bookmarks</H1>\n");
    out.push_str("<DL><p>\n");
    write_children(&mut out, collection, None, 1);
    out.push_str("</DL><p>\n");
    out
}

/// Recursively emit the `<DT>` entries for `parent`'s children at `depth`
/// (cosmetic 4-space indent per level; the importer is whitespace-tolerant).
fn write_children(out: &mut String, collection: &Collection, parent: Option<Uuid>, depth: usize) {
    let indent = "    ".repeat(depth);
    for item in collection.children(parent) {
        match item {
            Item::Folder(f) => {
                out.push_str(&indent);
                out.push_str("<DT><H3>");
                out.push_str(&html_escape(&f.name));
                out.push_str("</H3>\n");
                out.push_str(&indent);
                out.push_str("<DL><p>\n");
                write_children(out, collection, Some(f.id), depth + 1);
                out.push_str(&indent);
                out.push_str("</DL><p>\n");
            }
            Item::Bookmark(b) => {
                out.push_str(&indent);
                out.push_str("<DT><A HREF=\"");
                out.push_str(&html_escape(&b.url));
                out.push('"');
                if b.added > 0 {
                    out.push_str(" ADD_DATE=\"");
                    out.push_str(&(b.added / 1000).to_string());
                    out.push('"');
                }
                if !b.tags.is_empty() {
                    out.push_str(" TAGS=\"");
                    out.push_str(&html_escape(&b.tags.join(",")));
                    out.push('"');
                }
                out.push('>');
                out.push_str(&html_escape(&b.title));
                out.push_str("</A>\n");
            }
        }
    }
}

/// Escape the characters that would otherwise break Netscape-HTML tag/attribute
/// parsing — the inverse of the importer's (crate-private) `html_unescape`.
/// `&` is escaped first so re-encoding `&lt;`'s `&` never double-escapes.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hlc::{Author, Hlc};
    use crate::import::netscape;
    use crate::import::{ParsedBookmark, ParsedNode};
    use crate::op::{Op, OpKind};

    fn author() -> Author {
        Author::new("alice".into(), "test-node".into())
    }

    fn apply_folder(c: &mut Collection, id: Uuid, name: &str, parent: Option<Uuid>, key: &str) {
        c.apply(&Op::new(
            Hlc::new(1, 0, "n1".into()),
            author(),
            OpKind::AddFolder {
                id,
                name: name.into(),
                parent,
                order_key: key.into(),
            },
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_bookmark(
        c: &mut Collection,
        id: Uuid,
        parent: Option<Uuid>,
        key: &str,
        url: &str,
        title: &str,
        tags: Vec<String>,
        added: u64,
    ) {
        c.apply(&Op::new(
            Hlc::new(1, 0, "n1".into()),
            author(),
            OpKind::AddBookmark {
                id,
                parent,
                order_key: key.into(),
                url: url.into(),
                title: title.into(),
                favicon_ref: None,
                tags,
                notes: String::new(),
                added,
                source: crate::model::Source::Manual,
            },
        ));
    }

    /// Find a folder child node by name among a node slice (order-preserving).
    fn find_folder<'a>(nodes: &'a [ParsedNode], name: &str) -> &'a [ParsedNode] {
        for node in nodes {
            if let ParsedNode::Folder {
                name: n,
                children: c,
            } = node
            {
                if n == name {
                    return c;
                }
            }
        }
        panic!("folder {name:?} not found in {nodes:?}");
    }

    fn only_bookmark(nodes: &[ParsedNode]) -> &ParsedBookmark {
        assert_eq!(nodes.len(), 1, "expected exactly one node in {nodes:?}");
        let ParsedNode::Bookmark(b) = &nodes[0] else {
            panic!("expected a bookmark, got {:?}", nodes[0]);
        };
        b
    }

    #[test]
    fn round_trips_nested_folders_and_bookmarks_through_the_importer() {
        let mut c = Collection::new();
        let reading = Uuid::from_u128(1);
        let nested = Uuid::from_u128(2);
        apply_folder(&mut c, reading, "Reading", None, "a");
        apply_bookmark(
            &mut c,
            Uuid::from_u128(10),
            None,
            "b",
            "https://top-level.example/",
            "Top Level Bookmark",
            vec![],
            0,
        );
        apply_bookmark(
            &mut c,
            Uuid::from_u128(11),
            Some(reading),
            "a",
            "https://blog.example/post",
            "Blog Post",
            vec!["tech".into(), "news".into()],
            1_700_000_000_000,
        );
        apply_folder(&mut c, nested, "Nested", Some(reading), "b");
        apply_bookmark(
            &mut c,
            Uuid::from_u128(12),
            Some(nested),
            "a",
            "https://deep.example/link",
            "Deep Link",
            vec![],
            0,
        );

        let html = to_netscape_html(&c);
        assert!(html.starts_with("<!DOCTYPE NETSCAPE-Bookmark-file-1>"));

        let tree = netscape::parse(&html);
        // Top level: Reading folder, then the top-level bookmark (order_key
        // "a" < "b" matches the live tree's sibling order).
        assert_eq!(tree.roots.len(), 2, "roots: {:?}", tree.roots);
        let ParsedNode::Folder {
            name: reading_name,
            children: reading_children,
        } = &tree.roots[0]
        else {
            panic!("expected Reading folder first, got {:?}", tree.roots[0]);
        };
        assert_eq!(reading_name, "Reading");
        let ParsedNode::Bookmark(top) = &tree.roots[1] else {
            panic!(
                "expected top-level bookmark second, got {:?}",
                tree.roots[1]
            );
        };
        assert_eq!(top.url, "https://top-level.example/");
        assert_eq!(top.title, "Top Level Bookmark");

        // Reading/Blog Post (bookmark) + Reading/Nested (folder), in order.
        assert_eq!(reading_children.len(), 2, "{reading_children:?}");
        let ParsedNode::Bookmark(blog) = &reading_children[0] else {
            panic!("expected Blog Post first, got {:?}", reading_children[0]);
        };
        assert_eq!(blog.url, "https://blog.example/post");
        assert_eq!(blog.title, "Blog Post");
        assert_eq!(blog.tags, vec!["tech".to_string(), "news".to_string()]);
        assert_eq!(blog.added_ms, 1_700_000_000_000);

        let nested_children = find_folder(reading_children, "Nested");
        let deep = only_bookmark(nested_children);
        assert_eq!(deep.url, "https://deep.example/link");
        assert_eq!(deep.title, "Deep Link");
    }

    #[test]
    fn escapes_ampersand_angle_brackets_and_quotes_and_reimports_clean() {
        let mut c = Collection::new();
        apply_folder(&mut c, Uuid::from_u128(1), "R&D <ideas>", None, "a");
        apply_bookmark(
            &mut c,
            Uuid::from_u128(10),
            Some(Uuid::from_u128(1)),
            "a",
            "https://example.com/?a=1&b=2&title=\"x\"",
            "Apple & <Co> \"quoted\"",
            vec![],
            0,
        );

        let html = to_netscape_html(&c);
        // The raw special characters must not appear unescaped inside the tag
        // bodies (they would corrupt a real HTML parser's tag boundaries).
        assert!(html.contains("&amp;"));
        assert!(html.contains("&lt;"));
        assert!(html.contains("&gt;"));
        assert!(html.contains("&quot;"));

        let tree = netscape::parse(&html);
        let folder_children = find_folder(&tree.roots, "R&D <ideas>");
        let bm = only_bookmark(folder_children);
        assert_eq!(bm.title, "Apple & <Co> \"quoted\"");
        assert_eq!(bm.url, "https://example.com/?a=1&b=2&title=\"x\"");
    }

    #[test]
    fn empty_collection_exports_a_valid_minimal_document() {
        let c = Collection::new();
        let html = to_netscape_html(&c);
        assert!(html.starts_with("<!DOCTYPE NETSCAPE-Bookmark-file-1>"));
        assert!(html.contains("<DL><p>"));
        assert!(html.contains("</DL><p>"));

        let tree = netscape::parse(&html);
        assert!(tree.roots.is_empty(), "empty collection -> empty tree");
    }
}
