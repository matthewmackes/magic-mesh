# WL-FUNC-011 CAS stream staging evidence — 2026-08-11

- Scope: Files ingestion has a production-used filesystem staging primitive
  that streams a declared bounded length while hashing, fsyncs a private
  create-new stage, and commits by hard-linking into the content-addressed path
  without replacing an existing object.
- Failure behavior: short, oversized, or digest-mismatched streams fail before
  publication. Rollback removes only the inode owned by the staging operation,
  and drop cleans an uncommitted stage.
- Production path: the running `collab` worker refuses `LinkFile` metadata until
  the exact canonical payload verifies, then `ingest_and_register_file` stages
  exact bytes, installs CAS
  through an owned commit token, applies the authenticated `LinkFile` command,
  confirms the exact durable projection, and retains bytes only after commit.
  Authorization/projection failure rolls back only a blob installed by that
  operation; exact pre-existing blobs remain intact.
- Farm gates: BigBoy `.130`, slot 2: blob suite **10 passed**, ingest/register
  transaction **4 passed**, and exact worker admission **1 passed**, with zero
  failures. The worker binary compiled through the production caller.
- Remaining boundary: the RDP guest-image bridge still truthfully reports
  provider unavailable until its authenticated caller supplies Files authority.
