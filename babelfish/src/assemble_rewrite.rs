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
                dbg!(entity_graph);
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
                for assemble in subassembles {
                    output.push(handle_error!(generate_subassemble(
                        HashSet::new(),
                        a.entity.as_str(),
                        assemble,
                        &entities
                    )));
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    );
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
    let current_entity_graph = if let Some(current_entity_graph) = entity_graph.get_mut(entity_name)
    {
        current_entity_graph
    } else {
        entity_graph.insert(entity_name.to_string(), HashMap::new());
        entity_graph.get_mut(entity_name).unwrap()
    };
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

fn get_filter_uses(filter: &Expression) -> Result<HashMap<String, Vec<String>>> {
    let mut filter_uses = HashMap::new();
    for u in filter.uses().into_iter() {
        let u_split: Vec<_> = u.split('.').map(|x| x.to_string()).collect();
        if u_split.len() < 2 {
            return Err(Error::FieldInFilterHasNoEntity(u, print_json!(&filter)));
        }
        let entity_name = &u_split[0];
        let field = u_split[1..].join(".");
        if !filter_uses.contains_key(entity_name) {
            filter_uses.insert(entity_name.clone(), vec![field]);
        } else {
            filter_uses.get_mut(entity_name).unwrap().push(field);
        }
    }
    Ok(filter_uses)
}

fn gather_constraints(
    filter_uses: HashMap<String, Vec<String>>,
    entities: &HashMap<String, Entity>,
    subassemble_entity: &Entity,
) -> Result<HashSet<(ConstraintType, Option<String>, String)>> {
    // Gather constraints using a set to avoid duplicates
    let mut constraints = HashSet::new();
    for (entity_name, fields) in filter_uses.into_iter() {
        let entity = entities
            .get(&entity_name)
            .ok_or_else(|| Error::EntityMissingFromErd(entity_name.clone()))?;
        let references = entity
            .get_references()
            .ok_or_else(|| Error::NoReferencesInErd(entity_name.clone()))?;
        for field in fields {
            if let Some(r) = references.get(&field) {
                let storage_constraint = r.storage_constraints.first().ok_or_else(|| {
                    Error::StorageConstraintsNotFoundInEntity(
                        field.clone(),
                        print_json!(subassemble_entity),
                    )
                })?;
                let target_path = storage_constraint
                    .target_path
                    .as_ref()
                    .map(|tp| format!("{entity_name}.{tp}"));
                constraints.insert((storage_constraint.constraint_type, target_path, field));
            }
        }
    }
    Ok(constraints)
}

fn generate_lookup_pipeline(
    grandparent_entities: &HashSet<String>,
    parent_entity: &str,
    subassemble: &Subassemble,
    filter: &Expression,
    entities: &HashMap<String, Entity>,
) -> Result<Vec<Stage>> {
    // replace parent entity fieldRefs with variableRefs. These can be potentially
    // optimized out with movement, but they may not be if a given conjunctive
    // subexpression also uses the child entity
    let theta = grandparent_entities
        .iter()
        .map(|n| (n.clone(), Expression::Ref(Ref::VariableRef(n.clone()))))
        .chain(std::iter::once((
            parent_entity.to_string(),
            Expression::Ref(Ref::VariableRef(parent_entity.to_string())),
        )))
        .collect();
    let mut lookup_pipeline = vec![
        Stage::Project(ProjectStage {
            items: map! {
                subassemble.entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::VariableRef("ROOT".to_string()))),
                "_id".to_string() => ProjectItem::Exclusion,
            },
        }),
        Stage::Match(MatchStage {
            expr: vec![MatchExpression::Expr(MatchExpr {
                expr: Box::new(filter.clone().substitute(theta)),
            })],
            numbering: None,
        }),
    ];
    // add recursive sub assemblies
    let parent_entity = subassemble.entity.to_string();
    for subassemble in subassemble.subassemble.iter().flatten() {
        lookup_pipeline.push(generate_subassemble(
            grandparent_entities.clone(),
            parent_entity.as_str(),
            subassemble.clone(),
            entities,
        )?)
    }
    Ok(lookup_pipeline)
}

