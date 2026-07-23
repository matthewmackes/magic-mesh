//! Path-component validation for cloud state written by the root worker.
//!
//! Cloud request fields arrive over the cross-UID Bus. Any field used below a
//! state root must therefore be one ordinary filename component: never an
//! absolute path, separator-bearing path, or traversal component.

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
        || value.len() > 255
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
}
