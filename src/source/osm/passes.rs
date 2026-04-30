use std::collections::{HashMap, HashSet};
use std::path::Path;

use osmpbf::{Element, ElementReader};

use crate::common::importance::ImportanceCalculator;
use crate::config::Config;
use crate::target::json_writer::JsonWriter;
use crate::target::nominatim_place::NominatimPlace;

use super::admin::{ADMIN_LEVEL_COUNTY, ADMIN_LEVEL_MUNICIPALITY, AdministrativeBoundaryIndex};

/// Data collected in pass 1 (relations): admin boundaries and POI relation members.
pub(crate) struct Pass1Result {
    pub admin_relations: Vec<AdminRelationData>,
    pub poi_relation_member_way_ids: HashSet<i64>,
    pub poi_relation_node_ids: HashSet<i64>,
}

/// Data collected in pass 2 (ways): streets, POI ways, and the node IDs needed for pass 3.
pub(crate) struct Pass2Result {
    pub street_ways: Vec<StreetWayData>,
    pub poi_way_ids: HashSet<i64>,
    pub needed_node_ids: HashSet<i64>,
    pub admin_way_node_ids: HashMap<i64, Vec<i64>>,
}
use super::coordinate::{Coordinate, CoordinateStore};
use super::entity::{OsmEntityConverter, extract_country_code};
use super::indexing::{
    AdminRelationData, StreetWayData, build_admin_boundary_index, build_street_index,
};
use super::pass4;
use super::popularity::OsmPopularityCalculator;
use super::street::{StreetIndex, HIGHWAY_TYPES};

// ---------------------------------------------------------------------------
// OsmConverter -- main 4-pass converter
// ---------------------------------------------------------------------------

pub struct OsmConverter {
    config: Config,
}

impl OsmConverter {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Convert an OSM PBF file to Nominatim NDJSON.
    pub fn convert(
        &self,
        input: &Path,
        output: &Path,
        is_appending: bool,
        usage: &crate::common::usage::UsageBoost,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert!(input.exists(), "Input file does not exist: {:?}", input);

        let mut nodes_coords = CoordinateStore::new(500_000);
        let mut way_centroids = CoordinateStore::new(50_000);
        let mut admin_boundary_index = AdministrativeBoundaryIndex::new();
        let mut street_index = StreetIndex::new();
        let popularity_calculator = OsmPopularityCalculator::new(&self.config);

        let p1 = self.pass1_relations(input, &popularity_calculator)?;

        let p2 = self.pass2_ways(
            input,
            &p1.admin_relations,
            &p1.poi_relation_member_way_ids,
            &p1.poi_relation_node_ids,
            &popularity_calculator,
        )?;

        Self::pass3_nodes(input, &p2.needed_node_ids, &mut nodes_coords)?;

        build_admin_boundary_index(
            &p1.admin_relations,
            &p2.admin_way_node_ids,
            &nodes_coords,
            &mut admin_boundary_index,
        );
        eprintln!("  {}", admin_boundary_index.get_statistics());

        build_street_index(&p2.street_ways, &nodes_coords, &mut street_index);
        eprintln!("  {}", street_index.get_statistics());

        let results = self.pass4_convert(
            input,
            &p2.poi_way_ids,
            &p1.poi_relation_member_way_ids,
            &nodes_coords,
            &mut way_centroids,
            &mut admin_boundary_index,
            &street_index,
            &popularity_calculator,
            usage,
        )?;

        eprintln!("Finished processing {} entities", results.len());
        JsonWriter::export(&results, output, is_appending)?;

        Ok(())
    }

    /// Pass 1: Relations -- collect admin boundaries and POI relation member IDs.
    fn pass1_relations(
        &self,
        input: &Path,
        popularity_calculator: &OsmPopularityCalculator,
    ) -> Result<Pass1Result, Box<dyn std::error::Error>> {
        eprintln!("Pass 1/4: Scanning relations for admin boundaries and POI relations...");

        let mut admin_relations: Vec<AdminRelationData> = Vec::new();
        let mut poi_relation_member_way_ids: HashSet<i64> = HashSet::new();
        let mut poi_relation_node_ids: HashSet<i64> = HashSet::new();

        let reader = ElementReader::from_path(input)?;
        reader.for_each(|element| {
            if let Element::Relation(relation) = element {
                let tags: HashMap<&str, &str> = relation.tags().collect();

                collect_admin_relation(
                    &relation,
                    &tags,
                    &mut admin_relations,
                    &mut poi_relation_node_ids,
                );

                collect_poi_relation_members(
                    &relation,
                    &tags,
                    popularity_calculator,
                    &mut poi_relation_member_way_ids,
                    &mut poi_relation_node_ids,
                );
            }
        })?;

        eprintln!(
            "  Found {} admin boundary relations",
            admin_relations.len()
        );
        eprintln!(
            "  Found {} POI relation member ways",
            poi_relation_member_way_ids.len()
        );

        Ok(Pass1Result { admin_relations, poi_relation_member_way_ids, poi_relation_node_ids })
    }

