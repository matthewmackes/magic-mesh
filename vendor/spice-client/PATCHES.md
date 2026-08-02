# Local patches

This is the `spice-client` 0.2.0 source published by the Quickemu Manager
project, retained locally because the live Browser VM transport needs bounded
behavior before an upstream release is available.

The local fork:

- uses saturating rectangle subtraction in display copy paths so malformed or
  partially clipped draw commands cannot panic the receiver;
- does not start the receive loop for the write-only inputs channel, allowing
  keyboard and pointer writes to acquire its mutex while the display loop is
  active.

The Browser VDI adapter separately snapshots decoded display updates through
the upstream callback API so polling never waits behind the display receive
mutex. The live Dell gate still rejects the current upstream wire-layout
decode when it produces a flat frame; this fork does not claim that decoder is
production-ready.
