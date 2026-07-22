//! Offline geocoder — FTS5 gazetteer reader behind the `ProviderContract`
//! geocoder seam (P1).
//!
//! The seat ships a bundled gazetteer at
//! `<client_data_dir>/maps/<region>/gazetteer.sqlite` (see [`crate::basemap`]);
//! this module runs a prefix search over its FTS5 `places_fts` index and returns
//! ranked name/street/city + `lat`/`lon` results, so the destination-search bar
//! turns typed text into real map pins with no network and no Nominatim daemon.
//!
//! Fail-soft everywhere: a seat with no gazetteer installed (or an unreadable
//! DB) returns an empty result set plus a human note — never an error, never a
//! panic.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags};

/// One gazetteer hit. Text columns are always present (empty, never `NULL`), so
/// the UI can compose a title/subtitle without option juggling.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoResult {
    /// Place / POI name (may be empty for a bare address row).
    pub name: String,
    /// House number, when the row is a street address.
    pub housenumber: String,
    /// Street name.
    pub street: String,
    /// City / locality.
    pub city: String,
    /// Latitude (WGS84).
    pub lat: f64,
    /// Longitude (WGS84).
    pub lon: f64,
    /// OSM-derived kind (`street`, `poi`, `address`, `place:town`, …).
    pub kind: String,
}

impl GeoResult {
    /// The `"{housenumber} {street}"` line, or just the street, or empty.
    fn street_line(&self) -> String {
        let hn = self.housenumber.trim();
        let st = self.street.trim();
        match (hn.is_empty(), st.is_empty()) {
            (false, false) => format!("{hn} {st}"),
            (_, false) => st.to_string(),
            _ => String::new(),
        }
    }

    /// The primary label for a result row: the place name, else its street line,
    /// else the city, else a safe placeholder.
    #[must_use]
    pub fn title(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
        let street = self.street_line();
        if !street.is_empty() {
            return street;
        }
        let city = self.city.trim();
        if !city.is_empty() {
            return city.to_string();
        }
        "Unknown place".to_string()
    }

    /// The secondary line: the street line and/or city not already shown as the
    /// title, joined with a comma. Empty when there is nothing more to say.
    #[must_use]
    pub fn subtitle(&self) -> String {
        let title = self.title();
        let mut parts: Vec<String> = Vec::new();
        let street = self.street_line();
        if !street.is_empty() && street != title {
            parts.push(street);
        }
        let city = self.city.trim();
        if !city.is_empty() && city != title {
            parts.push(city.to_string());
        }
        parts.join(", ")
    }
}

/// The outcome of a geocode: ranked results plus an optional human note (shown
/// when there is nothing to list — no gazetteer, no match, or a read error).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeocodeOutcome {
    /// Ranked matches (best first), possibly empty.
    pub results: Vec<GeoResult>,
    /// A one-line explanation shown in place of results, when there are none.
    pub note: Option<String>,
}

/// Turn free user text into an FTS5 prefix-MATCH expression: each alphanumeric
/// token becomes a quoted prefix term, AND-ed together. `None` when the text
/// holds no usable token (so we skip the query entirely). Quoting a token that
/// is pure alphanumeric (punctuation split out) keeps FTS5 operator keywords
/// (`AND`/`OR`/`NEAR`) and stray syntax from ever reaching the parser.
fn fts_match(text: &str) -> Option<String> {
    let terms: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" "))
    }
}

/// Run the prefix search against the gazetteer at `db`. Explicit-path entry
/// point (unit-tested against a synthetic fixture); the production path resolves
/// `db` via [`gazetteer_path`].
///
/// # Errors
/// Propagates a `rusqlite` error when the DB cannot be opened or the query
/// fails (e.g. the file is not a gazetteer). Callers use [`geocode`] to turn
/// that into a fail-soft note.
pub fn query_db(db: &Path, text: &str, limit: usize) -> rusqlite::Result<Vec<GeoResult>> {
    let Some(match_expr) = fts_match(text) else {
        return Ok(Vec::new());
    };
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare(
        "SELECT p.name, p.housenumber, p.street, p.city, p.lat, p.lon, p.kind \
         FROM places_fts f JOIN places p ON p.id = f.rowid \
         WHERE places_fts MATCH ?1 \
         ORDER BY rank \
         LIMIT ?2",
    )?;
    let limit = i64::try_from(limit).unwrap_or(50);
    let rows = stmt.query_map(params![match_expr, limit], |row| {
        Ok(GeoResult {
            name: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
            housenumber: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            street: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            city: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            lat: row.get(4)?,
            lon: row.get(5)?,
            kind: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        })
    })?;
    Ok(rows.flatten().collect())
}

/// The installed gazetteer path, if present.
#[must_use]
pub fn gazetteer_path() -> Option<PathBuf> {
    crate::basemap::region_dir()
        .map(|d| d.join("gazetteer.sqlite"))
        .filter(|p| p.exists())
}

