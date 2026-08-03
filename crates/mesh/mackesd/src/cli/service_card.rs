//! Privileged local command seam for universal service cards.

use crate::*;

pub fn run(command: ServiceCardCmd) -> anyhow::Result<()> {
    use mackesd_core::workers::service_catalog as catalog;

    let root = mackesd_core::default_qnm_shared_root();
    let result = match command {
        ServiceCardCmd::Save => {
            let submission = catalog::read_configuration_submission(std::io::stdin().lock())
                .map_err(anyhow::Error::msg)?;
            catalog::save_configuration(&root, submission)
        }
        ServiceCardCmd::Test { service_kind } => catalog::test_configuration(&root, &service_kind),
        ServiceCardCmd::Enable { service_kind } => {
            catalog::enable_configuration(&root, &service_kind)
        }
        ServiceCardCmd::Disable { service_kind } => {
            catalog::disable_configuration(&root, &service_kind)
        }
        ServiceCardCmd::Remove { service_kind } => {
            catalog::remove_configuration(&root, &service_kind)
        }
    }
    .map_err(anyhow::Error::msg)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
