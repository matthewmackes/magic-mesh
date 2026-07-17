# EGUI Shell Visual Refinement Worklist

**Status:** ACTIVE  
**Parent worklist:** `docs/WORKLIST.md`  
**Scope:** Direct DRM/KMS egui shell, desktop windows, panels, dialogs, menus, cards, toolbars, controls, and interaction feedback.  
**Goal:** Replace the current block-heavy visual treatment with a coherent, professional surface system while preserving keyboard reachability, direct-rendering performance, and the existing application behavior.

## Governing design rules

- Do not solve blockiness by merely increasing every corner radius.
- Reduce unnecessary containers, outlines, separators, and permanently filled buttons.
- Establish hierarchy primarily through spacing, typography, surface contrast, alignment, and interaction state.
- Keep application state outside disposable immediate-mode widget builders.
- Custom-paint only shell-defining components or controls that cannot be composed cleanly from standard egui widgets.
- Preserve keyboard navigation, visible focus, readable contrast, and reduced-motion behavior.
- Validate all changes on the production DRM/KMS path, not only the windowed development path.

## Epic UI-VIS — Shell visual system and component refinement

### Foundation and audit

- [ ] **UI-VIS-101 — Inventory every visible shell surface and control family.** Catalog windows, title bars, panels, cards, groups, menus, dialogs, taskbar elements, launchers, tabs, buttons, toggles, sliders, text inputs, status indicators, tooltips, notifications, and custom-painted controls; record duplicate implementations and inconsistent styling.
- [ ] **UI-VIS-102 — Capture a visual baseline on the production DRM/KMS seat.** Save representative screenshots for the desktop, launcher, settings, networking, VM management, dialogs, menus, notifications, and dense data views at supported resolutions and scale factors.
- [ ] **UI-VIS-103 — Identify and remove redundant visual containers.** Flag nested frames, groups inside cards, repeated outlines, double separators, and surfaces that do not communicate a real hierarchy or interaction boundary.
- [ ] **UI-VIS-104 — Define a centralized shell design-token module.** Establish named tokens for backgrounds, elevated surfaces, overlays, borders, text levels, accent, semantic states, spacing, control heights, corner radii, strokes, shadows, icon sizes, and motion durations; prohibit scattered magic values in feature code.

### Global egui theme

- [ ] **UI-VIS-105 — Install one authoritative global egui style.** Configure `Style`, `Spacing`, `Visuals`, widget-state visuals, selection, menus, windows, text styles, interaction sizes, slider geometry, and popup appearance from the centralized token module.
- [ ] **UI-VIS-106 — Establish a restrained three-level surface hierarchy.** Provide application background, persistent surface, and elevated/floating surface levels with sufficient separation but without boxing every region.
- [ ] **UI-VIS-107 — Standardize spacing and density.** Use a documented spacing scale, increase cramped vertical rhythm, normalize internal margins, and set ordinary interactive controls to a consistent usable height while retaining a deliberate compact mode where density is necessary.
- [ ] **UI-VIS-108 — Standardize corner geometry.** Define small, medium, and large radii for controls, cards, menus, dialogs, and windows; do not allow arbitrary per-widget rounding.
- [ ] **UI-VIS-109 — Make inactive controls visually quiet.** Remove unnecessary inactive fills and borders from toolbar, title-bar, navigation, and icon actions; reserve stronger fills and strokes for hover, active, selected, focused, warning, and destructive states.
- [ ] **UI-VIS-110 — Define semantic color roles.** Separate accent, success, warning, error, information, disabled, focus, and selection colors; do not communicate state through color alone.

### Surfaces, windows, and layout