    /// Pass 2: Ways -- collect all required node IDs and way metadata.
    fn pass2_ways(
        &self,
        input: &Path,
        admin_relations: &[AdminRelationData],
        poi_relation_member_way_ids: &HashSet<i64>,
        poi_relation_node_ids: &HashSet<i64>,
        popularity_calculator: &OsmPopularityCalculator,
    ) -> Result<Pass2Result, Box<dyn std::error::Error>> {
        eprintln!("Pass 2/4: Scanning ways for streets, admin boundaries, and POIs...");

        let admin_way_ids: HashSet<i64> = admin_relations
            .iter()
            .flat_map(|r| r.way_ids.iter().copied())
            .collect();

        let mut street_ways: Vec<StreetWayData> = Vec::new();
        let mut needed_node_ids: HashSet<i64> = poi_relation_node_ids.clone();
        let mut poi_way_ids: HashSet<i64> = HashSet::new();
        let mut admin_way_node_ids: HashMap<i64, Vec<i64>> = HashMap::new();

        let reader = ElementReader::from_path(input)?;
        reader.for_each(|element| {
            if let Element::Way(way) = element {
                let tags: HashMap<&str, &str> = way.tags().collect();
                let node_ids: Vec<i64> = way.refs().collect();

                process_way(
                    &way,
                    &tags,
                    &node_ids,
                    &admin_way_ids,
                    poi_relation_member_way_ids,
                    popularity_calculator,
                    &mut street_ways,
                    &mut needed_node_ids,
                    &mut poi_way_ids,
                    &mut admin_way_node_ids,
                );
            }
        })?;

        eprintln!("  Found {} street ways", street_ways.len());
        eprintln!("  Found {} POI ways", poi_way_ids.len());
        eprintln!(
            "  Total unique node coordinates needed: {}",
            needed_node_ids.len()
        );

        Ok(Pass2Result { street_ways, needed_node_ids, poi_way_ids, admin_way_node_ids })
    }

    /// Pass 3: Nodes -- collect coordinates for all needed nodes.
    fn pass3_nodes(
        input: &Path,
        needed_node_ids: &HashSet<i64>,
        nodes_coords: &mut CoordinateStore,
    ) -> Result<(), Box<dyn std::error::Error>> {
        eprintln!("Pass 3/4: Collecting node coordinates...");

        let reader = ElementReader::from_path(input)?;
        reader.for_each(|element| {
            match element {
                Element::Node(node)
                    if needed_node_ids.contains(&node.id()) => {
                        nodes_coords.put(
                            node.id(),
                            Coordinate {
                                lat: node.lat(),
                                lon: node.lon(),
                            },
                        );
                    }
                Element::DenseNode(node)
                    if needed_node_ids.contains(&node.id) => {
                        nodes_coords.put(
                            node.id,
                            Coordinate {
                                lat: node.lat(),
                                lon: node.lon(),
                            },
                        );
                    }
                _ => {}
            }
        })?;

        eprintln!("  Building administrative boundary index...");
        Ok(())
    }