fn handle_reference_constraint(
    grandparent_entities: &HashSet<String>,
    parent_entity: &str,
    subassemble: &Subassemble,
    subassemble_entity: &Entity,
    filter: &Expression,
    entities: &HashMap<String, Entity>,
) -> Result<Stage> {
    let mut pipeline = Vec::new();
    // all of the grandparent_entities are in scope as variables, the parent entity
    // is in scope as a field
    let let_map = grandparent_entities
        .iter()
        .map(|n| (n.clone(), Expression::Ref(Ref::VariableRef(n.clone()))))
        .chain(std::iter::once((
            parent_entity.to_string(),
            Expression::Ref(Ref::FieldRef(parent_entity.to_string())),
        )))
        .collect();
    let collection = subassemble_entity.collection.clone();
    let lookup_pipeline = generate_lookup_pipeline(
        grandparent_entities,
        parent_entity,
        subassemble,
        filter,
        entities,
    )?;
    pipeline.push(Stage::Lookup(Lookup::Subquery(SubqueryLookup {
        from: Some(LookupFrom::Collection(collection)),
        let_body: Some(let_map),
        pipeline: Pipeline {
            pipeline: lookup_pipeline,
        },
        as_var: subassemble.entity.to_string(),
    })));
    pipeline.push(
                Stage::Project(
                    ProjectStage {
                        items: map! {
                            subassemble.entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::FieldRef(format!("{}.{}", subassemble.entity, subassemble.entity)))),
                        }
                    }
                )
            );
    let join = subassemble.join.unwrap_or(AssembleJoinType::Inner);
    pipeline.push(Stage::Unwind(Unwind::Document(UnwindExpr {
        path: Box::new(Expression::Ref(Ref::FieldRef(subassemble.entity.clone()))),
        preserve_null_and_empty_arrays: Some(join == AssembleJoinType::Left),
        include_array_index: None,
    })));
    Ok(Stage::SubPipeline(Pipeline { pipeline }))
}

fn handle_embedded_constraint(
    grandparent_entities: &HashSet<String>,
    target_path: Option<String>,
    field: &str,
    subassemble: &Subassemble,
    filter: &Expression,
    entities: &HashMap<String, Entity>,
) -> Result<Stage> {
    let mut pipeline = Vec::new();
    let target_path =
        target_path.ok_or_else(|| Error::MissingTargetPathInEmbedded(print_json!(field)))?;
    pipeline.push(Stage::Unwind(Unwind::Document(UnwindExpr {
        path: Box::new(Expression::Ref(Ref::FieldRef(target_path.clone()))),
        preserve_null_and_empty_arrays: Some(subassemble.join == Some(AssembleJoinType::Left)),
        include_array_index: None,
    })));
    pipeline.push(Stage::Project(ProjectStage {
                items: map! {
                    subassemble.entity.clone() => ProjectItem::Assignment(Expression::Ref(Ref::FieldRef(target_path))),
                    "_id".to_string() => ProjectItem::Exclusion,
                },
            }));
    pipeline.push(Stage::Match(MatchStage {
        expr: vec![MatchExpression::Expr(MatchExpr {
            expr: Box::new(filter.clone()),
        })],
        numbering: None,
    }));
    // add recursive sub assemblies
    let parent_entity = subassemble.entity.to_string();
    for subassemble in subassemble.subassemble.iter().flatten() {
        pipeline.push(generate_subassemble(
            grandparent_entities.clone(),
            parent_entity.as_str(),
            subassemble.clone(),
            entities,
        )?)
    }
    Ok(Stage::SubPipeline(Pipeline { pipeline }))
}

fn generate_subassemble(
    mut grandparent_entities: HashSet<String>,
    parent_entity: &str,
    subassemble: Subassemble,
    entities: &HashMap<String, Entity>,
) -> Result<Stage> {
    let mut pipeline = Vec::new();
    if subassemble.filter.is_none() {
        return Err(Error::MissingFilterInSubassemble(
            subassemble.entity.clone(),
        ));
    }
    let subassemble_entity = entities
        .get(subassemble.entity.as_str())
        .ok_or_else(|| Error::EntityMissingFromErd(subassemble.entity.clone()))?;
    let filter = subassemble.filter.clone().unwrap();

    let filter_uses = get_filter_uses(&filter)?;

    let constraints = gather_constraints(filter_uses, entities, subassemble_entity)?;
    if constraints.is_empty() {
        return Err(Error::NoConstraintsImpliedByFilter(print_json!(&filter)));
    }

    grandparent_entities.insert(parent_entity.to_string());
    // generate stages implied by constraints
    for (constraint_type, target_path, field) in constraints {
        // TODO: we may want to not clone the whole thing here, only some pieces really need cloned
        let subassemble = subassemble.clone();
        if constraint_type == ConstraintType::Reference {
            pipeline.push(handle_reference_constraint(
                &grandparent_entities,
                parent_entity,
                &subassemble,
                subassemble_entity,
                &filter,
                entities,
            )?);
        } else if constraint_type == ConstraintType::Embedded {
            pipeline.push(handle_embedded_constraint(
                &grandparent_entities,
                target_path,
                field.as_str(),
                &subassemble,
                &filter,
                entities,
            )?);
        } else {
            todo!("Implement other constraint types");
        }
    }
    Ok(Stage::SubPipeline(Pipeline { pipeline }))
}
