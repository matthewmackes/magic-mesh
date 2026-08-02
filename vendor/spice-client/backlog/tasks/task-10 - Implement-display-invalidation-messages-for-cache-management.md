---
id: task-10
title: Implement display invalidation messages for cache management
status: Done
assignee:
  - '@claude'
created_date: '2025-09-29 00:33'
updated_date: '2025-09-30 00:48'
labels:
  - spice-client
  - protocol
  - medium-priority
  - display
dependencies: []
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Display invalidation messages (INVAL_LIST, INVAL_PALETTE, INVAL_ALL_PALETTES) are used to manage client-side caching of display resources. Proper implementation improves performance by avoiding unnecessary redraws.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Implement SPICE_MSG_DISPLAY_INVAL_LIST handler to invalidate specific regions
- [x] #2 Implement SPICE_MSG_DISPLAY_INVAL_PALETTE handler for palette cache invalidation
- [x] #3 Implement SPICE_MSG_DISPLAY_INVAL_ALL_PALETTES handler
- [x] #4 Add cache invalidation logic to display channel
- [x] #5 Add tests for invalidation scenarios
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Review existing display channel code and cache structures
2. Implement SPICE_MSG_DISPLAY_INVAL_LIST handler for region invalidation
3. Implement SPICE_MSG_DISPLAY_INVAL_PALETTE handler for palette cache
4. Add tests for invalidation scenarios
5. Test with real VMs to verify cache invalidation works correctly
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented display invalidation message handlers for SPICE protocol cache management:

- Added ImageCache methods: remove(), clear(), and len() for cache management operations
- Implemented SPICE_MSG_DISPLAY_INVAL_LIST handler to parse and process invalidation lists (u16 count + array of u64 resource IDs)
- Enhanced SPICE_MSG_DISPLAY_INVAL_ALL_PIXMAPS handler to clear entire image cache
- INVAL_PALETTE and INVAL_ALL_PALETTES handlers were already in place (acknowledge as no-ops since we convert palettes to RGBA)

Added comprehensive test coverage:
- test_image_cache_operations: Validates basic cache CRUD operations
- test_inval_list_message_parsing: Tests protocol message format parsing
- test_cache_invalidation_logic: Verifies selective cache invalidation
- test_inval_list_with_nonexistent_ids: Handles gracefully when IDs don't exist
- test_inval_all_pixmaps: Tests full cache clearing
- test_empty_inval_list: Edge case with zero invalidations

All tests pass (11/11 in display module).

Files modified:
- spice-client/src/channels/display.rs: Added cache management and invalidation handlers
<!-- SECTION:NOTES:END -->
