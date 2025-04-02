use ast::{
    definitions::{
        visitor::Visitor, Assemble, AssembleJoinType, Expression, Lookup, LookupFrom, MatchExpr,
        MatchExpression, MatchStage, Pipeline, ProjectItem, ProjectStage, Ref, Stage, Subassemble,
        SubqueryLookup, Unwind, UnwindExpr,
    },
    map,
};
use linked_hash_map::LinkedHashMap;
use schema::{ConstraintType, Direction, Entity, Erd};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Could not find ERD path: {0}")]
    CouldNotFindErd(String),
    #[error("Could not parse ERD: {0}")]
    CouldNotParseErd(#[from] serde_json::Error),
    #[error("Entity: {0} missing from ERD")]
    EntityMissingFromErd(String),
    #[error("Missing filter in subassemble: {0}")]
    MissingFilterInSubassemble(String),
    #[error("Missing key in filter, key: {0}, filter: {1}")]
    MissingKeyInFilter(String, String),
    #[error("Storage constraints for field {0} not found in entity: {1}")]
    StorageConstraintsNotFoundInEntity(String, String),
    #[error("No references in ERD for entity: {0}")]
    NoReferencesInErd(String),
    #[error("Reference key not found: {0}")]
    ReferenceKeyNotFound(String),
    #[error("Embedded constraints must have targetPath. missing targetPath for field: {0}")]
    MissingTargetPathInEmbedded(String),
    #[error("Project key {0} not found in entity: {1}")]
    ProjectKeyNotFound(String, String),
    #[error("Field in filter has no entity: {0}, filter: {1}")]
    FieldInFilterHasNoEntity(String, String),
    #[error("Field {0} not found in entity: {1}")]
    FieldNotFoundInEntity(String, String),
    #[error("No constraints implied by filter: {0}")]
    NoConstraintsImpliedByFilter(String),
    #[error("Entity {0} not in scope")]
    EntityNotInScope(String),
    #[error("Disagreeing constraint types for fields in subassemble filter")]
    DisagreeingConstraintTypes,
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct AssembleRewrite {
    error: Option<Error>,
}

pub fn rewrite_pipeline(pipeline: Pipeline) -> Result<Pipeline> {
    let mut visitor = AssembleRewrite { error: None };
    let pipeline = visitor.visit_pipeline(pipeline);
    if let Some(e) = visitor.error {
        Err(e)
    } else {
        Ok(pipeline)
    }
}

macro_rules! print_json {
    ($v:expr) => {
        serde_json::to_string_pretty($v).unwrap()
    };
}

impl Visitor for AssembleRewrite {
    // visit_stage is here to handle Assemble stages and replace them with SubPipelines
    fn visit_stage(&mut self, stage: Stage) -> Stage {
        if self.error.is_some() {
            return Stage::SubPipeline(Pipeline {
                pipeline: Vec::new(),
            });
        }
        macro_rules! handle_error {
            ($e:expr) => {
                match $e {
                    Err(e) => {
                        self.error = Some(e);
                        return Stage::SubPipeline(Pipeline {
                            pipeline: Vec::new(),
                        });
                    }
                    Ok(v) => v,
                }
            };
        }
        match stage {
            Stage::Assemble(mut a) => {
                let erd_json = handle_error!(std::fs::read_to_string(&a.erd)
                    .map_err(|_| Error::CouldNotFindErd(a.erd.clone())));
                let erd: Erd =
                    handle_error!(serde_json::from_str(&erd_json).map_err(Error::CouldNotParseErd));
                let entities = erd.entities;
                let entity_graph = handle_error!(build_entity_graph(&a, &entities));
                let mut output = vec![Stage::Project(ProjectStage {
                    items: map! {
                        a.entity.clone() => ProjectItem::Assignment(Expression::Ref(Ref::VariableRef("ROOT".to_string()))),
                        "_id".to_string() => ProjectItem::Exclusion,
                    },
                })];
                if let Some(ref filter) = a.filter {
                    output.push(Stage::Match(MatchStage {
                        expr: vec![MatchExpression::Expr(MatchExpr {
                            expr: Box::new(filter.clone()),
                        })],
                        numbering: None,
                    }));
                }
                let project_keys = handle_error!(check_and_collect_project_keys(&a, &entities));
                let subassembles = std::mem::take(&mut a.subassemble);
                //let mut mapping_registry = map! {
                //    a.entity.clone() => Expression::Ref(Ref::FieldRef(a.entity.clone())),
                //};
                for assemble in subassembles {
                    let pipeline = handle_error!(generate_subassemble(
                        &a.entity,
                        assemble,
                        &entity_graph,
                        &entities
                    ));
                    output.push(pipeline);
                }
                if let Some(f) = a.filter {
                    output.push(Stage::Match(MatchStage {
                        expr: vec![MatchExpression::Expr(MatchExpr { expr: Box::new(f) })],
                        numbering: None,
                    }));
                }
                output.push(generate_project(project_keys));
                Stage::SubPipeline(Pipeline { pipeline: output })
            }
            _ => stage,
        }
    }

    // visit_pipeline is here to flatten out SubPipelines introduced as replacements
    // for Assemble stages
    fn visit_pipeline(&mut self, pipeline: Pipeline) -> Pipeline {
        Pipeline {
            pipeline: pipeline
                .pipeline
                .into_iter()
                .flat_map(|stage| match self.visit_stage(stage) {
                    Stage::SubPipeline(sub_pipeline) => sub_pipeline.pipeline,
                    stage => vec![stage],
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Constraint {
    constraint_type: ConstraintType,
    target_path: Option<String>,
    direction: Direction,
}

impl Constraint {
    fn inverse(&self) -> Constraint {
        Constraint {
            constraint_type: self.constraint_type,
            target_path: self.target_path.clone(),
            direction: self.direction.inverse(),
        }
    }
}

fn build_entity_graph(
    assemble: &Assemble,
    entities: &HashMap<String, Entity>,
) -> Result<HashMap<String, HashMap<String, Constraint>>> {
    let mut entity_graph = HashMap::new();
    build_entity_graph_aux(
        assemble.entity.as_str(),
        Some(assemble.subassemble.as_slice()),
        entities,
        &mut entity_graph,
    )?;
    let mut reverse_graph = HashMap::new();
    for (source_name, graph) in entity_graph.iter() {
        for (target_name, constraint) in graph.iter() {
            if let Some(target_graph) = entity_graph.get(target_name) {
                if let Some(target_constraint) = target_graph.get(source_name) {
                    if target_constraint.constraint_type != constraint.constraint_type {
                        return Err(Error::DisagreeingConstraintTypes);
                    }
                } else {
                    reverse_graph
                        .entry(target_name.clone())
                        .or_insert_with(HashMap::new)
                        .insert(source_name.clone(), constraint.inverse());
                }
            }
        }
    }
    Ok(graph_union(entity_graph, reverse_graph))
}

fn graph_union<T>(
    mut a: HashMap<String, HashMap<String, T>>,
    b: HashMap<String, HashMap<String, T>>,
) -> HashMap<String, HashMap<String, T>> {
    for (k, v) in b {
        a.entry(k).or_insert_with(HashMap::new).extend(v);
    }
    a
}

fn build_entity_graph_aux(
    entity_name: &str,
    subassembles: Option<&[Subassemble]>,
    entities: &HashMap<String, Entity>,
    entity_graph: &mut HashMap<String, HashMap<String, Constraint>>,
) -> Result<()> {
    let entity = entities
        .get(entity_name)
        .ok_or(Error::EntityMissingFromErd(entity_name.to_string()))?;
    let references = entity
        .get_references()
        .ok_or(Error::NoReferencesInErd(entity_name.to_string()))?;
    let current_entity_graph = entity_graph
        .entry(entity_name.to_string())
        .or_insert_with(HashMap::new);
    for (field, reference) in references.iter() {
        let storage_constraint = reference.storage_constraints.first().ok_or(
            Error::StorageConstraintsNotFoundInEntity(field.clone(), print_json!(entity)),
        )?;
        let target_path = storage_constraint.target_path.clone();
        let constraint = Constraint {
            constraint_type: storage_constraint.constraint_type,
            target_path,
            direction: storage_constraint.direction,
        };
        current_entity_graph.insert(reference.entity.clone(), constraint);
    }
    for subassemble in subassembles.into_iter().flatten() {
        build_entity_graph_aux(
            subassemble.entity.as_str(),
            subassemble.subassemble.as_ref().map(|x| x.as_slice()),
            entities,
            entity_graph,
        )?;
    }
    Ok(())
}

fn minimal_ordering(graph: &HashMap<String, HashMap<String, Constraint>>) -> Vec<String> {
    let mut ordering = Vec::new();
    let mut queue = Vec::new();
    let mut visited = HashSet::new();
    let root = graph
        .iter()
        .find(|(_, v)| v.iter().all(|(_, c)| c.direction == Direction::Parent))
        .unwrap()
        .0
        .clone();
    ordering.push(root.clone());
    queue.push(root.clone());
    visited.insert(root.clone());
    while !queue.is_empty() {
        for edge in graph
            .get(queue.pop().unwrap().as_str())
            .into_iter()
            .flatten()
        {
            if visited.contains(edge.0) {
                continue;
            }
            if edge.1.direction == Direction::Parent {
                ordering.push(edge.0.clone());
                queue.push(edge.0.clone());
                visited.insert(edge.0.clone());
            }
        }
    }
    ordering
}

fn check_and_collect_project_keys(
    assemble: &Assemble,
    entities: &HashMap<String, Entity>,
) -> Result<Vec<String>> {
    let mut ret = Vec::new();
    check_and_collect_project_keys_aux(
        assemble.entity.as_str(),
        assemble.project.as_slice(),
        Some(assemble.subassemble.as_slice()),
        entities,
        &mut ret,
    )?;
    Ok(ret)
}

fn check_and_collect_project_keys_aux(
    entity_name: &str,
    project: &[String],
    subassembles: Option<&[Subassemble]>,
    entities: &HashMap<String, Entity>,
    ret: &mut Vec<String>,
) -> Result<()> {
    let entity = entities
        .get(entity_name)
        .ok_or(Error::EntityMissingFromErd(entity_name.to_string()))?;
    for field in project {
        if !entity.can_contain_field(field.as_str()) {
            return Err(Error::ProjectKeyNotFound(
                field.to_string(),
                print_json!(entity),
            ));
        }
        ret.push(format!("{}.{}", entity_name, field));
    }
    for subassemble in subassembles.into_iter().flatten() {
        check_and_collect_project_keys_aux(
            subassemble.entity.as_str(),
            subassemble.project.as_slice(),
            subassemble.subassemble.as_ref().map(|x| x.as_slice()),
            entities,
            ret,
        )?;
    }
    Ok(())
}

fn generate_project(project: Vec<String>) -> Stage {
    let mut found_id = false;
    let mut project_items = project
        .into_iter()
        .map(|projection| {
            if projection == "_id" {
                found_id = true
            };
            (projection, ProjectItem::Inclusion)
        })
        .collect::<LinkedHashMap<_, _>>();
    if !found_id {
        project_items.insert("_id".to_string(), ProjectItem::Exclusion);
    }
    Stage::Project(ProjectStage {
        items: project_items,
    })
}

fn generate_subassemble(
    parent_entity: &str,
    subassemble: Subassemble,
    entity_graph: &HashMap<String, HashMap<String, Constraint>>,
    entities: &HashMap<String, Entity>,
) -> Result<Stage> {
    let edge_constraint = entity_graph
        .get(parent_entity)
        .ok_or(Error::EntityNotInScope(parent_entity.to_string()))?
        .get(subassemble.entity.as_str())
        .ok_or(Error::EntityNotInScope(subassemble.entity.clone()))?;
    let pipeline = vec![match edge_constraint.constraint_type {
        ConstraintType::Reference => {
            handle_reference_constraint(subassemble, edge_constraint, entity_graph, entities)
        }
        ConstraintType::Embedded => {
            handle_embedded_constraint(subassemble, edge_constraint, entity_graph, entities)
        }
        ConstraintType::Bucket => {
            todo!();
        }
    }?];
    Ok(Stage::SubPipeline(Pipeline { pipeline }))
}

fn generate_lookup_pipeline(
    subassemble: Subassemble,
    entity_graph: &HashMap<String, HashMap<String, Constraint>>,
    entities: &HashMap<String, Entity>,
) -> Result<Vec<Stage>> {
    // replace parent entity fieldRefs with variableRefs. These can be potentially
    // optimized out with movement, but they may not be if a given conjunctive
    // subexpression also uses the child entity
    let theta = HashMap::new();
    let mut lookup_pipeline = vec![Stage::Project(ProjectStage {
        items: map! {
            subassemble.entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::VariableRef("ROOT".to_string()))),
            "_id".to_string() => ProjectItem::Exclusion,
        },
    })];
    if let Some(filter) = subassemble.filter {
        lookup_pipeline.push(Stage::Match(MatchStage {
            expr: vec![MatchExpression::Expr(MatchExpr {
                expr: Box::new(filter.substitute(theta)),
            })],
            numbering: None,
        }));
    }
    // add recursive sub assemblies
    let parent_entity = subassemble.entity.to_string();
    for subassemble in subassemble.subassemble.iter().flatten() {
        lookup_pipeline.push(generate_subassemble(
            parent_entity.as_str(),
            subassemble.clone(),
            entity_graph,
            entities,
        )?)
    }
    Ok(lookup_pipeline)
}

fn handle_reference_constraint(
    subassemble: Subassemble,
    constraint: &Constraint,
    entity_graph: &HashMap<String, HashMap<String, Constraint>>,
    entities: &HashMap<String, Entity>,
) -> Result<Stage> {
    let mut pipeline = Vec::new();
    // all of the grandparent_entities are in scope as variables, the parent entity
    // is in scope as a field
    let entity_name = subassemble.entity.to_string();
    let join_type = subassemble.join.unwrap_or(AssembleJoinType::Inner);
    let subassemble_entity = entities
        .get(subassemble.entity.as_str())
        .ok_or(Error::EntityMissingFromErd(subassemble.entity.clone()))?;
    let collection = subassemble_entity.collection.clone();
    let lookup_pipeline = generate_lookup_pipeline(subassemble, entity_graph, entities)?;
    pipeline.push(Stage::Lookup(Lookup::Subquery(SubqueryLookup {
        from: Some(LookupFrom::Collection(collection)),
        let_body: None,
        pipeline: Pipeline {
            pipeline: lookup_pipeline,
        },
        as_var: entity_name.clone(),
    })));
    pipeline.push(
                Stage::Project(
                    ProjectStage {
                        items: map! {
                            entity_name.clone() => ProjectItem::Assignment(Expression::Ref(Ref::FieldRef(format!("{}.{}", entity_name, entity_name)))),
                        }
                    }
                )
            );
    pipeline.push(Stage::Unwind(Unwind::Document(UnwindExpr {
        path: Box::new(Expression::Ref(Ref::FieldRef(entity_name.to_string()))),
        preserve_null_and_empty_arrays: Some(join_type == AssembleJoinType::Left),
        include_array_index: None,
    })));
    Ok(Stage::SubPipeline(Pipeline { pipeline }))
}

fn handle_embedded_constraint(
    subassemble: Subassemble,
    constraint: &Constraint,
    entity_graph: &HashMap<String, HashMap<String, Constraint>>,
    entities: &HashMap<String, Entity>,
) -> Result<Stage> {
    let mut pipeline = Vec::new();
    let target_path = constraint
        .target_path
        .as_ref()
        .ok_or_else(|| Error::MissingTargetPathInEmbedded(print_json!(constraint)))?;
    pipeline.push(Stage::Unwind(Unwind::Document(UnwindExpr {
        path: Box::new(Expression::Ref(Ref::FieldRef(target_path.clone()))),
        preserve_null_and_empty_arrays: Some(subassemble.join == Some(AssembleJoinType::Left)),
        include_array_index: None,
    })));
    pipeline.push(Stage::Project(ProjectStage {
                items: map! {
                    subassemble.entity.clone() => ProjectItem::Assignment(Expression::Ref(Ref::FieldRef(target_path.to_string()))),
                    "_id".to_string() => ProjectItem::Exclusion,
                },
            }));
    if let Some(filter) = subassemble.filter {
        pipeline.push(Stage::Match(MatchStage {
            expr: vec![MatchExpression::Expr(MatchExpr {
                expr: Box::new(filter.clone()),
            })],
            numbering: None,
        }));
    }
    // add recursive sub assemblies
    let parent_entity = subassemble.entity.to_string();
    for subassemble in subassemble.subassemble.into_iter().flatten() {
        pipeline.push(generate_subassemble(
            parent_entity.as_str(),
            subassemble,
            entity_graph,
            entities,
        )?)
    }
    Ok(Stage::SubPipeline(Pipeline { pipeline }))
}
