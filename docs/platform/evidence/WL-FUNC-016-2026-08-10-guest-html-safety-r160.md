# WL-FUNC-016 guest HTML safety — r160

- Revision: `4e865027`
- Scope: guest CF_HTML fails closed on executable tags, event-handler attributes, and `javascript:`/`vbscript:` URLs while preserving ordinary formatting.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func016-cfhtml-safety-r160b install-helpers/xcp-build.sh cargo test -p mde-vdi-rdp --features live-connect --lib clipboard::tests::guest_html_active_content_is_refused_before_host_publication -- --nocapture`
- Result: `1 passed; 0 failed; 98 filtered out` on seat 90.

