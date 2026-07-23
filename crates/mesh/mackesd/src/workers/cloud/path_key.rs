//! Path-component validation for cloud state written by the root worker.
//!
//! Cloud request fields arrive over the cross-UID Bus. Any field used below a
//! state root must therefore be one ordinary filename component: never an
//! absolute path, separator-bearing path, or traversal component.

/// Linux filesystems used by the platform cap one filename component at 255
/// bytes. Keys are ASCII-only below, so bytes and characters are identical.
const MAX_COMPONENT_BYTES: usize = 255;

/// Return `value` when it is one path-safe ASCII key component.
///
/// The accepted alphabet covers hostnames and the workload/image identifiers
/// used by the platform while excluding path separators and shell whitespace.
pub(super) fn segment<'a>(field: &str, value: &'a str) -> Result<&'a str, String> {
    if value.is_empty() {
        return Err(format!("missing `{field}`"));
    }
    if value.trim() != value
        || value == "."
        || value == ".."
        || value.len() > MAX_COMPONENT_BYTES
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err(format!(
            "`{field}` must be one path-safe [A-Za-z0-9._-] segment"
        ));
    }
    Ok(value)
}

/// Validate a key that becomes a filename stem after the sink appends
/// `suffix`. This must run before directory creation: a stem that is legal by
/// itself can still exceed the component limit as `<stem>.json` or
/// `<stem>.container`.
pub(super) fn file_stem<'a>(field: &str, value: &'a str, suffix: &str) -> Result<&'a str, String> {
    let value = segment(field, value)?;
    if value.len().saturating_add(suffix.len()) > MAX_COMPONENT_BYTES {
        return Err(format!(
            "`{field}` is too long for its `{suffix}` state filename"
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_platform_host_and_object_keys() {
        assert_eq!(
            segment("node", "node-1.example_lab").unwrap(),
            "node-1.example_lab"
        );
        assert_eq!(segment("name", "web_api.v2").unwrap(), "web_api.v2");
    }

    #[test]
    fn rejects_paths_traversal_whitespace_and_overlong_keys() {
        for bad in [
            "", " ", ".", "..", "../x", "x/y", "/tmp/x", "x\\y", "x y", "x\n",
        ] {
            assert!(segment("key", bad).is_err(), "accepted unsafe key {bad:?}");
        }
        assert!(segment("key", &"x".repeat(256)).is_err());
    }

    #[test]
    fn filename_stems_reserve_space_for_the_sink_suffix() {
        assert!(file_stem("name", &"x".repeat(250), ".json").is_ok());
        assert!(file_stem("name", &"x".repeat(251), ".json").is_err());
        assert!(file_stem("name", &"x".repeat(245), ".container").is_ok());
        assert!(file_stem("name", &"x".repeat(246), ".container").is_err());
    }
}
