use std::collections::HashMap;
use std::path::Path;

use crate::config::UsageConfig;

/// Optional per-entity popularity nudge driven by an external semicolon-separated CSV.
///
/// Format: `id;...;usage` - the first field is the entity ID, the last field is the
/// usage count, and any columns in between (e.g. a human-readable name) are ignored.
/// `id;usage` works too. Header row is optional (skipped if column N doesn't parse
/// as a u64). Lines starting with `#` and blank lines are ignored.
///
/// The CSV is shared across all sources. Any ID present in the file gets a
/// bounded multiplicative boost on its raw popularity; missing IDs and IDs at
/// or below `usage_floor` get factor 1.0 (no change). The shape is
/// `1 + alpha * log10(usage / usage_floor)`, so the signal is gentle: a stop
/// with 1000x the floor receives a ~2.5x popularity boost at the default
/// alpha=0.5, which translates to roughly +0.05 importance after the log10
/// normalisation in [`crate::common::importance::ImportanceCalculator`].
pub struct UsageBoost {
    counts: HashMap<String, u64>,
    cfg: UsageConfig,
}

impl UsageBoost {
    pub fn empty() -> Self {
        Self { counts: HashMap::new(), cfg: UsageConfig::default() }
    }

    pub fn load(path: Option<&Path>, cfg: &UsageConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let Some(path) = path else {
            return Ok(Self::empty());
        };
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read usage CSV '{}': {e}", path.display()))?;
        let mut counts = HashMap::new();
        for (lineno, raw) in content.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split(';').collect();
            if parts.len() < 2 { continue; }
            let id = parts[0].trim();
            let usage_str = parts[parts.len() - 1].trim();
            let usage: u64 = match usage_str.parse() {
                Ok(n) => n,
                // Tolerate a non-numeric first row as a header.
                Err(_) if lineno == 0 => continue,
                Err(e) => return Err(format!("invalid usage on line {}: {raw} ({e})", lineno + 1).into()),
            };
            counts.insert(id.to_string(), usage);
        }
        eprintln!("Loaded usage data: {} entries from {}", counts.len(), path.display());
        Ok(Self { counts, cfg: cfg.clone() })
    }

    pub fn factor(&self, id: &str) -> f64 {
        let Some(&usage) = self.counts.get(id) else { return 1.0 };
        if usage <= self.cfg.usage_floor {
            return 1.0;
        }
        1.0 + self.cfg.alpha * (usage as f64 / self.cfg.usage_floor as f64).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> UsageConfig {
        UsageConfig { alpha: 0.5, usage_floor: 100 }
    }

    fn with(counts: &[(&str, u64)]) -> UsageBoost {
        let mut counts_map = HashMap::new();
        for (id, n) in counts {
            counts_map.insert((*id).to_string(), *n);
        }
        UsageBoost { counts: counts_map, cfg: cfg() }
    }

    #[test]
    fn empty_returns_one() {
        let u = UsageBoost::empty();
        assert_eq!(u.factor("any"), 1.0);
    }

    #[test]
    fn missing_id_returns_one() {
        let u = with(&[("a", 1_000_000)]);
        assert_eq!(u.factor("not-there"), 1.0);
    }

    #[test]
    fn at_or_below_floor_returns_one() {
        let u = with(&[("eq", 100), ("below", 5)]);
        assert_eq!(u.factor("eq"), 1.0);
        assert_eq!(u.factor("below"), 1.0);
    }

    #[test]
    fn above_floor_uses_log_scaled_boost() {
        let u = with(&[("hot", 10_000)]); // 100 * floor
        let f = u.factor("hot");
        let expected = 1.0 + 0.5 * (100f64).log10();
        assert!((f - expected).abs() < 1e-9);
    }

    #[test]
    fn very_popular_stays_bounded() {
        let u = with(&[("oslo_s", 5_000_000)]); // 50_000 * floor
        let f = u.factor("oslo_s");
        assert!(f < 4.0, "boost should nudge, not flip rankings: got {f}");
    }

    #[test]
    fn parses_minimal_csv_with_optional_header() {
        let path = std::env::temp_dir().join("usage_boost_minimal.csv");
        std::fs::write(&path, "id;name;usage\nNSR:StopPlace:1;Oslo S;1500\n# comment\n\nNSR:StopPlace:2;Lillestrøm;42\n").unwrap();
        let u = UsageBoost::load(Some(&path), &cfg()).unwrap();
        assert_eq!(u.factor("NSR:StopPlace:1"), 1.0 + 0.5 * (15f64).log10());
        assert_eq!(u.factor("NSR:StopPlace:2"), 1.0); // below floor
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parses_csv_without_header() {
        let path = std::env::temp_dir().join("usage_boost_no_header.csv");
        std::fs::write(&path, "NSR:StopPlace:1;Oslo S;1500\nNSR:StopPlace:2;Bergen;2500\n").unwrap();
        let u = UsageBoost::load(Some(&path), &cfg()).unwrap();
        assert!(u.factor("NSR:StopPlace:1") > 1.0);
        assert!(u.factor("NSR:StopPlace:2") > 1.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn parses_csv_with_only_id_and_usage() {
        let path = std::env::temp_dir().join("usage_boost_two_col.csv");
        std::fs::write(&path, "NSR:StopPlace:1;1500\n").unwrap();
        let u = UsageBoost::load(Some(&path), &cfg()).unwrap();
        assert!(u.factor("NSR:StopPlace:1") > 1.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn extra_columns_are_ignored() {
        let path = std::env::temp_dir().join("usage_boost_extra_cols.csv");
        std::fs::write(&path, "id;name;type;usage\nNSR:StopPlace:1;Oslo S;rail;1500\n").unwrap();
        let u = UsageBoost::load(Some(&path), &cfg()).unwrap();
        assert!(u.factor("NSR:StopPlace:1") > 1.0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_non_header_line_errors() {
        let path = std::env::temp_dir().join("usage_boost_invalid.csv");
        std::fs::write(&path, "id;name;usage\nNSR:StopPlace:1;Oslo S;1500\nNSR:StopPlace:2;Bergen;not-a-number\n").unwrap();
        let res = UsageBoost::load(Some(&path), &cfg());
        assert!(res.is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn no_path_disables_boost() {
        let u = UsageBoost::load(None, &cfg()).unwrap();
        assert_eq!(u.factor("anything"), 1.0);
    }
}