/// Geocode `text` against the installed offline gazetteer, fail-soft.
///
/// Returns an empty result set plus an explanatory note when no gazetteer is
/// installed, the query has no usable token, there are no matches, or the DB
/// cannot be read.
#[must_use]
pub fn geocode(text: &str, limit: usize) -> GeocodeOutcome {
    let Some(db) = gazetteer_path() else {
        return GeocodeOutcome {
            results: Vec::new(),
            note: Some("No gazetteer installed for this region".to_string()),
        };
    };
    match query_db(&db, text, limit) {
        Ok(results) if results.is_empty() => GeocodeOutcome {
            results,
            note: Some("No matching places".to_string()),
        },
        Ok(results) => GeocodeOutcome {
            results,
            note: None,
        },
        Err(_) => GeocodeOutcome {
            results: Vec::new(),
            note: Some("Gazetteer could not be read".to_string()),
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)] // tests fail by panicking, with context
mod tests {
    use super::*;

    /// Build a synthetic 4-row FTS5 gazetteer matching the real bundle's schema,
    /// so tests never depend on `/root/mcnf-offline-mapdata`.
    fn synth_gazetteer(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE places(id INTEGER PRIMARY KEY, name TEXT, housenumber TEXT, \
               street TEXT, city TEXT, lat REAL, lon REAL, kind TEXT);
             CREATE VIRTUAL TABLE places_fts USING fts5(name, housenumber, street, city, \
               content='places', content_rowid='id', prefix='2 3', \
               tokenize='unicode61 remove_diacritics 2');",
        )
        .unwrap();
        let rows = [
            (
                1,
                "Athens",
                "",
                "",
                "Athens",
                32.2044,
                -95.8549,
                "place:town",
            ),
            (
                2,
                "Snowflake Donuts",
                "",
                "",
                "Athens",
                32.198,
                -95.853,
                "poi",
            ),
            (
                3,
                "",
                "500",
                "Medical Center Dr",
                "Athens",
                32.15,
                -95.86,
                "address",
            ),
            (
                4,
                "Murchison",
                "",
                "",
                "Murchison",
                32.278,
                -95.749,
                "place:village",
            ),
        ];
        for (id, name, hn, st, city, lat, lon, kind) in rows {
            conn.execute(
                "INSERT INTO places VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![id, name, hn, st, city, lat, lon, kind],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO places_fts(places_fts) VALUES('rebuild')", [])
            .unwrap();
    }

    #[test]
    fn prefix_match_finds_a_named_place() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("g.sqlite");
        synth_gazetteer(&db);
        let hits = query_db(&db, "snow", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Snowflake Donuts");
        assert!((hits[0].lat - 32.198).abs() < 1e-6);
    }

    #[test]
    fn prefix_match_is_multi_token_and() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("g.sqlite");
        synth_gazetteer(&db);
        // "medical center" AND-matches only the address row.
        let hits = query_db(&db, "medical center", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title(), "500 Medical Center Dr");
        assert_eq!(hits[0].subtitle(), "Athens");
    }

    #[test]
    fn city_prefix_matches_every_place_in_the_city() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("g.sqlite");
        synth_gazetteer(&db);
        // Three of the four rows are in Athens.
        let hits = query_db(&db, "athen", 10).unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn empty_or_punctuation_only_query_returns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("g.sqlite");
        synth_gazetteer(&db);
        assert!(query_db(&db, "   ", 10).unwrap().is_empty());
        assert!(query_db(&db, "!!!", 10).unwrap().is_empty());
        assert!(fts_match("  ,. ").is_none());
        assert_eq!(fts_match("500 Main").as_deref(), Some("\"500\"* \"Main\"*"));
    }

    #[test]
    fn missing_gazetteer_is_a_soft_note_not_an_error() {
        let out = geocode_at(Path::new("/nonexistent/g.sqlite"), "athens", 10);
        assert!(out.results.is_empty());
        assert!(out.note.is_some());
    }

    #[test]
    fn title_and_subtitle_compose_sensibly() {
        let poi = GeoResult {
            name: "Snowflake Donuts".into(),
            housenumber: String::new(),
            street: String::new(),
            city: "Athens".into(),
            lat: 0.0,
            lon: 0.0,
            kind: "poi".into(),
        };
        assert_eq!(poi.title(), "Snowflake Donuts");
        assert_eq!(poi.subtitle(), "Athens");

        let city = GeoResult {
            name: "Athens".into(),
            housenumber: String::new(),
            street: String::new(),
            city: "Athens".into(),
            lat: 0.0,
            lon: 0.0,
            kind: "place:town".into(),
        };
        // City equals title → not repeated in the subtitle.
        assert_eq!(city.title(), "Athens");
        assert_eq!(city.subtitle(), "");
    }

    /// Test helper mirroring [`geocode`] but against an explicit path.
    fn geocode_at(db: &Path, text: &str, limit: usize) -> GeocodeOutcome {
        if !db.exists() {
            return GeocodeOutcome {
                results: Vec::new(),
                note: Some("No gazetteer installed for this region".to_string()),
            };
        }
        match query_db(db, text, limit) {
            Ok(results) if results.is_empty() => GeocodeOutcome {
                results,
                note: Some("No matching places".to_string()),
            },
            Ok(results) => GeocodeOutcome {
                results,
                note: None,
            },
            Err(_) => GeocodeOutcome {
                results: Vec::new(),
                note: Some("Gazetteer could not be read".to_string()),
            },
        }
    }
}
