//! `Identity` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `identity` subcommand.
#[allow(unreachable_code)]
pub fn run(json: bool) -> anyhow::Result<()> {
    {
        // Load (or first-create) this node's signing key, fingerprint
        // it, and render the W25 word-pair.
        let key_path = std::path::PathBuf::from(mackesd_core::node_key::DEFAULT_KEY_PATH);
        let signing = mackesd_core::node_key::load_or_create(&key_path)
            .with_context(|| format!("loading node key at {}", key_path.display()))?;
        let node = mackesd_core::identity::NodeKey::from_bytes(signing.to_bytes());
        let fingerprint = node.fingerprint();
        let word_pair = mackesd_core::identity::fingerprint_word_pair(&fingerprint);
        if json {
            println!(
                "{}",
                serde_json::json!({ "fingerprint": fingerprint, "word_pair": word_pair })
            );
        } else {
            println!("fingerprint: {fingerprint}");
            println!("word-pair:   {word_pair}");
        }
    }
    Ok(())
}
