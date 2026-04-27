use crate::common::util::round6;
use crate::config::ImportanceConfig;

/// Normalizes raw popularity scores to Photon importance values in the 0-1 range.
///
/// Uses log10 normalization because popularity values span many orders of magnitude
/// (e.g. 1 for a small address to 1 billion for a major city). Log scaling compresses
/// this range so that differences between low-popularity items are still visible.
/// The `floor` config value sets the minimum importance (typically 0.1).
pub struct ImportanceCalculator {
    config: ImportanceConfig,
}

impl ImportanceCalculator {
    pub fn new(config: &ImportanceConfig) -> Self {
        Self { config: *config }
    }

    /// Normalize popularity to Photon importance (0-1 range) using log10 normalization.
    pub fn calculate_importance(&self, popularity: f64) -> f64 {
        round6(self.scaled_importance(popularity).clamp(self.config.floor, 1.0))
    }

    /// Like [`Self::calculate_importance`] but without the upper clamp at 1.0. Used for entries
    /// (e.g. GroupOfStopPlaces) whose popularity grows multiplicatively past `maxPopularity` and
    /// where downstream Photon scoring (which has no implicit cap) needs them to dominate the
    /// importance band so far-away major cities can outrank near-focus streets sharing the same
    /// name prefix. Caller typically multiplies the result by a category-specific factor.
    pub fn calculate_importance_unclamped(&self, popularity: f64) -> f64 {
        round6(self.scaled_importance(popularity).max(self.config.floor))
    }

    fn scaled_importance(&self, popularity: f64) -> f64 {
        let log_pop = popularity.log10();
        let log_min = self.config.min_popularity.log10();
        let log_max = self.config.max_popularity.log10();
        let normalized = (log_pop - log_min) / (log_max - log_min);
        self.config.floor + (normalized * (1.0 - self.config.floor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prod_config() -> ImportanceConfig {
        ImportanceConfig {
            min_popularity: 1.0,
            max_popularity: 1_000_000_000.0,
            floor: 0.1,
        }
    }

    #[test]
    fn test_min_popularity_returns_floor() {
        let calc = ImportanceCalculator::new(&prod_config());
        assert_eq!(calc.calculate_importance(1.0), 0.1);
    }

    #[test]
    fn test_max_popularity_returns_one() {
        let calc = ImportanceCalculator::new(&prod_config());
        assert_eq!(calc.calculate_importance(1_000_000_000.0), 1.0);
    }

    #[test]
    fn test_mid_popularity() {
        let calc = ImportanceCalculator::new(&prod_config());
        let imp = calc.calculate_importance(1000.0);
        // log10(1000) = 3, log10(1) = 0, log10(1e9) = 9
        // normalized = 3/9 = 0.333...
        // scaled = 0.1 + (0.333... * 0.9) = 0.1 + 0.3 = 0.4
        assert_eq!(imp, 0.4);
    }

    #[test]
    fn test_below_min_clamps_to_floor() {
        let calc = ImportanceCalculator::new(&prod_config());
        // popularity of 0.1 would give negative normalized => clamp to floor
        let imp = calc.calculate_importance(0.1);
        assert_eq!(imp, 0.1);
    }

    #[test]
    fn test_above_max_clamps_to_one() {
        let calc = ImportanceCalculator::new(&prod_config());
        let imp = calc.calculate_importance(10_000_000_000.0);
        assert_eq!(imp, 1.0);
    }

    #[test]
    fn test_known_stop_place_importance() {
        // StopPlace default = 50
        let calc = ImportanceCalculator::new(&prod_config());
        let imp = calc.calculate_importance(50.0);
        // log10(50)=1.699, normalized=1.699/9=0.1888, scaled=0.1+0.1888*0.9=0.2699
        assert_eq!(imp, 0.269897);
    }

    #[test]
    fn test_known_matrikkel_importance() {
        // Matrikkel address popularity = 20
        let calc = ImportanceCalculator::new(&prod_config());
        let imp = calc.calculate_importance(20.0);
        assert_eq!(imp, 0.230103);
    }

    #[test]
    fn test_unclamped_below_min_still_clamps_to_floor() {
        let calc = ImportanceCalculator::new(&prod_config());
        assert_eq!(calc.calculate_importance_unclamped(0.1), 0.1);
    }

    #[test]
    fn test_unclamped_above_max_exceeds_one() {
        let calc = ImportanceCalculator::new(&prod_config());
        // log10(1e14)=14, normalized=14/9=1.555..., scaled=0.1+1.555...*0.9=1.5
        let imp = calc.calculate_importance_unclamped(1e14);
        assert_eq!(imp, 1.5);
    }

    #[test]
    fn test_unclamped_at_max_matches_clamped() {
        let calc = ImportanceCalculator::new(&prod_config());
        assert_eq!(calc.calculate_importance_unclamped(1_000_000_000.0), 1.0);
    }
}
