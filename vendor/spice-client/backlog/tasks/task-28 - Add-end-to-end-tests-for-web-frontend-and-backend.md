---
id: task-28
title: Add end-to-end tests for web frontend and backend
status: To Do
assignee: []
created_date: '2025-09-29 22:45'
labels:
  - testing
  - e2e
dependencies: []
priority: low
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Create comprehensive end-to-end tests that verify the full stack: browser frontend → backend API → quickemu-core → QEMU. Use Playwright for browser automation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 E2E test infrastructure set up with Playwright
- [ ] #2 Test: List VMs from frontend
- [ ] #3 Test: Start VM from frontend and verify connection
- [ ] #4 Test: SPICE display renders and accepts input
- [ ] #5 Test: Multiple concurrent VM connections work
- [ ] #6 Test: Error handling and recovery scenarios
- [ ] #7 Tests integrated into CI pipeline
<!-- AC:END -->