- [ ] **UI-VIS-111 — Replace default `ui.group()` styling with reusable surface primitives.** Implement shared card, inset, toolbar, section, dialog, and overlay helpers with controlled fill, stroke, margin, rounding, and optional shadow behavior.
- [ ] **UI-VIS-112 — Refine the application-window frame.** Treat title bar and content as one visual object, normalize active/inactive window states, reduce frame heaviness, use a subtle content separator, and ensure window controls have appropriate target sizes and quiet inactive treatment.
- [ ] **UI-VIS-113 — Rebuild title-bar controls as purpose-built shell widgets.** Provide consistent minimize, maximize/restore, close, menu, icon, title, drag region, and active-window indication behavior with hover, pressed, focus, and disabled states.
- [ ] **UI-VIS-114 — Replace box-based sectioning with typographic section hierarchy.** Use page titles, section headings, descriptions, metadata, whitespace, and alignment before adding a containing frame.
- [ ] **UI-VIS-115 — Normalize alignment and layout grids.** Align labels, values, icons, fields, actions, card edges, and baselines across related views; remove incidental one-off offsets.
- [ ] **UI-VIS-116 — Introduce responsive layout breakpoints.** Reflow toolbars, cards, forms, and side panels for narrow and wide displays without clipping, overlap, or excessive empty space.

### Typography and iconography

- [ ] **UI-VIS-117 — Establish a complete typography hierarchy.** Define page title, window title, section title, body, control label, metadata, caption, monospaced data, and emphasized-value styles with consistent sizes, weights, line heights, and wrapping behavior.
- [ ] **UI-VIS-118 — Audit font loading and fallback behavior.** Verify the intended fonts load on clean installations and that fallback glyphs, Unicode symbols, technical characters, and scaling remain legible on the DRM path.
- [ ] **UI-VIS-119 — Replace text glyphs and emoji used as interface icons.** Use one consistent vector or raster icon set with normalized optical size, stroke weight, alignment, active treatment, and accessibility labels.
- [ ] **UI-VIS-120 — Standardize icon-only interaction.** Require tooltips, accessible names, visible focus, adequate targets, predictable placement, and clear selected/active states for every icon-only action.

### Buttons and interactive controls

- [ ] **UI-VIS-121 — Define primary, secondary, quiet, toolbar, destructive, and icon-button components.** Limit primary emphasis to the dominant action in a region and prevent ad hoc button styling in feature views.
- [ ] **UI-VIS-122 — Refine text inputs, selectors, and editable fields.** Standardize labels, placeholders, focus rings, validation, disabled/read-only states, clear actions, padding, and error/help placement.
- [ ] **UI-VIS-123 — Refine toggles, checkboxes, radio controls, sliders, and progress indicators.** Normalize geometry, hit targets, animation, keyboard behavior, semantic state, and value presentation.
- [ ] **UI-VIS-124 — Refine tabs and navigation selection.** Replace heavy filled boxes where possible with restrained surface changes, accent indicators, typography, and motion while maintaining unmistakable selection.
- [ ] **UI-VIS-125 — Refine menus, context menus, tooltips, popovers, and combo boxes.** Normalize padding, row height, icon columns, shortcuts, separators, selected state, placement, clipping, and dismissal behavior.
- [ ] **UI-VIS-126 — Refine dialogs and destructive confirmations.** Establish predictable title, message, details, action, cancellation, default-focus, escape, and typed-confirmation patterns without unnecessary nested panels.
- [ ] **UI-VIS-127 — Refine status, notification, and alert components.** Use consistent severity hierarchy, iconography, wording, action placement, dismissal, persistence, and non-color indicators.

### Motion and interaction feedback

- [ ] **UI-VIS-128 — Create a centralized motion specification.** Define durations and easing for hover, press, selection, panel movement, popup/dialog appearance, window state changes, progress, and toggle motion.
- [ ] **UI-VIS-129 — Add restrained hover and press transitions.** Animate only properties that clarify interaction; avoid continuous decorative motion and avoid overriding egui state behavior with fixed per-widget fills.
- [ ] **UI-VIS-130 — Add coherent open, close, expand, collapse, and selection transitions.** Preserve spatial continuity for menus, sidebars, cards, dialogs, tabs, and shell overlays without delaying input.
- [ ] **UI-VIS-131 — Implement reduced-motion behavior.** Disable or shorten nonessential transitions while retaining immediate state feedback when reduced motion is selected or platform policy requests it.

### Accessibility and input quality

