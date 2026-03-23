use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Convert a structured ID like `KVE:PostalAddress:123` into a place_id string.
///
/// Photon accepts `[0-9a-zA-Z_-]{1,60}` for place_id. For IDs that only contain
/// ASCII characters, colons are replaced with dashes and other invalid chars with
/// underscores. For IDs containing non-ASCII (e.g. Norwegian Å, Ø, Æ), a hash
/// suffix is appended to prevent collisions from lossy sanitization.
pub fn as_place_id(id: &str) -> String {
    let has_non_ascii = id.bytes().any(|b| b > 127);

    if has_non_ascii {
        // Use prefix + hash to avoid collisions from lossy character replacement.
        // Budget: 43 chars for the sanitized prefix + 1 dash + 16 hex chars = 60 max.
        let prefix: String = id
            .chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
                ':' => '-',
                _ => '_',
            })
            .take(43) // leave room for dash + 16 hex chars = 17
            .collect();

        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        let hash = hasher.finish();

        format!("{prefix}-{hash:016x}")
    } else {
        id.chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => c,
                ':' => '-',
                _ => '_',
            })
            .take(60)
            .collect()
    }
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
    fn norwegian_chars_use_hash_suffix() {
        let id = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        assert!(id.starts_with("KVE-TopographicPlace-3907-_rfuglveien-"));
        assert!(id.len() <= 60);
    }

    #[test]
    fn different_norwegian_chars_produce_different_ids() {
        let a = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        let b = as_place_id("KVE:TopographicPlace:3907-Ørfuglveien");
        assert_ne!(a, b);
    }

    #[test]
    fn truncates_at_60_chars() {
        let long_id = "A".repeat(70);
        assert_eq!(as_place_id(&long_id).len(), 60);
    }

    #[test]
    fn norwegian_id_within_60_chars() {
        let id = as_place_id("KVE:TopographicPlace:3442-Steinsjøvegen");
        assert!(id.len() <= 60);
    }

    #[test]
    fn plain_numeric_id() {
        assert_eq!(as_place_id("12345"), "12345");
    }

    #[test]
    fn deterministic() {
        let a = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        let b = as_place_id("KVE:TopographicPlace:3907-Årfuglveien");
        assert_eq!(a, b);
    }
}
