# Platform release versioning

The next production release is `13.0.0`, published as `magic-mesh-v13.0.0`.
It targets Fedora 44 on x86_64. ARM64 is not part of this release envelope.

This document defines the release identity contract for Magic Mesh. There is
one numeric release source: the `version` field under `[workspace.package]` in
the repository-root `Cargo.toml`.

Do not create a second version constant in a shell helper, Rust crate, RPM
recipe, welcome message, GUI, or documentation. A label such as a codename or
channel is presentation metadata; it is not an alternate release number.

## The source of truth

`Cargo.toml` → `[workspace.package]` → `version` is the authoritative platform
release version. Workspace crates that ship with the platform inherit it with
`version.workspace = true`. The three intentionally isolated browser helper
workspaces cannot inherit across Cargo workspace roots; their package versions
are synchronized to this value and their lockfiles carry the resulting package
metadata. The value is a SemVer-compatible release identity; it is the value
that must remain consistent across builds and installations.

The RPM may add packaging metadata after that version:

```text
<Cargo workspace version>-<RPM release>.<architecture>
```

The RPM `VERSION` must equal the Cargo workspace version. The RPM `RELEASE` is
only the packaging iteration and must not be used as a substitute for the
platform version.

## Surface contract

| Surface | Required source or reflection | Verification |
| --- | --- | --- |
| Cargo/workspace | Root `Cargo.toml` `[workspace.package].version` | `cargo metadata --no-deps --format-version 1` |
| Rust build identity | `CARGO_PKG_VERSION`, exposed by `mde-theme::brand::build` | `mackesd --version` and `mde-shell-egui --version` |
| About page | The shared `mde-theme` build identity, including its version line | Open About and compare its Version/build fields with the CLI |
| Watermark and splash | The shared `mde-theme` release identity | Render the shell and compare the visible release line with About |
| RPM | Cargo package metadata emitted by the RPM build | `rpm -qp --qf '%{VERSION}-%{RELEASE}.%{ARCH}\n' <candidate.rpm>` |
| Bash welcome | The installed node's release snapshot/package metadata | Compare the welcome version with `rpm -q --qf '%{VERSION}\n' magic-mesh` |
| `mesh-help` | Installed RPM metadata; no embedded version literal | The Platform release line and `rpm -q magic-mesh` agree |
| Isolated browser helpers | Root release, mirrored only because Cargo uses separate workspace roots | Compare their manifest versions to the root before the browser RPM cut |

The CLI, About, watermark, and splash must not hand-format a numeric version.
They consume the shared build identity so a single Cargo bump updates all of
those compiled surfaces. The welcome path is an installed-node reflection: its
reported release must come from the installed package/status metadata and must
match the RPM `VERSION`. `mesh-help` explicitly queries the installed RPM and
prints the full package identity (`VERSION-RELEASE.ARCH`) without knowing a
particular release number.

### Intentional non-release package boundaries

The following workspace packages are internal libraries or test-only helpers,
not independently published release roles: `mde-kdc-host`, `mde-kdc-proto`,
`magic-fleet`, and `mackes-transport` retain `0.0.0`; the isolated Maps
verifier retains `0.0.0`. They are included in the source workspace for
dependency and test resolution, but no standalone artifact, RPM, image, or
runtime version surface is produced for them. The shipped `mde-role-chooser`
and all isolated browser helper workspaces inherit the platform version.

## Bump procedure

1. Edit only the `version` value in the root `Cargo.toml` workspace package
   section for the platform release bump. Do not copy that number into runtime
   code or helper scripts.
2. Confirm workspace inheritance:

   ```bash
   cargo metadata --no-deps --format-version 1
   ```

   Every shipped workspace package should report the workspace release unless
   it is intentionally excluded and has its own documented release boundary.
   For the isolated browser helpers, update their manifest/lockfile package
   metadata to the same root value; those copies are Cargo packaging metadata,
   not alternate platform release authorities.
3. Run the required farm build/test gates from the repository's build
   environment instructions. Include the release RPM lane; the candidate RPM
   is the package-level proof of the release.
4. Inspect the candidate RPM before promotion:

   ```bash
   rpm -qp --qf '%{VERSION}\n' <candidate.rpm>
   rpm -qp --qf '%{VERSION}-%{RELEASE}.%{ARCH}\n' <candidate.rpm>
   ```

   The first command must equal the workspace version. The second command is
   the exact artifact identity to record for promotion and rollback.
5. After installation, verify the installed reflection surfaces:

   ```bash
   rpm -q --qf '%{VERSION}-%{RELEASE}.%{ARCH}\n' magic-mesh
   mackesd --version
   mde-shell-egui --version
   mesh-help
   ```

   Confirm that the CLI, About page, watermark/splash, welcome message, and
   `mesh-help` all identify the same platform release. A mismatch is a release
   blocker, not a cosmetic documentation issue.
6. Record the candidate's RPM NEVRA, checksum, build evidence, and promotion
   result in the existing release evidence/worklist system. Do not add a
   second active version ledger.

## Review guardrails

- Search for newly introduced numeric release literals outside the workspace
  package version before merging.
- Keep package release iterations separate from platform version bumps.
- Preserve historical artifact names and evidence when they describe a past
  build; historical records are not runtime release sources.
- Validate the installed RPM, not only a source checkout: source-only checks
  cannot prove what the welcome script, CLI, or package metadata will show on a
  seat.
