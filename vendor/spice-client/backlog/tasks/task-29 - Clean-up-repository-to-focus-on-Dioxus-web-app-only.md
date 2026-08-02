---
id: task-29
title: Clean up repository to focus on Dioxus web app only
status: Done
assignee:
  - '@claude'
created_date: '2025-09-29 22:53'
updated_date: '2025-09-29 22:57'
labels:
  - cleanup
  - refactor
dependencies: []
priority: high
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Remove unused frontend implementations (GTK4, Slint, Leptos) and update build configuration since the project is consolidating to Dioxus web app only
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Remove gtk4-app directory and dependencies
- [x] #2 Remove slint-app directory and dependencies
- [x] #3 Remove leptos-web directory and dependencies
- [x] #4 Update Cargo.workspace to remove old frontends
- [x] #5 Update justfile to remove old build targets
- [x] #6 Update CLAUDE.md to reflect new architecture
- [x] #7 Update README if present to reflect Dioxus-only approach
<!-- AC:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Survey current directory structure and dependencies
2. Remove gtk4-app directory
3. Remove slint-app directory
4. Remove leptos-web directory
5. Update Cargo.toml workspace configuration
6. Update justfile to remove old targets
7. Update CLAUDE.md documentation
8. Check for and update README if present
9. Verify build still works
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cleaned up repository to focus on Dioxus web application:

- Removed gtk4-app, slint-app, and leptos-web directories
- Updated Cargo.toml workspace to only include core, spice-client, and dioxus-web
- Updated justfile with new web-focused build commands (build-web, serve-web, run-web-verbose)
- Removed GTK4 and Slint specific commands from justfile
- Updated CLAUDE.md to reflect new architecture and web-only approach
- Updated README.md with web-focused Getting Started, simplified dependencies, and updated project structure

Core library and SPICE client build successfully. Dioxus-web has some missing dependencies that will need to be resolved in a separate task.
<!-- SECTION:NOTES:END -->
