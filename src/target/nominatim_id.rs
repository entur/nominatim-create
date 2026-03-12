/// Convert a structured ID like `KVE:PostalAddress:123` into a place_id string.
///
/// Photon accepts `[0-9a-zA-Z_-]{1,60}` for place_id, so any characters outside
/// that set are replaced with underscores. The result is truncated to 60 characters.
pub fn as_place_id(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
            ':' => '-',
            _ => '_',
        })
        .take(60)
        .collect();
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_id() {
        assert_eq!(as_place_id("NSR:StopPlace:59977"), "NSR-StopPlace-59977");
    }

    #[test]
    fn address_id() {
        assert_eq!(as_place_id("KVE:PostalAddress:225678815"), "KVE-PostalAddress-225678815");
    }

    #[test]
    fn street_id_with_spaces() {
        assert_eq!(
            as_place_id("KVE:TopographicPlace:0301-Karl Johans gate"),
            "KVE-TopographicPlace-0301-Karl_Johans_gate"
        );
    }

    #[test]
    fn truncates_at_60_chars() {
        let long_id = "A".repeat(70);
        assert_eq!(as_place_id(&long_id).len(), 60);
    }

    #[test]
    fn plain_numeric_id() {
        assert_eq!(as_place_id("12345"), "12345");
    }
}
