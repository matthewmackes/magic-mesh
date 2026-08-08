# WL-FUNC-019 — hostile resource-catalog wire boundary

Status: focused contract proof complete; live multi-source publication and
Dell runtime acceptance remain `Remaining`.

## Change

`crates/mesh/mackes-mesh-types/src/resources.rs` now has a focused catalog
wire-boundary test covering:

- distinct discovery provenance sources on one resource card;
- duplicate resource-card identity rejection;
- malformed provenance scope/trust rejection; and
- unknown resource-kind rejection.

The test confirms that multiple valid observations remain represented while
identity collisions and hostile or future wire values fail closed.

## Verification

Farm `.90` completed the focused Rust test in slot
`resource-catalog-hostile-20260806-r1`:

```text
1 passed, 0 failed
```

`git diff --check -- crates/mesh/mackes-mesh-types/src/resources.rs`: pass.

No live publisher, Dell runtime, or external resource state was changed.

## Source hash at capture

```text
7414e53a6f549cb4695642056de8fbf6a0c4953a91a1c90aa819dd940d245e3f  crates/mesh/mackes-mesh-types/src/resources.rs
```
