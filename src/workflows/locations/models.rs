use serde::{Deserialize, Serialize};

/// Tag values accepted by the `PocketBase` `locations.tags` select field.
pub const LOCATION_TAGS: [&str; 5] = ["exhibition", "store", "fleamarket", "museum", "other"];

/// `locations.tags` is a `maxSelect: 2` field.
pub const MAX_LOCATION_TAGS: u8 = 2;

/// `PocketBase` `geoPoint` value. An unset point comes back as `{"lon": 0, "lat": 0}`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

impl GeoPoint {
    /// `true` when the point carries real coordinates rather than `PocketBase`'s zero value.
    #[must_use]
    pub fn is_set(&self) -> bool {
        (self.lat.abs() > f64::EPSILON || self.lon.abs() > f64::EPSILON)
            && self.lat.is_finite()
            && self.lon.is_finite()
            && self.lat.abs() <= 90.0
            && self.lon.abs() <= 180.0
    }
}

/// A map location as the bot works with it, decoupled from the `PocketBase` wire format.
///
/// `location` shadows the struct name, but it is the collection's own field name for the
/// street address and renaming it here would only add a mapping step.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[allow(clippy::struct_field_names)]
pub struct Location {
    pub uid: String,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub opening_hours: Option<String>,
    pub url: Option<String>,
    pub tags: Option<Vec<String>>,
    pub image_url: Option<String>,
    pub image_description: Option<String>,
    pub point: Option<GeoPoint>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub state: Option<String>,
    pub updated: String,
}

/// Deserialization mirror of a record in the `PocketBase` `locations` collection.
#[derive(Deserialize, Debug, Clone)]
pub struct PbLocationRecord {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub image_description: Option<String>,
    #[serde(default)]
    pub point: Option<GeoPoint>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub opening_hours: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub updated: String,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct PbLocationResponse {
    pub items: Vec<PbLocationRecord>,
    pub page: u32,
    #[serde(rename = "totalPages")]
    pub total_pages: u32,
}

/// `PocketBase` returns `""` rather than `null` for unset text fields.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

impl PbLocationRecord {
    /// Converts the wire record into a [`Location`], normalizing `PocketBase`'s empty-string and
    /// zero-point representations of "unset" into `None`.
    #[must_use]
    pub fn into_location(self, pb_url: &str, collection: &str) -> Location {
        let image_url = non_empty(self.image).map(|img| {
            format!(
                "{}/api/files/{}/{}/{}",
                pb_url.trim_end_matches('/'),
                collection,
                self.id,
                img
            )
        });

        Location {
            uid: self.id,
            title: self.title,
            description: non_empty(self.description),
            location: non_empty(self.location),
            opening_hours: non_empty(self.opening_hours),
            url: non_empty(self.url),
            tags: self.tags.filter(|t| !t.is_empty()),
            image_url,
            image_description: non_empty(self.image_description),
            point: self.point.filter(GeoPoint::is_set),
            start_date: non_empty(self.start_date),
            end_date: non_empty(self.end_date),
            state: non_empty(self.state),
            updated: self.updated,
        }
    }
}

/// Payload handed to moderators in Zulip and, on approval, pushed to `PocketBase`.
///
/// Field order is the order moderators see in the YAML block, so keep the human-interesting
/// fields first. `point` is deliberately serialized even when unset — moderators fill it in.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LocationModerationPayload {
    pub id: String,
    pub title: String,
    pub location: String,
    pub opening_hours: String,
    pub description: String,
    pub url: String,
    pub tags: Vec<String>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub point: Option<GeoPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geo_point_is_set() {
        assert!(
            GeoPoint {
                lat: 60.1699,
                lon: 24.9384
            }
            .is_set()
        );
        // PocketBase's zero value for an unset geoPoint.
        assert!(!GeoPoint { lat: 0.0, lon: 0.0 }.is_set());
        // Out of range values are not usable coordinates.
        assert!(
            !GeoPoint {
                lat: 91.0,
                lon: 24.0
            }
            .is_set()
        );
        assert!(
            !GeoPoint {
                lat: 60.0,
                lon: 181.0
            }
            .is_set()
        );
    }

    #[test]
    fn test_pb_record_into_location_normalizes_unset_fields() {
        let json = serde_json::json!({
            "id": "abcdefghij12345",
            "title": "Legokauppa",
            "description": "",
            "start_date": "",
            "end_date": "",
            "location": "Mannerheimintie 1, Helsinki",
            "url": "",
            "image": "cover.png",
            "image_description": "",
            "point": {"lat": 0.0, "lon": 0.0},
            "tags": [],
            "opening_hours": "ma-pe 10-18",
            "state": "published",
            "updated": "2026-07-01 10:00:00.000Z"
        });

        let record: PbLocationRecord = serde_json::from_value(json).unwrap();
        let loc = record.into_location("https://pb.example.com/", "locations");

        assert_eq!(loc.uid, "abcdefghij12345");
        assert_eq!(loc.title, "Legokauppa");
        assert_eq!(loc.description, None);
        assert_eq!(loc.url, None);
        assert_eq!(loc.start_date, None);
        assert_eq!(loc.tags, None);
        assert_eq!(loc.point, None);
        assert_eq!(loc.opening_hours.as_deref(), Some("ma-pe 10-18"));
        assert_eq!(
            loc.image_url.as_deref(),
            Some("https://pb.example.com/api/files/locations/abcdefghij12345/cover.png")
        );
    }

    #[test]
    fn test_pb_record_into_location_keeps_real_point_and_tags() {
        let json = serde_json::json!({
            "id": "abcdefghij12345",
            "title": "Museo",
            "point": {"lat": 60.1699, "lon": 24.9384},
            "tags": ["museum", "store"],
            "updated": "2026-07-01 10:00:00.000Z"
        });

        let record: PbLocationRecord = serde_json::from_value(json).unwrap();
        let loc = record.into_location("https://pb.example.com", "locations");

        assert_eq!(
            loc.point,
            Some(GeoPoint {
                lat: 60.1699,
                lon: 24.9384
            })
        );
        assert_eq!(
            loc.tags,
            Some(vec!["museum".to_string(), "store".to_string()])
        );
        assert_eq!(loc.image_url, None);
    }
}
