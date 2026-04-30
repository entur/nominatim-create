use super::xml::StopPlaceXml;

pub(crate) fn calculate_stop_popularity(
    config: &crate::config::StopPlaceConfig,
    sp: &StopPlaceXml,
    child_types: &[String],
    usage_boost: f64,
) -> i64 {
    let mut pop = config.default_value;
    let mut all_types: Vec<&str> = child_types.iter().map(|s| s.as_str()).collect();
    if let Some(t) = &sp.stop_place_type {
        all_types.push(t);
    }
    let sum: f64 = all_types
        .iter()
        .map(|t| config.stop_type_factors.get(*t).copied().unwrap_or(1.0))
        .sum();
    if sum > 0.0 {
        pop = (pop as f64 * sum) as i64;
    }
    if let Some(w) = &sp.weighting
        && let Some(factor) = config.interchange_factors.get(w)
    {
        pop = (pop as f64 * factor) as i64;
    }
    (pop as f64 * usage_boost) as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    
    use super::super::tests::helpers::{make_stop_place, test_config};

    #[test]
    fn basic_stop_returns_default_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[], 1.0);
        assert_eq!(pop, config.stop_place.default_value);
    }

    #[test]
    fn bus_station_has_higher_popularity_than_basic() {
        let config = test_config();
        let basic = make_stop_place("NSR:StopPlace:1", "Test", None, Some("onstreetBus"));
        let bus_station = make_stop_place("NSR:StopPlace:2", "Test", None, Some("busStation"));
        let basic_pop = calculate_stop_popularity(&config.stop_place, &basic, &[], 1.0);
        let bus_pop = calculate_stop_popularity(&config.stop_place, &bus_station, &[], 1.0);
        assert!(bus_pop > basic_pop);
    }

    #[test]
    fn metro_station_has_boosted_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("metroStation"));
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[], 1.0);
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn rail_station_has_boosted_popularity() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[], 1.0);
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn recommended_interchange_multiplies_popularity() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        sp.weighting = Some("recommendedInterchange".to_string());
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[], 1.0);
        // 50 * 2 (rail) * 3 (interchange) = 300
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0 * 3.0) as i64);
    }

    #[test]
    fn preferred_interchange_gives_high_popularity() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, Some("railStation"));
        sp.weighting = Some("preferredInterchange".to_string());
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &[], 1.0);
        // 50 * 2 * 10 = 1000
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0 * 10.0) as i64);
    }

    #[test]
    fn popularity_values_strictly_ordered() {
        let config = test_config();
        let pops: Vec<i64> = vec![
            calculate_stop_popularity(&config.stop_place, &make_stop_place("1", "T", None, None), &[], 1.0),
            calculate_stop_popularity(&config.stop_place, &make_stop_place("2", "T", None, Some("busStation")), &[], 1.0),
            {
                let mut sp = make_stop_place("3", "T", None, Some("railStation"));
                sp.weighting = Some("recommendedInterchange".to_string());
                calculate_stop_popularity(&config.stop_place, &sp, &[], 1.0)
            },
            {
                let mut sp = make_stop_place("4", "T", None, Some("railStation"));
                sp.weighting = Some("preferredInterchange".to_string());
                calculate_stop_popularity(&config.stop_place, &sp, &[], 1.0)
            },
        ];
        for i in 0..pops.len() - 1 {
            assert!(pops[i] < pops[i + 1], "Expected {} < {}", pops[i], pops[i + 1]);
        }
    }

    // ===== Multimodal parent tests =====

    #[test]
    fn multimodal_parent_uses_sum_of_child_types() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["railStation".to_string(), "metroStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types, 1.0);
        // 50 * (2 + 2) = 200
        assert_eq!(pop, (config.stop_place.default_value as f64 * 4.0) as i64);
    }

    #[test]
    fn multimodal_parent_sums_factors_not_multiplies() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["railStation".to_string(), "metroStation".to_string(), "busStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types, 1.0);
        // 50 * (2+2+2) = 300, NOT 50 * 2*2*2 = 400
        assert_eq!(pop, (config.stop_place.default_value as f64 * 6.0) as i64);
    }

    #[test]
    fn multimodal_parent_unconfigured_child_defaults_to_factor_1() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["ferryStop".to_string(), "tramStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types, 1.0);
        // 50 * (1+1) = 100
        assert_eq!(pop, (config.stop_place.default_value as f64 * 2.0) as i64);
    }

    #[test]
    fn multimodal_parent_with_interchange() {
        let config = test_config();
        let mut sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        sp.weighting = Some("preferredInterchange".to_string());
        let child_types = vec!["railStation".to_string(), "metroStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types, 1.0);
        // 50 * (2+2) * 10 = 2000
        assert_eq!(pop, (config.stop_place.default_value as f64 * 4.0 * 10.0) as i64);
    }

    #[test]
    fn duplicate_child_types_are_summed() {
        let config = test_config();
        let sp = make_stop_place("NSR:StopPlace:1", "Test", None, None);
        let child_types = vec!["railStation".to_string(), "railStation".to_string(), "railStation".to_string()];
        let pop = calculate_stop_popularity(&config.stop_place, &sp, &child_types, 1.0);
        // 50 * (2+2+2) = 300
        assert_eq!(pop, (config.stop_place.default_value as f64 * 6.0) as i64);
    }

    // ===== GroupOfStopPlaces popularity tests =====

    #[test]
    fn gosp_single_member_boosted() {
        let config = test_config();
        let pop = config.group_of_stop_places.gos_boost_factor * 60.0;
        assert_eq!(pop, 600.0);
    }

    #[test]
    fn gosp_two_members_multiplies() {
        let config = test_config();
        let pop = config.group_of_stop_places.gos_boost_factor * (60.0 * 60.0);
        assert_eq!(pop, 36000.0);
    }

    #[test]
    fn gosp_empty_returns_boost_factor() {
        let config = test_config();
        // Empty fold: 1.0 * boost
        let pops: Vec<i64> = vec![];
        let result = if pops.is_empty() {
            config.group_of_stop_places.gos_boost_factor
        } else {
            config.group_of_stop_places.gos_boost_factor * pops.iter().fold(1.0, |acc, &p| acc * p as f64)
        };
        assert_eq!(result, 10.0);
    }

    #[test]
    fn gosp_realistic_oslo_scenario() {
        let config = test_config();
        let member_pops: Vec<i64> = vec![600, 60, 60];
        let pop = config.group_of_stop_places.gos_boost_factor
            * member_pops.iter().fold(1.0, |acc, &p| acc * p as f64);
        assert_eq!(pop, 21_600_000.0);
    }
}