    /// Pass 4: Read PBF again, collect POI data, compute centroids, and convert.
    #[allow(clippy::too_many_arguments)]
    fn pass4_convert(
        &self,
        input: &Path,
        poi_way_ids: &HashSet<i64>,
        poi_relation_member_way_ids: &HashSet<i64>,
        nodes_coords: &CoordinateStore,
        way_centroids: &mut CoordinateStore,
        admin_boundary_index: &mut AdministrativeBoundaryIndex,
        street_index: &StreetIndex,
        popularity_calculator: &OsmPopularityCalculator,
        usage: &crate::common::usage::UsageBoost,
    ) -> Result<Vec<NominatimPlace>, Box<dyn std::error::Error>> {
        eprintln!("Pass 4/4: Processing POI entities and writing output...");

        let all_needed_way_ids: HashSet<i64> = poi_way_ids
            .iter()
            .chain(poi_relation_member_way_ids.iter())
            .copied()
            .collect();

        let (node_data, way_data, rel_data) =
            pass4::collect_pass4_data(input, &all_needed_way_ids, popularity_calculator)?;

        pass4::compute_way_centroids(&way_data, nodes_coords, way_centroids);

        let importance_calc = ImportanceCalculator::new(&self.config.importance, usage);
        let mut converter = OsmEntityConverter {
            nodes_coords,
            way_centroids,
            admin_boundary_index,
            street_index,
            popularity_calculator,
            importance_calc,
            config: &self.config,
        };

        let mut results: Vec<NominatimPlace> = Vec::new();

        pass4::convert_poi_nodes(&node_data, &mut converter, &mut results);
        pass4::convert_poi_ways(&way_data, poi_way_ids, &mut converter, &mut results);
        pass4::convert_poi_relations(&rel_data, &mut converter, &mut results);

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Pass 1 helpers
// ---------------------------------------------------------------------------

fn collect_admin_relation(
    relation: &osmpbf::Relation,
    tags: &HashMap<&str, &str>,
    admin_relations: &mut Vec<AdminRelationData>,
    poi_relation_node_ids: &mut HashSet<i64>,
) {
    if tags.get("boundary") != Some(&"administrative") {
        return;
    }

    if let Some(admin_level_str) = tags.get("admin_level")
        && let Ok(admin_level) = admin_level_str.parse::<i32>()
            && (admin_level == ADMIN_LEVEL_COUNTY
                || admin_level == ADMIN_LEVEL_MUNICIPALITY)
            {
                let name = tags.get("name").map(|s| s.to_string());
                let ref_code = tags.get("ref").map(|s| s.to_string());
                let country = extract_country_code(tags);

                if let (Some(name), Some(ref_code), Some(country)) =
                    (name, ref_code, country)
                {
                    let way_ids: Vec<i64> = relation
                        .members()
                        .filter(|m| m.member_type == osmpbf::RelMemberType::Way)
                        .map(|m| m.member_id)
                        .collect();

                    admin_relations.push(AdminRelationData {
                        name,
                        admin_level,
                        ref_code,
                        way_ids,
                        country,
                    });
                }
            }

    // Collect node members from admin relations
    for member in relation.members() {
        if member.member_type == osmpbf::RelMemberType::Node {
            poi_relation_node_ids.insert(member.member_id);
        }
    }
}

fn collect_poi_relation_members(
    relation: &osmpbf::Relation,
    tags: &HashMap<&str, &str>,
    popularity_calculator: &OsmPopularityCalculator,
    poi_relation_member_way_ids: &mut HashSet<i64>,
    poi_relation_node_ids: &mut HashSet<i64>,
) {
    if !tags.contains_key("name")
        || !tags
            .iter()
            .any(|(k, v)| popularity_calculator.has_filter(k, v))
    {
        return;
    }

    for member in relation.members() {
        match member.member_type {
            osmpbf::RelMemberType::Way => {
                poi_relation_member_way_ids.insert(member.member_id);
            }
            osmpbf::RelMemberType::Node => {
                poi_relation_node_ids.insert(member.member_id);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Pass 2 helpers
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn process_way(
    way: &osmpbf::Way,
    tags: &HashMap<&str, &str>,
    node_ids: &[i64],
    admin_way_ids: &HashSet<i64>,
    poi_relation_member_way_ids: &HashSet<i64>,
    popularity_calculator: &OsmPopularityCalculator,
    street_ways: &mut Vec<StreetWayData>,
    needed_node_ids: &mut HashSet<i64>,
    poi_way_ids: &mut HashSet<i64>,
    admin_way_node_ids: &mut HashMap<i64, Vec<i64>>,
) {
    // Street way?
    if let Some(name) = tags.get("name")
        && let Some(highway) = tags.get("highway")
            && HIGHWAY_TYPES.contains(highway) {
                street_ways.push(StreetWayData {
                    name: name.to_string(),
                    node_ids: node_ids.to_vec(),
                });
                needed_node_ids.extend(node_ids);
            }

    // Admin boundary way?
    if admin_way_ids.contains(&way.id()) {
        admin_way_node_ids.insert(way.id(), node_ids.to_vec());
        needed_node_ids.extend(node_ids);
    }

    // POI relation member way?
    if poi_relation_member_way_ids.contains(&way.id()) {
        needed_node_ids.extend(node_ids);
    }

    // Direct POI way?
    if tags.contains_key("name")
        && tags
            .iter()
            .any(|(k, v)| popularity_calculator.has_filter(k, v))
    {
        poi_way_ids.insert(way.id());
        needed_node_ids.extend(node_ids);
    }
}
