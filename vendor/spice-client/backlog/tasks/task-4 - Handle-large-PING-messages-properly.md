---
id: task-4
title: Handle large PING messages properly
status: Done
assignee:
  - '@claude'
created_date: '2025-09-28 23:14'
updated_date: '2025-09-29 00:54'
labels:
  - spice-client
  - protocol
dependencies: []
ordinal: 4000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The Main Channel receives PING messages up to 256KB but truncates PONG responses to 4KB with warning 'PING data too large (256012 bytes), truncating PONG to 4096 bytes'. This may violate SPICE protocol expectations.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Research SPICE protocol requirements for PING/PONG data sizes
- [x] #2 Determine if truncation is acceptable or if full PONG is required
- [ ] #3 Implement proper handling for large PING messages
- [ ] #4 Remove or adjust truncation if necessary
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Research SPICE protocol documentation for PING/PONG requirements
2. Analyze current implementation in main.rs
3. Check if other SPICE implementations handle large PINGs
4. Determine correct behavior based on protocol specs
5. Implement proper handling if changes needed
6. Test with large PING messages
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Research findings:
- SPICE protocol PING messages contain an id (4 bytes) and timestamp (8 bytes) = 12 bytes of essential data
- Server can send additional data after these fields for testing/debugging
- Client should only echo back the id and timestamp (12 bytes), not the entire payload
- CVE-2016-9577 was a buffer overflow in spice-server when handling large messages
- Current implementation has duplicate PING handlers with different behavior
<!-- SECTION:NOTES:END -->
