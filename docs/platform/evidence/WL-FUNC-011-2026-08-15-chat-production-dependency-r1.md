# WL-FUNC-011 Chat production dependency hard-cut r1

Date: 2026-08-15

The shell's legacy `mde-chat` module and `ChatState` are now compiled only for
tests. Production shell code uses the new `calendar` module for the shared
date conversion, and `mde-chat` is a dev-dependency only. The production shell
therefore has no compiled dependency on the retired Chat model; the legacy
model remains available to its compatibility tests and to the independent
chat worker/files/terminal integrations that still own those contracts.

Farm validation:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=collab-chat-dep-farm27 \
  install-helpers/xcp-build.sh cargo check -p mde-shell-egui --tests
```

Result: the shell package and its test configuration checked successfully.