- [ ] **UI-VIS-132 — Standardize visible keyboard focus.** Ensure every reachable control has a consistent high-contrast focus treatment distinct from hover and selection.
- [ ] **UI-VIS-133 — Verify keyboard operation after every visual refactor.** Preserve tab order, arrow navigation, activation, escape behavior, shortcuts, menu traversal, and focus restoration.
- [ ] **UI-VIS-134 — Validate contrast and non-color communication.** Test text, icons, focus, disabled content, selection, semantic states, and overlays against their actual rendered backgrounds.
- [ ] **UI-VIS-135 — Enforce minimum interaction targets.** Ensure title-bar controls, taskbar items, icon actions, toggles, resize handles, and compact controls remain reliably clickable and touch-capable where required.
- [ ] **UI-VIS-136 — Validate scale-factor and resolution behavior.** Test supported scale factors and representative small, standard, ultrawide, and high-DPI displays for legibility, clipping, alignment, and target size.

### Migration, quality, and performance

- [ ] **UI-VIS-137 — Migrate shell-defining surfaces first.** Apply the new system to the desktop, window frame, title bar, taskbar, launcher, navigation, dialogs, menus, and notifications before polishing secondary application views.
- [ ] **UI-VIS-138 — Migrate feature workspaces to shared components.** Remove duplicated local card, button, field, menu, status, and layout implementations as each workspace adopts the central component set.
- [ ] **UI-VIS-139 — Add visual regression coverage.** Create deterministic captures or snapshot tests for core components and representative shell states, including inactive, hovered, active, selected, focused, disabled, warning, error, and reduced-motion variants.
- [ ] **UI-VIS-140 — Add design-token and component-use checks.** Detect prohibited magic colors, arbitrary radii, direct feature-level widget styling, obsolete helpers, and reintroduction of retired boxed layouts where practical.
- [ ] **UI-VIS-141 — Profile repaint and tessellation behavior.** Measure frame time, repaint requests, shape counts, text layout, allocations, texture use, and animation cost on representative low-end hardware and the production DRM path.
- [ ] **UI-VIS-142 — Prevent idle repaint loops.** Confirm static screens sleep correctly and animations request repaint only while active; do not trade visual polish for continuous unnecessary rendering.
- [ ] **UI-VIS-143 — Bound visual effects for low-end systems.** Keep shadows, translucency, blur substitutes, gradients, large overlays, and animated geometry within documented performance budgets and provide graceful reductions when needed.
- [ ] **UI-VIS-144 — Remove superseded styling code.** Delete legacy constants, duplicate helpers, dead themes, obsolete component variants, and feature-specific workarounds after migration.
- [ ] **UI-VIS-145 — Perform a final cross-workspace visual audit.** Verify consistent hierarchy, density, typography, surfaces, controls, state treatment, motion, accessibility, and performance across the entire shell.

## Required implementation order

1. Complete the inventory, production screenshots, and duplicate-style audit.
2. Establish design tokens, semantic colors, typography, spacing, radii, and motion specifications.
3. Install the authoritative global egui theme and shared surface/control primitives.
4. Refine the window frame, title bar, desktop, taskbar, launcher, navigation, dialogs, menus, and notifications.
5. Migrate feature workspaces and delete duplicate styling code as each migration lands.
6. Add accessibility, scale-factor, reduced-motion, visual-regression, and component-policy coverage.
7. Profile and tune repaint, tessellation, text, allocation, texture, and animation behavior on the DRM path.
8. Complete the live-seat visual audit and remove all remaining legacy styling paths.

## Definition of done

This worklist is complete only when:

- the shell no longer relies on repeated bordered rectangles to create hierarchy;
- all shell-defining and feature surfaces use the centralized token and component system;
- title bars, windows, taskbar, launcher, navigation, menus, dialogs, notifications, forms, and controls have coherent inactive, hover, active, selected, focused, disabled, and semantic states;
- typography and spacing create clear hierarchy without unnecessary containers;
- all icon-only controls have consistent artwork, tooltips, accessible names, focus, and adequate targets;
- keyboard navigation and focus behavior pass regression testing across every migrated surface;
- reduced-motion and supported scale-factor behavior are verified;
- visual-regression coverage protects the core component states and representative shell views;
- static screens do not continuously repaint and animations remain within the established production performance budget;
- live DRM/KMS seat review confirms the shell looks coherent and professional at representative resolutions;
- legacy styling constants, duplicate components, obsolete groups, and superseded theme paths are removed.