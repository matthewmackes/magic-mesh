fn main() {
    println!("cargo:rerun-if-env-changed=MCNF_BUILD_SOURCE_REVISION");
    let revision = std::env::var("MCNF_BUILD_SOURCE_REVISION")
        .unwrap_or_else(|_| "0000000000000000000000000000000000000000".to_owned());
    assert!(
        revision.len() == 40
            && revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "MCNF_BUILD_SOURCE_REVISION must be a full lowercase Git revision"
    );
    println!("cargo:rustc-env=MCNF_GUEST_SOURCE_REVISION={revision}");
}
