use crate::erd::{ConstraintType, Erd, ErdRelationship, RelationshipType, Relationships, Source};
use petgraph::{
    algo,
    dot::Dot,
    graph::{DiGraph, NodeIndex},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap};

pub struct ErdGraph {
    pub graph: DiGraph<String, usize>,
    pub node_indices: HashMap<String, NodeIndex>,
    pub edge_data: HashMap<NodeIndex, HashMap<NodeIndex, EdgeData>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeData {
    Embedded {
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

// TODO: remove this trait once we settle on an ERD format
pub trait GetErdData {
    fn get_relationship(
        &self,
        source_entity_name: &str,
        target_entity_name: &str,
    ) -> Option<&ErdRelationship>;

    fn get_source(&self, entity_name: &str) -> Option<&Source>;

    fn iter(&self) -> impl Iterator<Item = &String>;

    fn size(&self) -> usize;
}

impl GetErdData for Erd {
    fn get_relationship(
        &self,
        source_entity_name: &str,
        target_entity_name: &str,
    ) -> Option<&ErdRelationship> {
        self.get_relationship(source_entity_name, target_entity_name)
    }

    fn get_source(&self, entity_name: &str) -> Option<&Source> {
        self.get_source(entity_name)
    }

    fn iter(&self) -> impl Iterator<Item = &String> {
        self.iter()
            .map(|(name, _)| name)
    }

    fn size(&self) -> usize {
        self.size()
    }
}

impl GetErdData for Relationships {
    fn get_relationship(
        &self,
        source_entity_name: &str,
        target_entity_name: &str,
    ) -> Option<&ErdRelationship> {
        self.get_relationship(source_entity_name, target_entity_name)
    }

    fn get_source(&self, _entity_name: &str) -> Option<&Source> {
        None
    }

    fn iter(&self) -> impl Iterator<Item = &String> {
        self.iter()
            .map(|(name, _)| name)
    }

    fn size(&self) -> usize {
        self.size()
    }
}

impl ErdGraph {
    pub fn new<T>(erd: &T) -> Self
    where 
        T: GetErdData
    {
        let mut graph = DiGraph::default();
        let mut node_indices = HashMap::new();
        let mut edge_data: HashMap<_, HashMap<_, _>> = HashMap::new();

        // Add entities as nodes
        for entity_name in erd.iter() {
            // TODO: we may not want to add the names as node labels for efficiency
            let node_index = graph.add_node(entity_name.to_string());
            node_indices.insert(entity_name.to_string(), node_index);
        }

        // convert to Vec to induce a stable order
        let node_indices_vec: Vec<_> = node_indices.iter().map(|(s, n)| (s, *n)).collect();
        for (source_entity_name, source_index) in node_indices_vec.iter() {
            for (target_entity_name, target_index) in node_indices_vec.iter() {
                if source_entity_name == target_entity_name {
                    continue; // Skip self-loops
                }
                if let Some((weight, constraint)) =
                    get_edge_from_data(erd, source_entity_name, target_entity_name)
                {
                    graph.add_edge(*source_index, *target_index, weight);
                    edge_data
                        .entry(*source_index)
                        .or_default()
                        .insert(*target_index, constraint);
                }
            }
        }
        Self {
            graph,
            node_indices,
            edge_data,
        }
    }

    pub fn get_entity_name(&self, node_index: NodeIndex) -> Option<&String> {
        self.graph.node_weight(node_index)
    }

    pub fn get_index(&self, entity_name: &str) -> Option<NodeIndex> {
        self.node_indices.get(entity_name).cloned()
    }

    pub fn get_edge_data_by_names(
        &self,
        source_entity_name: &str,
        target_entity_name: &str,
    ) -> Option<&EdgeData> {
        let source_index = self.node_indices.get(source_entity_name)?;
        let target_index = self.node_indices.get(target_entity_name)?;
        self.get_edge_data(*source_index, *target_index)
    }

    pub fn get_edge_data(
        &self,
        source_index: NodeIndex,
        target_index: NodeIndex,
    ) -> Option<&EdgeData> {
        self.edge_data
            .get(&source_index)
            .and_then(|edges| edges.get(&target_index))
    }

    pub fn path_to(
        &self,
        source_index: NodeIndex,
        target_index: NodeIndex,
    ) -> Option<Vec<NodeIndex>> {
        let path = algo::astar(
            &self.graph,
            source_index,
            |finish| finish == target_index,
            |e| *e.weight(),
            |_| 0,
        )?
        .1;
        Some(path)
    }

    pub fn path_to_by_names(
        &self,
        source_entity_name: &str,
        target_entity_name: &str,
    ) -> Option<Vec<String>> {
        let source_index = self.node_indices.get(source_entity_name)?;
        let target_index = self.node_indices.get(target_entity_name)?;
        self.path_to(*source_index, *target_index).map(|path| {
            path.iter()
                .filter_map(|&index| self.get_entity_name(index).cloned())
                .collect()
        })
    }
}

impl std::fmt::Display for ErdGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", Dot::with_config(&self.graph, &[]))
    }
}

fn get_edge_from_data<T>
(
    ed: &T,
    source_entity_name: &str,
    target_entity_name: &str,
) -> Option<(usize, EdgeData)> 
where 
    T: GetErdData,
{
    let get_relationship_weight = |relationship_type, constraint_type| {
        match (relationship_type, constraint_type) {
            // datas should actually be based of cardinality with a large constant factor for
            // being foreign vs embeded
            (ConstraintType::Embedded, RelationshipType::OneToOne) => 1,
            (ConstraintType::Embedded, RelationshipType::ManyToOne) => 2,
            (ConstraintType::Embedded, RelationshipType::ManyToMany) => 4,
            (ConstraintType::Foreign, RelationshipType::OneToOne) => ed.size(),
            (ConstraintType::Foreign, RelationshipType::ManyToOne) => ed.size() * 2,
            (ConstraintType::Foreign, RelationshipType::ManyToMany) => ed.size() * 4,
        }
    };
    let get_relationship_constraint = |entity_name: &str, relationship: &ErdRelationship| {
        match relationship.constraint.constraint_type {
            ConstraintType::Embedded => EdgeData::Embedded {
                source_entity: entity_name.to_string(),
                target_path: relationship
                    .constraint
                    .target_path
                    .clone()
                    .unwrap_or_default(),
                relationship_type: relationship.relationship_type, // Default to ManyToOne if no relationship found
            },
            ConstraintType::Foreign => {
                EdgeData::Foreign {
                    db: relationship.constraint.db.clone().unwrap(),
                    collection: relationship.constraint.collection.clone().unwrap(),
                    relationship_type: relationship.relationship_type,
                    foreign_key: relationship.constraint.foreign_key.clone().unwrap(),
                    local_key: relationship.constraint.local_key.clone().unwrap(),
                }
            }
        }
    };
    if let Some(relationship) = ed.get_relationship(source_entity_name, target_entity_name) {
        Some((
            get_relationship_weight(
                relationship.constraint.constraint_type,
                relationship.relationship_type,
            ),
            get_relationship_constraint(source_entity_name, relationship),
        ))
    } else {
        None
    }
}
