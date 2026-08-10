# WL-FUNC-016 — bounded RDP CF_HTML transport (r125)

Date: 2026-08-10

Base revision: `a5e9fd54`

## Result

The production IronRDP CLIPRDR path now preserves Unicode text and additionally
negotiates Windows' registered `HTML Format`. It encodes and validates CF_HTML
byte offsets, bounds payloads to 1 MiB and remote format lists to 256 entries,
and exposes production send/take HTML methods.

Malformed, oversized, unsupported, unsolicited, and stale replacement
callbacks fail closed. A local offer replacement is generation-bound so an
older guest request cannot receive newer data under the old format.

## Focused farm proof

Build VM `.50` (`172.20.0.50`) passed four focused exact tests, each with
`1 passed; 0 failed; 96 filtered out`:

```text
rich_html_negotiation_round_trips_registered_cf_html
rich_html_admission_is_bounded_in_both_directions
replacement_and_unsupported_formats_refuse_stale_callbacks
bridge_bounds_host_text_and_decodes_guest_unicode
```

Formatting and `git diff --check` passed.

Source SHA-256:

- `73962e13159739b2812336c2f109fa6d6ec56ee64bc4696261cf620cbfc8206d`
  — `crates/desktop/mde-vdi-rdp/src/clipboard.rs`
- `30a76cc6b1334e6ebb65a20f152bd92155ab9b300433f80539f9688cb7a4d512`
  — `crates/desktop/mde-vdi-rdp/src/connect.rs`

The shell materialization layer still admits only plain text, and live Windows
CF_HTML interoperability remains open. This checkpoint does not claim the
end-to-end rich-clipboard epic complete.
