//! `Playbooks` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.
use crate::*;

/// Handle the `playbooks` subcommand.
#[allow(unreachable_code)]
pub fn run(cmd: PlaybooksCmd) -> anyhow::Result<()> {
    {
        // CB-1.5.b follow-up — curated playbook surface.
        match cmd {
            PlaybooksCmd::List { json } => {
                let root = playbooks_root();
                let mut entries = enumerate_playbook_roles(&root);
                entries.sort();
                let rows: Vec<serde_json::Value> = entries
                    .into_iter()
                    .map(|name| {
                        let description = playbook_description(&name);
                        serde_json::json!({
                            "name":        name,
                            "description": description,
                        })
                    })
                    .collect();
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if rows.is_empty() {
                    println!("(no curated playbooks under {})", root.display());
                } else {
                    for r in &rows {
                        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let desc = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
                        println!("{name:<28} {desc}");
                    }
                }
            }
            PlaybooksCmd::Run { name } => {
                // Spawn ansible-pull directly so the user sees
                // its progress streaming. Exit with whatever
                // ansible-pull exited with.
                let status = std::process::Command::new("ansible-pull")
                    .args(["--tags", &name, "site.yml"])
                    .status();
                match status {
                    Ok(s) => std::process::exit(s.code().unwrap_or(1)),
                    Err(e) => {
                        eprintln!("mded: ansible-pull spawn failed: {e}");
                        std::process::exit(2);
                    }
                }
            }
        }
    }
    Ok(())
}
