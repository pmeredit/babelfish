use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use crate::erd::{ConstraintType, Erd, ErdRelationship, RelationshipType};
use petgraph::{algo::steiner_tree, dot::Dot, graph::{NodeIndex, UnGraph}, prelude::StableUnGraph};

pub struct ErdGraph {
    pub graph: UnGraph<String, usize>,
    pub node_indices: HashMap<String, NodeIndex>,
    pub edge_data: HashMap<NodeIndex, HashMap<NodeIndex, EdgeData>>,
}

pub struct SteinerTree<'a> {
    pub graph: StableUnGraph<String, usize>,
    pub root: NodeIndex,
    pub node_indices: &'a HashMap<String, NodeIndex>,
    pub edge_data: &'a HashMap<NodeIndex, HashMap<NodeIndex, EdgeData>>,
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

impl ErdGraph {
    pub fn new(erd: &Erd) -> Self {
        let mut graph = UnGraph::default();
        let mut node_indices = HashMap::new();
        let mut edge_data: HashMap<_, HashMap<_,_>> = HashMap::new();
        
        // Add entities as nodes
        for (entity_name, _) in erd.iter() {
            // TODO: we may not want to add the names as node labels for efficiency
            let node_index = graph.add_node(entity_name.to_string());
            node_indices.insert(entity_name.to_string(), node_index);
        }
        
        let node_indices_vec: Vec<_> = node_indices.iter().map(|(s, n)|(s, *n)).collect();
        for (i, (source_entity_name, source_index)) in node_indices_vec.iter().enumerate() {
            for (target_entity_name, target_index) in node_indices_vec.iter().skip(i+1) {
                let (weight, constraint) = get_edge_data(erd, source_entity_name, target_entity_name);
                graph.add_edge(*source_index, *target_index, weight);
                edge_data.entry(*source_index)
                    .or_default()
                    .insert(*target_index, constraint);
            }
        }
        Self { graph, node_indices, edge_data }
    }

    pub fn get_entity_name(&self, node_index: NodeIndex) -> Option<&String> {
        self.graph.node_weight(node_index)
    }

    pub fn get_steiner_tree(&self, entities: &[String]) -> SteinerTree<'_> {
        let nodes: Vec<_> = entities.iter().map(|entity| {
            self.node_indices.get(entity).expect("Entity not found in node indices").clone()
        }).collect();
        let graph = steiner_tree::steiner_tree(
            &self.graph,
            nodes.as_slice(),
        );
        let mut root = NodeIndex::end();
        for edge in graph.edge_indices() {
            let (source, target) =  graph.edge_endpoints(edge).expect("Edge endpoints not found");
            let edge_data = self.get_edge_data(source, target)
                .expect("Edge data not found for steiner tree");
            println!("Edge: {:?} -> {:?}, Data: {:?}", 
                self.get_entity_name(source).unwrap(), 
                self.get_entity_name(target).unwrap(), 
                edge_data);
            match edge_data {
                EdgeData::EmbeddedSource { .. } | EdgeData::Foreign { .. } => {
                    root = target;
                    break;
                }
                _ => {}
            }
        }
        if root == NodeIndex::end() {
            println!("edge_data: {:?}", self.edge_data);
        }

        println!("Steiner tree root: {:?}", root);

        SteinerTree {
            graph,
            root,
            node_indices: &self.node_indices,
            edge_data: &self.edge_data,
        }
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

impl SteinerTree<'_> {
    pub fn get_edge_data(&self, source_index: NodeIndex, target_index: NodeIndex) -> Option<&EdgeData> {
        self.edge_data.get(&source_index)
            .and_then(|edges| edges.get(&target_index))
            .or_else(|| self.edge_data.get(&target_index).and_then(|edges| edges.get(&source_index)))
    }

    pub fn get_entity_name(&self, node_index: NodeIndex) -> Option<&String> {
        self.graph.node_weight(node_index)
    }

    pub fn topological_sort(&self) -> Vec<NodeIndex> {
        let mut ret = Vec::new();
        let mut visited = std::collections::HashSet::new();
        self.topo_aux(self.root, &mut ret, &mut visited);  
        println!("sorted nodes: {:?}", ret);
        println!("Topological sort: {:?}", ret.iter().map(|n| self.get_entity_name(*n).unwrap()).collect::<Vec<_>>());
        ret
    }

    fn topo_aux(&self, node: NodeIndex, ret: &mut Vec<NodeIndex>, visited: &mut HashSet<NodeIndex>) {
        if visited.contains(&node) {
            return;
        }
        ret.push(node);
        let mut worklist = Vec::new();
        for neighbor in self.graph.neighbors(node) {
            if visited.contains(&neighbor) {
                continue;
            }
            ret.push(neighbor);
            worklist.push(neighbor);
            visited.insert(neighbor);
        }
        for neighbor in worklist {
            self.topo_aux(neighbor, ret, visited);
        }
    }

    pub fn node_weight(&self, node_index: NodeIndex) -> Option<&String> {
        self.graph.node_weight(node_index)
    }
}

impl std::fmt::Display for ErdGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", Dot::with_config(&self.graph, &[]))
    }
}

impl std::fmt::Display for SteinerTree<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", Dot::with_config(&self.graph, &[]))
    }
}

fn get_edge_data(erd: &Erd, source_entity_name: &str, target_entity_name: &str) -> (usize, EdgeData) {
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
        return (
            get_relationship_weight(relationship.constraint.constraint_type, relationship.relationship_type),
            get_relationship_constraint(source_entity_name, relationship),
        );

    }
    if let Some(relationship) = erd.get_relationship(target_entity_name, source_entity_name) {
        return (
            get_relationship_weight(relationship.constraint.constraint_type, relationship.relationship_type),
            get_relationship_constraint(target_entity_name, relationship),
        );
    }
    if let Some(ref source) = erd.get_source(source_entity_name) {
        if let Some(ref target_path) = source.target_path {
            return (
                get_relationship_weight(ConstraintType::Embedded, RelationshipType::ManyToOne),
                EdgeData::EmbeddedSource {
                    db: source.db.clone(),
                    collection: source.collection.clone(),
                    target_path: target_path.clone(),
                },
            );
        }
        return (
            get_relationship_weight(ConstraintType::Foreign, RelationshipType::ManyToOne),
            EdgeData::Foreign {
                db: source.db.clone(),
                collection: source.collection.clone(),
                // TODO: Handle errors
                local_key: erd.get_primary_key(source_entity_name).unwrap().clone(),
                foreign_key: erd.get_primary_key(target_entity_name).unwrap().clone(),
                relationship_type: RelationshipType::ManyToOne, // Default to ManyToOne if no relationship found
            });
    }
    unreachable!() // No source found
}
