//! `MeshSshKey` CLI verb handler.
//!
//! Extracted verbatim from `bin/mackesd.rs` (arch-1). Behaviour is unchanged;
//! only the location moved.
use crate::*;

/// FILEMGR-6 — `mackesd mesh-ssh-key <provision|install|rotate|status>`. The
/// shared mesh SSH keypair is sealed under `mesh-ssh-key` (the ref the FILEMGR-5
/// mesh-mount worker reads); the public half installs for the mesh user behind an
/// overlay-only sshd Match block. `rotate` is the documented re-key path.
pub fn run(cmd: MeshSshKeyCmd) -> anyhow::Result<()> {
    use mackesd_core::ipc::mesh_ssh_key::{MeshKeyProvisioner, ProvisionOutcome, SshdReload};
    use mackesd_core::ipc::secret_store::{repo_root, SecretStore};

    let (args, verb) = match &cmd {
        MeshSshKeyCmd::Provision(a) => (a, "provision"),
        MeshSshKeyCmd::Install(a) => (a, "install"),
        MeshSshKeyCmd::Rotate(a) => (a, "rotate"),
        MeshSshKeyCmd::Status(a) => (a, "status"),
    };
    let repo = args.repo.clone().unwrap_or_else(repo_root);
    let workgroup_root = args
        .workgroup_root
        .clone()
        .unwrap_or_else(mackesd_core::default_qnm_shared_root);
    let store = SecretStore::resolve(&repo, &workgroup_root);

    let mut prov = MeshKeyProvisioner::new(store);
    if let Some(user) = args.mesh_user.clone() {
        prov = prov.with_mesh_user(user);
    }
    // Off-node / `--no-reload`: write the config but never fake the sshd reload.
    if args.no_reload {
        prov = prov.with_sshd_unit(None);
    }

    let report = |o: &ProvisionOutcome| {
        let what = if o.rekeyed {
            "re-keyed"
        } else if o.generated {
            "generated + sealed"
        } else {
            "reused sealed key"
        };
        println!("mesh-ssh-key {verb}: {what}");
        println!("  public: {}", o.public_line);
        match &o.reload {
            SshdReload::Reloaded => println!("  sshd:   reloaded"),
            SshdReload::Skipped => {
                println!("  sshd:   config written (reload skipped — deploy-gated)")
            }
            SshdReload::Gated(why) => {
                println!("  sshd:   config written, reload gated: {why}");
            }
        }
    };

    match cmd {
        MeshSshKeyCmd::Provision(_) => report(&prov.provision().map_err(|e| anyhow::anyhow!(e))?),
        MeshSshKeyCmd::Rotate(_) => report(&prov.rotate().map_err(|e| anyhow::anyhow!(e))?),
        MeshSshKeyCmd::Install(_) => {
            let line = prov
                .sealed_public_line()
                .map_err(|e| anyhow::anyhow!(e))?
                .context(
                "no shared mesh SSH key is sealed yet — run `mackesd mesh-ssh-key provision` first",
            )?;
            let reload = prov.apply(&line).map_err(|e| anyhow::anyhow!(e))?;
            report(&ProvisionOutcome {
                generated: false,
                rekeyed: false,
                public_line: line,
                reload,
            });
        }
        MeshSshKeyCmd::Status(_) => {
            match prov.sealed_public_line().map_err(|e| anyhow::anyhow!(e))? {
                Some(line) => {
                    println!("mesh-ssh-key status: sealed");
                    println!("  public: {line}");
                }
                None => {
                    println!("mesh-ssh-key status: NOT provisioned");
                    std::process::exit(3);
                }
            }
        }
    }
    Ok(())
}
