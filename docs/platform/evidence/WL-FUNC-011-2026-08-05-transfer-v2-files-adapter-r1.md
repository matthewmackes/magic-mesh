# WL-FUNC-011 — TransferJob V2 Files projection (2026-08-05)

This slice makes the strict `TransferJobV2` contract reachable from the existing
Files-side read model without creating a second ledger. The adapter validates the
V2 job, admits only executor kinds and operations that have a lossless legacy
projection, and requires a typed `FileRefId` on the direction-facing side.

The projection deliberately drops endpoint/profile/resource details, operation
options, checksum policy, attempts, and typed errors because the legacy
`TransferJobView` cannot represent them. Paths, URLs, commands, and credentials
are not accepted by the adapter. Unsupported kinds, invalid schemas, and missing
typed file references fail closed.

## Verification

- Farm `.90`: focused adapter tests passed, `5 passed; 0 failed; 52 filtered out`.
- Farm `.50`: Rust formatting check passed for the changed adapter/export files.
- The change is an adapter slice only; it does not claim full daemon lane parity,
  live transfer execution, or completion of WL-FUNC-011.
