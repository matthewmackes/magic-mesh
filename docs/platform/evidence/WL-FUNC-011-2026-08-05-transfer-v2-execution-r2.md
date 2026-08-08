# WL-FUNC-011 — Transfer V2 claim and Files execution boundary (2026-08-05)

The live transfer worker now durably admits and claims queued V2 jobs, resolves
typed Local/Mesh endpoints through an injected `Send + Sync` Files authority,
and records typed terminal outcomes. Opaque object/node identities are never
parsed as paths, URLs, or commands. The generic authority seam revalidates exact
identity, generation, root/path containment, type, size, hash, and access before
a symlink-safe streamed copy and atomic destination replacement.

Integration review found that the production collaboration resolver exposes a
content-addressed read store but no destination mutation transaction. Replacing
an existing `<sha256>` path would corrupt that store and leave `FileRef`
metadata false. Production destination resolution therefore returns typed
`MutationUnsupported`; the job terminates Unsupported with zero bytes/checksum
and modifies no content file. No unsafe fallback was added.

## Verification

- BigBoy `.130`, slot `wl-func011-files-resolution-test-r3`:
  `cargo test -p mackesd --lib 'workers::transfers::' -- --nocapture`.
- Result: `90 passed; 0 failed; 4371 filtered out`.
- The command compiled the complete `mackesd` library target; scoped
  `git diff --check` passed.

## Remaining acceptance edge

Files must expose an authority-owned stage/commit operation that stores bytes
under the observed source hash and atomically updates the destination `FileRef`
generation/metadata. Production Local/Mesh commits remain honestly unsupported
until that API exists. V2 rsync, SFTP, HTTP, scrape, multipart, recurring mirror,
and Clipboard executors also remain open.
