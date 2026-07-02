//! The convergence property test (BOOKMARKS-1 acceptance).
//!
//! A CRDT's defining property: **any node that has seen the same op set reaches
//! the same state, regardless of the order the ops arrived.** Here two node-ids
//! issue a set of concurrent, overlapping ops (adds, edits, a move, a delete +
//! LWW resurrection, an add-folder + rename). We apply that set in a canonical
//! order, then in thousands of shuffled orders (proptest) plus a batch of
//! hand-rolled deterministic permutations, and assert the converged
//! [`Collection`] is identical every time.

use mde_bookmarks::{Author, Collection, Hlc, Op, OpKind, Source};
use proptest::prelude::*;
use uuid::Uuid;

fn author(user: &str, node: &str) -> Author {
    Author::new(user.into(), node.into())
}

fn add_bookmark(id: Uuid, parent: Option<Uuid>, order_key: &str, title: &str) -> OpKind {
    OpKind::AddBookmark {
        id,
        parent,
        order_key: order_key.into(),
        url: format!("https://{title}.example/"),
        title: title.into(),
        favicon_ref: None,
        tags: vec![],
        notes: String::new(),
        added: 0,
        source: Source::Manual,
    }
}

fn edit_title(id: Uuid, title: &str) -> OpKind {
    OpKind::EditBookmark {
        id,
        url: None,
        title: Some(title.into()),
        favicon_ref: None,
        tags: None,
        notes: None,
    }
}

/// A fixed set of concurrent ops from two nodes, deliberately overlapping on the
/// same targets with interleaved HLCs so order actually matters to a naive fold.
fn scenario() -> Vec<Op> {
    let f1 = Uuid::from_u128(0x01);
    let b1 = Uuid::from_u128(0x11);
    let b2 = Uuid::from_u128(0x12);

    vec![
        // n1 builds a folder + a bookmark inside it.
        Op::new(
            Hlc::new(1, 0, "n1".into()),
            author("alice", "n1"),
            OpKind::AddFolder {
                id: f1,
                name: "Imported".into(),
                parent: None,
                order_key: "a".into(),
            },
        ),
        Op::new(
            Hlc::new(2, 0, "n1".into()),
            author("alice", "n1"),
            add_bookmark(b1, Some(f1), "b", "b1-original"),
        ),
        // n2 concurrently adds a top-level bookmark.
        Op::new(
            Hlc::new(2, 0, "n2".into()),
            author("bob", "n2"),
            add_bookmark(b2, None, "a", "b2-original"),
        ),
        // Two concurrent title edits on b1 at the SAME (wall, counter) — the
        // node-id tiebreak (n2 > n1) must decide, order-independently.
        Op::new(
            Hlc::new(5, 0, "n1".into()),
            author("alice", "n1"),
            edit_title(b1, "b1-from-n1"),
        ),
        Op::new(
            Hlc::new(5, 0, "n2".into()),
            author("bob", "n2"),
            edit_title(b1, "b1-from-n2"),
        ),
        // A stale lower-HLC edit that must always lose.
        Op::new(
            Hlc::new(3, 0, "n1".into()),
            author("alice", "n1"),
            edit_title(b1, "b1-stale"),
        ),
        // n1 renames the folder.
        Op::new(
            Hlc::new(7, 0, "n1".into()),
            author("alice", "n1"),
            OpKind::RenameFolder {
                id: f1,
                name: "Imported/Firefox".into(),
            },
        ),
        // n2 moves b1 to the top level and reorders it.
        Op::new(
            Hlc::new(6, 0, "n2".into()),
            author("bob", "n2"),
            OpKind::MoveItem {
                id: b1,
                parent: None,
                order_key: "c".into(),
            },
        ),
        // n1 deletes b2 (LWW delete, no tombstone)...
        Op::new(
            Hlc::new(4, 0, "n1".into()),
            author("alice", "n1"),
            OpKind::DeleteItem { id: b2 },
        ),
        // ...then a higher-HLC re-add resurrects it (lock Q4).
        Op::new(
            Hlc::new(8, 0, "n2".into()),
            author("bob", "n2"),
            add_bookmark(b2, None, "d", "b2-reborn"),
        ),
    ]
}

fn converge(ops: &[Op]) -> Collection {
    let mut c = Collection::new();
    c.apply_all(ops.iter());
    c
}

/// The canonical (issue-order) converged state every permutation must match.
fn canonical() -> Collection {
    converge(&scenario())
}

/// A cheap deterministic in-place shuffle (Fisher-Yates over a linear-congruential
/// generator) so the property is also checked without relying on proptest.
fn lcg_shuffle(ops: &mut [Op], mut seed: u64) {
    let n = ops.len();
    for i in (1..n).rev() {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let j = (seed >> 33) as usize % (i + 1);
        ops.swap(i, j);
    }
}

#[test]
fn hand_rolled_shuffles_all_converge() {
    let want = canonical();
    // The reversed order is the harshest single permutation for a naive fold.
    let mut rev = scenario();
    rev.reverse();
    assert_eq!(converge(&rev), want, "reversed order converges");

    // Plus 200 pseudo-random permutations.
    for seed in 0..200u64 {
        let mut shuffled = scenario();
        lcg_shuffle(&mut shuffled, seed.wrapping_add(1));
        assert_eq!(converge(&shuffled), want, "shuffle seed {seed} converges");
    }
}

#[test]
fn resurrection_and_tiebreak_land_in_the_canonical_state() {
    // Pin the observable outcome so the property test isn't asserting equality
    // of a wrong-but-consistent state.
    let c = canonical();
    let f1 = Uuid::from_u128(0x01);
    let b1 = Uuid::from_u128(0x11);
    let b2 = Uuid::from_u128(0x12);

    // Folder renamed by the higher HLC.
    match c.item(f1) {
        Some(mde_bookmarks::Item::Folder(f)) => assert_eq!(f.name, "Imported/Firefox"),
        other => unreachable!("expected renamed folder, got {other:?}"),
    }
    // b1: n2's edit won the tiebreak, and it was moved to the top level.
    match c.item(b1) {
        Some(mde_bookmarks::Item::Bookmark(b)) => {
            assert_eq!(b.title, "b1-from-n2", "node-id tiebreak picks n2");
            assert_eq!(b.parent, None, "moved to top level");
            assert_eq!(b.last_author.user, "bob");
        }
        other => unreachable!("expected b1 bookmark, got {other:?}"),
    }
    // b2: deleted then resurrected by a higher-HLC re-add.
    match c.item(b2) {
        Some(mde_bookmarks::Item::Bookmark(b)) => assert_eq!(b.title, "b2-reborn"),
        other => unreachable!("expected resurrected b2, got {other:?}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Shuffle the op order every which way; the converged tree is invariant.
    #[test]
    fn shuffled_op_order_always_converges(perm in Just(scenario()).prop_shuffle()) {
        prop_assert_eq!(converge(&perm), canonical());
    }
}
