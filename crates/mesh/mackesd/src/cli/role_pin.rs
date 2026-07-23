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
        // Thin-lighthouse policy: the historical Lighthouse_Media subclass is
        // retired. Reject the capability at the CLI boundary rather than
        // allowing a day-2 promotion to turn a 512 MiB control-plane node into
        // a media or file-sharing host.
        if media {
            anyhow::bail!(
                "`--media` is retired: DigitalOcean lighthouses are thin control-plane \
                 nodes only; place media/file-sharing duties on a non-lighthouse node"
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
