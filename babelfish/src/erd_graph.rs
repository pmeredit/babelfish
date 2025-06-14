use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use crate::erd::{ConstraintType, Erd, ErdRelationship, RelationshipType};
use petgraph::{algo::{self, steiner_tree}, dot::Dot, graph::{DiGraph, NodeIndex, UnGraph}, prelude::StableUnGraph, visit::{EdgeRef, IntoEdgeReferences}};

pub struct ErdGraph {
    pub graph: DiGraph<String, usize>,
    pub node_indices: HashMap<String, NodeIndex>,
    pub edge_data: HashMap<NodeIndex, HashMap<NodeIndex, EdgeData>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeData {
    EmbeddedSource {
        db: String,
        collection: String,
        target_path: String,
        // Assume foreign_key and local_key are the primary keys for each entity
    },
    Embedded{
        source_entity: String,
        target_path: String,
        relationship_type: RelationshipType,
    },
    Foreign {
        db: String,
        collection: String,
        foreign_key: String,
        local_key: String,
        relationship_type: RelationshipType,
    },
}

impl EdgeData {
    pub fn not_parent(&self, node_label: &str) -> bool {
        match self {
            EdgeData::EmbeddedSource { .. } => true,
            EdgeData::Embedded { source_entity, .. } => source_entity != node_label,
            EdgeData::Foreign { .. } => true,
        }
    }
}

impl ErdGraph {
    pub fn new(erd: &Erd) -> Self {
        let mut graph = DiGraph::default();
        let mut node_indices = HashMap::new();
        let mut edge_data: HashMap<_, HashMap<_,_>> = HashMap::new();
        
        // Add entities as nodes
        for (entity_name, _) in erd.iter() {
            // TODO: we may not want to add the names as node labels for efficiency
            let node_index = graph.add_node(entity_name.to_string());
            node_indices.insert(entity_name.to_string(), node_index);
        }
        
        // convert to Vec to induce a stable order
        let node_indices_vec: Vec<_> = node_indices.iter().map(|(s, n)|(s, *n)).collect();
        for (source_entity_name, source_index) in node_indices_vec.iter() {
            for (target_entity_name, target_index) in node_indices_vec.iter() {
                if source_entity_name == target_entity_name {
                    continue; // Skip self-loops
                }
                if let Some((weight, constraint)) = get_edge_data(erd, source_entity_name, target_entity_name) {
                    graph.add_edge(*source_index, *target_index, weight);
                    edge_data.entry(*source_index)
                        .or_default()
                        .insert(*target_index, constraint);
                }
            }
        }
        Self { graph, node_indices, edge_data }
    }

    pub fn get_entity_name(&self, node_index: NodeIndex) -> Option<&String> {
        self.graph.node_weight(node_index)
    }

    pub fn get_edge_data_by_names(&self, source_entity_name: &str, target_entity_name: &str) -> Option<&EdgeData> {
        let source_index = self.node_indices.get(source_entity_name)?;
        let target_index = self.node_indices.get(target_entity_name)?;
        self.get_edge_data(*source_index, *target_index)
    }

    pub fn get_edge_data(&self, source_index: NodeIndex, target_index: NodeIndex) -> Option<&EdgeData> {
        self.edge_data.get(&source_index)
            .and_then(|edges| edges.get(&target_index))
            .or_else(|| self.edge_data.get(&target_index).and_then(|edges| edges.get(&source_index)))
    }
}

impl std::fmt::Display for ErdGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", Dot::with_config(&self.graph, &[]))
    }
}

fn get_edge_data(erd: &Erd, source_entity_name: &str, target_entity_name: &str) -> Option<(usize, EdgeData)> {
    let get_relationship_weight = |relationship_type, constraint_type| {
        match (relationship_type, constraint_type) {
            // datas should actually be based of cardinality with a large constant factor for
            // being foreign vs embeded
            (ConstraintType::Embedded, RelationshipType::OneToOne) => 1,
            (ConstraintType::Embedded, RelationshipType::ManyToOne) => 2,
            (ConstraintType::Embedded, RelationshipType::ManyToMany) => 4,
            (ConstraintType::Foreign, RelationshipType::OneToOne) => erd.size(),
            (ConstraintType::Foreign, RelationshipType::ManyToOne) => erd.size() * 2,
            (ConstraintType::Foreign, RelationshipType::ManyToMany) => erd.size() * 4,
        }
    };
    let get_relationship_constraint = |entity_name: &str, relationship: &ErdRelationship| {
        match relationship.constraint.constraint_type {
            ConstraintType::Embedded => EdgeData::Embedded {
                source_entity: entity_name.to_string(),
                target_path: relationship.constraint.target_path.clone().unwrap_or_default(),
                relationship_type: relationship.relationship_type, // Default to ManyToOne if no relationship found
            },
            ConstraintType::Foreign => {
                let source = erd.get_source(entity_name).expect("Source not found");
                EdgeData::Foreign {
                    db: source.db.clone(),
                    collection: source.collection.clone(),
                    relationship_type: relationship.relationship_type,
                    foreign_key: relationship.constraint.foreign_key.clone().unwrap(),
                    local_key: relationship.constraint.local_key.clone().unwrap(),
                }
            },
        }
    };
    if let Some(relationship) = erd.get_relationship(source_entity_name, target_entity_name) {
        Some((
            get_relationship_weight(relationship.constraint.constraint_type, relationship.relationship_type),
            get_relationship_constraint(source_entity_name, relationship),
        ))
    } else {
        None
    }
}
