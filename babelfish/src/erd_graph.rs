use std::collections::HashMap;

use crate::erd::{Erd, RelationshipType, ConstraintType};
use petgraph::{algo::steiner_tree, dot::Dot, graph::{NodeIndex, UnGraph}, prelude::StableUnGraph};

pub struct ErdGraph {
    pub graph: UnGraph<String, usize>,
    pub node_indices: HashMap<String, NodeIndex>,
}

impl ErdGraph {
    pub fn new(erd: &Erd) -> Self {
        let mut graph = UnGraph::default();
        let mut node_indices = std::collections::HashMap::new();
        
        // Add entities as nodes
        for (entity_name, _) in erd.iter() {
            // TODO: we may not want to add the names as node labels for efficiency
            let node_index = graph.add_node(entity_name.to_string());
            node_indices.insert(entity_name.to_string(), node_index);
        }
        
        for (source_entity_name, source_index) in node_indices.iter() {
            for (target_entity_name, target_index) in node_indices.iter() {
                if source_index == target_index {
                    continue; // Skip self-loops
                }
                let weight = get_weight(erd, source_entity_name, target_entity_name);
                if let Some(old_edge) = graph.find_edge(*target_index, *source_index) {
                    let old_weight = graph.edge_weight_mut(old_edge).unwrap();
                    // If an edge already exists, we should update the weight to the cheaper weight,
                    // this can happen if we are replacing the source with a direct relationship.
                    if weight < *old_weight {
                        *old_weight = weight;
                    }
                } else {
                    graph.add_edge(*source_index, *target_index, weight);
                }
            }
        }
        ErdGraph { graph, node_indices }
    }

    pub fn to_steiner_tree(&self, entities: &[String]) -> StableUnGraph<String, usize> {
        let nodes: Vec<_> = entities.iter().map(|entity| {
        self.node_indices.get(entity).expect("Entity not found in node indices").clone()
        }).collect();
        steiner_tree::steiner_tree(
            &self.graph,
 nodes.as_slice(),
        )
    }

    pub fn print(&self, entities: &[String]) {
         println!("{:?}", Dot::with_config(&self.graph, &[]));
         println!("Node indices: {:?}", self.node_indices);
         let tree = self.to_steiner_tree(entities);
         println!("Steiner tree: {:?}", Dot::with_config(&tree, &[]));
    }
}

fn get_weight(erd: &Erd, source_entity_name: &str, target_entity_name: &str) -> usize {
    let get_relationship_weight = |relationship_type, constraint_type| {
        match (relationship_type, constraint_type) {
            // weights should actually be based of cardinality with a large constant factor for
            // being foreign vs embeded
            (ConstraintType::Embedded, RelationshipType::OneToOne) => 1,
            (ConstraintType::Embedded, RelationshipType::ManyToOne) => 2,
            (ConstraintType::Embedded, RelationshipType::ManyToMany) => 4,
            (ConstraintType::Foreign, RelationshipType::OneToOne) => erd.size(),
            (ConstraintType::Foreign, RelationshipType::ManyToOne) => erd.size() * 2,
            (ConstraintType::Foreign, RelationshipType::ManyToMany) => erd.size() * 4,
        }
    };
    if let Some(relationship) = erd.get_relationship(source_entity_name, target_entity_name) {
        get_relationship_weight(relationship.constraint.constraint_type, relationship.relationship_type)
    } else if let Some(source) = erd.get_source(source_entity_name) {
        get_relationship_weight(
            if source.target_path.is_some() {
                ConstraintType::Embedded
            } else {
                ConstraintType::Foreign
            },
            RelationshipType::ManyToOne, // Default to ManyToOne if no relationship found
        )
    } else {
        unreachable!() // No source found
    }
}
