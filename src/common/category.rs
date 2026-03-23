// ---------------------------------------------------------------------------
// Category string constants for Nominatim NDJSON output.
//
// Categories are dot-separated strings stored in each place's `categories` array.
// They serve as facets for filtering/searching in the downstream Photon geocoder.
//
// Naming convention:
//   osm.*      — primary entity type (address, street, stop_place, poi, etc.)
//   source.*   — data source identifier (used by acceptance tests to filter by origin)
//   layer.*    — broad classification layer (used for result type filtering)
//   legacy.*   — compatibility categories matching the original converter's output
//   country.*  — ISO country code
//   *_gid.*    — geographic ID references (county, locality)
// ---------------------------------------------------------------------------

// Primary entity types
pub const OSM_ADDRESS: &str = "osm.public_transport.address";
pub const OSM_STREET: &str = "osm.public_transport.street";
pub const OSM_STOP_PLACE: &str = "osm.public_transport.stop_place";
pub const OSM_POI: &str = "osm.public_transport.poi";
pub const OSM_CUSTOM_POI: &str = "osm.public_transport.custom_poi";
pub const OSM_GOSP: &str = "osm.public_transport.group_of_stop_places";

// Data source identifiers
pub const SOURCE_ADRESSE: &str = "source.kartverket.matrikkelenadresse";
pub const SOURCE_STEDSNAVN: &str = "source.kartverket.stedsnavn";
pub const SOURCE_NSR: &str = "source.nsr";

pub const GOSP: &str = "GroupOfStopPlaces";

pub const SOURCE_OSM: &str = "source.openstreetmap";
pub const SOURCE_POI: &str = "source.custom.poi";
pub const SOURCE_BELAGENHET: &str = "source.lantmateriet.belagenhetsadress";

// Classification layers
pub const LAYER_ADDRESS: &str = "layer.address";
pub const LAYER_STREET: &str = "layer.street";
pub const LAYER_STOP_PLACE: &str = "layer.stopPlace";
pub const LAYER_GOSP: &str = "layer.groupOfStopPlaces";
pub const LAYER_POI: &str = "layer.poi";

// Category prefixes
pub const COUNTRY_PREFIX: &str = "country.";
pub const TARIFF_ZONE_ID_PREFIX: &str = "tariff_zone_id.";
pub const TARIFF_ZONE_AUTH_PREFIX: &str = "tariff_zone_authority.";
pub const FARE_ZONE_PREFIX: &str = "fare_zone_authority.";
pub const COUNTY_ID_PREFIX: &str = "county_gid.";
pub const LOCALITY_ID_PREFIX: &str = "locality_gid.";
pub const LEGACY_CATEGORY_PREFIX: &str = "legacy.category.";

/// Convert a colon-separated ID (e.g. `NSR:StopPlace:123`) to a dot-separated
/// category string (`NSR.StopPlace.123`), since colons are not valid in categories.
pub fn as_category(s: &str) -> String {
    s.replace(':', ".")
}

pub fn tariff_zone_id_category(ref_: &str) -> String {
    format!("{TARIFF_ZONE_ID_PREFIX}{}", as_category(ref_))
}

pub fn fare_zone_authority_category(ref_: &str) -> String {
    format!("{FARE_ZONE_PREFIX}{}", as_category(ref_))
}

pub fn county_ids_category(ref_: &str) -> String {
    format!("{COUNTY_ID_PREFIX}{}", as_category(ref_))
}

pub fn locality_ids_category(ref_: &str) -> String {
    format!("{LOCALITY_ID_PREFIX}{}", as_category(ref_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_category_replaces_colons() {
        assert_eq!(as_category("NSR:StopPlace:123"), "NSR.StopPlace.123");
    }

    #[test]
    fn test_as_category_no_colons() {
        assert_eq!(as_category("something"), "something");
    }

    #[test]
    fn test_tariff_zone_id_category() {
        assert_eq!(
            tariff_zone_id_category("RUT:TariffZone:1"),
            "tariff_zone_id.RUT.TariffZone.1"
        );
    }

    #[test]
    fn test_fare_zone_authority_category() {
        assert_eq!(
            fare_zone_authority_category("RUT:Authority:RUT"),
            "fare_zone_authority.RUT.Authority.RUT"
        );
    }

    #[test]
    fn test_county_ids_category() {
        assert_eq!(
            county_ids_category("KVE:TopographicPlace:03"),
            "county_gid.KVE.TopographicPlace.03"
        );
    }

    #[test]
    fn test_locality_ids_category() {
        assert_eq!(
            locality_ids_category("KVE:TopographicPlace:0301"),
            "locality_gid.KVE.TopographicPlace.0301"
        );
    }
}
