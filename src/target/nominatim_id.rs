#[derive(Debug, Clone, Copy)]
pub enum NominatimId {
    Address,
    Street,
    Stedsnavn,
    StopPlace,
    Gosp,
    Osm,
    Poi,
}

impl NominatimId {
    fn prefix(self) -> i64 {
        match self {
            NominatimId::Address => 100,
            NominatimId::Street => 200,
            NominatimId::Stedsnavn => 300,
            NominatimId::StopPlace => 400,
            NominatimId::Gosp => 450,
            NominatimId::Osm => 500,
            NominatimId::Poi => 600,
        }
    }

    pub fn create(self, id: &str) -> i64 {
        let tail = id.rsplit(':').next().unwrap_or(id);
        let num = match tail.parse::<i64>() {
            Ok(n) => n.unsigned_abs() as i64,
            Err(_) => java_string_hashcode(id).unsigned_abs() as i64,
        };
        format!("{}{}", self.prefix(), num).parse::<i64>().unwrap_or(self.prefix())
    }

    pub fn create_from_i64(self, id: i64) -> i64 {
        self.create(&id.to_string())
    }
}

/// Matches Java/Kotlin `String.hashCode()`: s[0]*31^(n-1) + s[1]*31^(n-2) + ... + s[n-1]
fn java_string_hashcode(s: &str) -> i64 {
    let mut h: i32 = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as i32);
    }
    h as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_java_hashcode() {
        // Known Java hashCode values
        assert_eq!(java_string_hashcode(""), 0);
        assert_eq!(java_string_hashcode("a"), 97);
        assert_eq!(java_string_hashcode("abc"), 96354);
    }

    #[test]
    fn test_create_numeric() {
        assert_eq!(NominatimId::StopPlace.create("123"), 400123);
    }

    #[test]
    fn test_gosp_known_id() {
        let id = NominatimId::Gosp.create("NSR:GroupOfStopPlaces:1");
        assert_eq!(id, 4501);
    }

    #[test]
    fn test_all_prefixes() {
        assert_eq!(NominatimId::Address.create("1"), 1001);
        assert_eq!(NominatimId::Street.create("1"), 2001);
        assert_eq!(NominatimId::Stedsnavn.create("1"), 3001);
        assert_eq!(NominatimId::StopPlace.create("1"), 4001);
        assert_eq!(NominatimId::Gosp.create("1"), 4501);
        assert_eq!(NominatimId::Osm.create("1"), 5001);
        assert_eq!(NominatimId::Poi.create("1"), 6001);
    }

    #[test]
    fn test_create_from_i64() {
        assert_eq!(NominatimId::StopPlace.create_from_i64(42), 40042);
        assert_eq!(NominatimId::Address.create_from_i64(123456), 100123456);
    }

    #[test]
    fn test_negative_numeric_uses_abs() {
        // Negative numbers should use absolute value
        assert_eq!(NominatimId::Osm.create("-123"), 500123);
    }

    #[test]
    fn test_colon_separated_ids_use_numeric_tail() {
        // Structured IDs like KVE:PostalAddress:123 should extract the numeric tail
        assert_eq!(NominatimId::Address.create("KVE:PostalAddress:123"), 100123);
        assert_eq!(NominatimId::StopPlace.create("NSR:StopPlace:1"), 4001);
        assert_eq!(NominatimId::StopPlace.create("NSR:StopPlace:2"), 4002);
        // No collision between different numeric tails
        assert_ne!(
            NominatimId::Address.create("KVE:PostalAddress:41209458"),
            NominatimId::Address.create("KVE:PostalAddress:41209459"),
        );
    }

    #[test]
    fn test_java_hashcode_known_values() {
        // Verified against Java String.hashCode()
        assert_eq!(java_string_hashcode("hello"), 99162322);
        // "test" in Java: 't'*31^3 + 'e'*31^2 + 's'*31 + 't' = 3556498
        assert_eq!(java_string_hashcode("test"), 3556498);
    }

    #[test]
    fn test_java_hashcode_wrapping_arithmetic() {
        // Long strings can cause i32 overflow, must use wrapping arithmetic
        let hash = java_string_hashcode("this is a very long string that will overflow i32");
        // Just verify it doesn't panic and produces a deterministic result
        assert_eq!(hash, java_string_hashcode("this is a very long string that will overflow i32"));
    }
}
