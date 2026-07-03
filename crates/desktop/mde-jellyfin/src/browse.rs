//! Browse-tree folds — turning the flat `/Items`-shaped fetches into the nested
//! shapes a browse UI renders.
//!
//! The client returns flat [`ItemsResponse`](crate::ItemsResponse) pages; these
//! pure folds assemble them (a series' seasons + episodes into a [`ShowTree`],
//! a mixed listing grouped by kind) so the folds themselves are unit-testable
//! with no network.

use std::collections::BTreeMap;

use crate::models::BaseItemDto;

/// One season within a [`ShowTree`]: the season item plus its ordered episodes.
#[derive(Debug, Clone, PartialEq)]
pub struct SeasonNode {
    /// The season item.
    pub season: BaseItemDto,
    /// The season's episodes, ordered by episode number.
    pub episodes: Vec<BaseItemDto>,
}

/// A fully-assembled show: the series plus its ordered seasons, each with its
/// episodes — the browse tree `shows → seasons → episodes`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShowTree {
    /// The series item.
    pub series: BaseItemDto,
    /// The series' seasons, ordered by season number.
    pub seasons: Vec<SeasonNode>,
}

/// Fold a flat `series + seasons + episodes` fetch into a [`ShowTree`].
///
/// Each episode is attached to the season it belongs to (matched by
/// [`season_id`](BaseItemDto::season_id), falling back to
/// [`parent_id`](BaseItemDto::parent_id)); seasons are ordered by season number
/// ([`index_number`](BaseItemDto::index_number)) and episodes within a season by
/// episode number. Episodes that match no season are dropped (a well-formed
/// Jellyfin response always sets the season id).
#[must_use]
pub fn build_show_tree(
    series: BaseItemDto,
    mut seasons: Vec<BaseItemDto>,
    episodes: Vec<BaseItemDto>,
) -> ShowTree {
    // Group episodes by their owning season id in one pass.
    let mut by_season: BTreeMap<String, Vec<BaseItemDto>> = BTreeMap::new();
    for episode in episodes {
        if let Some(season_id) = episode
            .season_id
            .clone()
            .or_else(|| episode.parent_id.clone())
        {
            by_season.entry(season_id).or_default().push(episode);
        }
    }

    // Seasons in season-number order.
    seasons.sort_by_key(|s| s.index_number.unwrap_or(i32::MAX));

    let nodes = seasons
        .into_iter()
        .map(|season| {
            let mut episodes = by_season.remove(&season.id).unwrap_or_default();
            episodes.sort_by_key(|e| e.index_number.unwrap_or(i32::MAX));
            SeasonNode { season, episodes }
        })
        .collect();

    ShowTree {
        series,
        seasons: nodes,
    }
}

/// Group a mixed item listing by its [`item_type`](BaseItemDto::item_type) —
/// e.g. splitting a search result or a library view into Movies / Series /
/// `BoxSet`s. Items with no type land under `""`.
#[must_use]
pub fn group_by_type(items: &[BaseItemDto]) -> BTreeMap<&str, Vec<&BaseItemDto>> {
    let mut grouped: BTreeMap<&str, Vec<&BaseItemDto>> = BTreeMap::new();
    for item in items {
        let key = item.item_type.as_deref().unwrap_or("");
        grouped.entry(key).or_default().push(item);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series() -> BaseItemDto {
        BaseItemDto {
            id: "series-1".into(),
            name: Some("The Show".into()),
            item_type: Some("Series".into()),
            ..BaseItemDto::default()
        }
    }

    fn season(id: &str, number: i32) -> BaseItemDto {
        BaseItemDto {
            id: id.into(),
            name: Some(format!("Season {number}")),
            item_type: Some("Season".into()),
            index_number: Some(number),
            series_id: Some("series-1".into()),
            ..BaseItemDto::default()
        }
    }

    fn episode(id: &str, season_id: &str, number: i32) -> BaseItemDto {
        BaseItemDto {
            id: id.into(),
            name: Some(format!("Episode {number}")),
            item_type: Some("Episode".into()),
            index_number: Some(number),
            season_id: Some(season_id.into()),
            series_id: Some("series-1".into()),
            ..BaseItemDto::default()
        }
    }

    #[test]
    fn folds_seasons_and_episodes_in_order() {
        // Deliberately out of order in, sorted out.
        let seasons = vec![season("s2", 2), season("s1", 1)];
        let episodes = vec![
            episode("e1b", "s1", 2),
            episode("e2a", "s2", 1),
            episode("e1a", "s1", 1),
        ];
        let tree = build_show_tree(series(), seasons, episodes);

        assert_eq!(tree.series.id, "series-1");
        assert_eq!(tree.seasons.len(), 2);

        // Season 1 first, its episodes ordered 1 then 2.
        assert_eq!(tree.seasons[0].season.id, "s1");
        let ep_ids: Vec<&str> = tree.seasons[0]
            .episodes
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(ep_ids, vec!["e1a", "e1b"]);

        // Season 2 second, one episode.
        assert_eq!(tree.seasons[1].season.id, "s2");
        assert_eq!(tree.seasons[1].episodes.len(), 1);
        assert_eq!(tree.seasons[1].episodes[0].id, "e2a");
    }

    #[test]
    fn episode_without_season_id_falls_back_to_parent_id() {
        let mut ep = episode("e1", "ignored", 1);
        ep.season_id = None;
        ep.parent_id = Some("s1".into());
        let tree = build_show_tree(series(), vec![season("s1", 1)], vec![ep]);
        assert_eq!(tree.seasons[0].episodes.len(), 1);
        assert_eq!(tree.seasons[0].episodes[0].id, "e1");
    }

    #[test]
    fn unmatched_episode_is_dropped() {
        // Episode points at a season that isn't in the season list.
        let tree = build_show_tree(series(), vec![season("s1", 1)], vec![episode("x", "s9", 1)]);
        assert!(tree.seasons[0].episodes.is_empty());
    }

    #[test]
    fn empty_series_folds_to_no_seasons() {
        let tree = build_show_tree(series(), vec![], vec![]);
        assert!(tree.seasons.is_empty());
    }

    #[test]
    fn group_by_type_partitions_a_mixed_listing() {
        let items = vec![
            BaseItemDto {
                id: "m1".into(),
                item_type: Some("Movie".into()),
                ..BaseItemDto::default()
            },
            BaseItemDto {
                id: "b1".into(),
                item_type: Some("BoxSet".into()),
                ..BaseItemDto::default()
            },
            BaseItemDto {
                id: "m2".into(),
                item_type: Some("Movie".into()),
                ..BaseItemDto::default()
            },
        ];
        let grouped = group_by_type(&items);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped["Movie"].len(), 2);
        assert_eq!(grouped["BoxSet"].len(), 1);
    }
}
