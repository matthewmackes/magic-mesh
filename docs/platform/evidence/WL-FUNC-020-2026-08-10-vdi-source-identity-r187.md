# WL-FUNC-020 Android VDI source identity admission — r187

- Revision: `e8c97e1a`
- Scope: the Cuttlefish adapter no longer exposes a provider VDI source unless the retained observation is guest-ready for the requested generation and the source is valid and bound to the adapter workload and image provenance.
- Hostile regression: a validly shaped source for a different workload is refused without being exposed to Workloads/VDI consumers.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func020-vdi-source-identity-r187b install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::cloud::verbs::android::cuttlefish::tests::adapter_refuses_vdi_source_with_mismatched_workload_identity -- --nocapture`
- Result: `1 passed; 0 failed; 0 ignored; 0 measured; 4723 filtered out` on `.90`.
- Live limits: no nested-KVM Cuttlefish boot, physical VDI presentation, guest packaging, or three-seat acceptance was performed.
