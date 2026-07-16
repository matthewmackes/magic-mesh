# SEARCH-omnibox — unified launcher/search foundation

`SEARCH-omnibox` is the remaining launcher-restructure epic: one fast entry point
for apps, files, mesh units, Browser history/bookmarks/search suggestions, and
assistant-ranked follow-up rows. `mde_egui::search_omnibox` owns a pure
ranked-result model; the Start Menu, Browser, Explorer, and Files adapters use it
without taking over their local dispatch paths.

## Locks

1. **One scorer, many adapters.** Start Menu, Browser omnibox, Explorer unit
   search, Files recursive search, and future Front Door UI should feed candidates
   into the shared model instead of cloning ranking rules.
2. **Typed exactness wins.** Ranking tiers are title prefix, target/URL prefix,
   title substring, auxiliary substring, then fuzzy title. A future AI/local model
   score may break ties inside a tier, but must not bury a direct typed match.
3. **Action dispatch stays local.** The model carries a typed payload; callers keep
   their existing action seams (`Surface` activation, Browser `submit_address`,
   Files navigation/search, Explorer `jump_to_id`). The shared layer does not
   duplicate command behavior.
4. **No fake indexing.** File candidates come from the Files model/search worker or
   a real indexer when that exists. Mesh candidates come from discovered units.
   Browser candidates come from the existing bookmark/history/suggestion stores.
5. **Incremental reachability.** Every slice must be used by a runtime path or by
   an adapter with an existing dispatch path. Pure helpers are allowed only when a
   live caller lands with the same slice.

## Current Slice

- Added `crates/shared/mde-egui/src/search_omnibox.rs` with `SearchDomain`,
  `SearchItem<T>`, `SearchHit<T>`, `MatchTier`, and `ranked_hits`. The model
  lives in the shared egui harness so independent surfaces can feed the same
  scorer without depending on the shell crate.
- Rewired `start_menu::search_matches` to build app candidates from
  `Surface::ALL` and `TILE_GROUPS`, preserving the existing Start-menu behavior
  through the shared ranker.
- Rewired Browser bookmark autocomplete through the shared ranker and added
  `SuggestionState::ordered_search_items` so bookmark, history, and SearXNG rows
  expose `BrowserBookmark`/`BrowserHistory`/`WebSuggestion` candidates while
  preserving the existing render order and `submit_address` commit payloads.
- Rewired Explorer's `/` unit search through shared `Mesh` candidates. Each real
  unit field (name, address, type taxonomy, LAN id/MAC, hosting node, service
  labels, and enrichment text) becomes a `SearchItem<usize>` payload that still
  jumps through Explorer's existing `jump_to_id` path.
- Added the Files adapter: `FileBrowser::search_omnibox_items(pane)` projects the
  active tab's current rows into `File` candidates with a typed
  `FileSearchTarget`. Current folders and streamed recursive-search result sets
  both flow through the same row model, and activation forwards to the existing
  `open_row` path.
- Covered all planned source domains in unit tests: apps, files, mesh, Browser
  bookmarks/history, web suggestions, and assistant rows.

## Next Slices

1. Front Door/full omnibox UI: one shell-owned focused entry field that can show
   instant local hits first and append slower assistant/model rows as they arrive.
2. Optional real file indexer: feed the same `File` candidate model from an
   explicit background worker when that exists; do not add in-UI filesystem crawls.
