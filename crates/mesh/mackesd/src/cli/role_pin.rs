//! `RolePin` CLI verb handler.
//!
//! Extracted verbatim from `main()` in `bin/mackesd.rs` (arch-1 SLICE 1:
//! CLI verb handlers). Behaviour is unchanged; only the location moved.

/// Handle the `role-pin` subcommand.
#[allow(unreachable_code)]
pub fn run(role: String, media: bool) -> anyhow::Result<()> {
    {
        let parsed: mde_role::Role = role.parse().map_err(|_| {
            anyhow::anyhow!("unknown role `{role}` — expected lighthouse|workstation")
        })?;
        // MEDIA-1 — pin the role + the media capability tag as a class. The
        // tag is only valid on the lighthouse tier; reject an inapplicable
        // request loudly rather than silently dropping it, so an operator
        // who typed `--media` on the wrong role is told why.
        if media && !mde_role::Capability::Media.applies_to(parsed) {
            anyhow::bail!(
                "`--media` is a lighthouse subclass (Lighthouse_Media) — it cannot apply to \
                     `{}`. Pin `lighthouse --media`, or drop the flag.",
                parsed.as_str()
            );
        }
        let class = mde_role::RoleClass {
            role: parsed,
            media,
        };
        match mde_role::pin_class(&class) {
            Ok(outcome) => {
                println!("role pinned: {outcome:?} (class {class})");
                return Ok(());
            }
            Err(e) => anyhow::bail!("role pin refused: {e}"),
        }
    }
    Ok(())
}
