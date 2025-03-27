use ast::{
    definitions::{
        visitor::Visitor, Assemble, AssembleJoinType, EqualityLookup, Expression, Lookup,
        LookupFrom, MatchExpr, MatchExpression, MatchStage, Pipeline, ProjectItem, ProjectStage,
        Ref, Stage, Subassemble, SubqueryLookup, Unwind, UnwindExpr,
    },
    map, set,
};
use linked_hash_map::LinkedHashMap;
use schema::{ConstraintType, Direction, Entity, Erd, Relationship};
use std::collections::{HashMap, HashSet};
use tailcall::tailcall;
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
    #[error("Embedded constraints must have targetPath: {0}")]
    MissingTargetPathInEmbedded(String),
    #[error("Project key {0} not found in entity: {1}")]
    ProjectKeyNotFound(String, String),
    #[error("Field in filter has no entity: {0}, filter: {1}")]
    FieldInFilterHasNoEntity(String, String),
    #[error("Field {0} not found in entity: {1}")]
    FieldNotFoundInEntity(String, String),
    #[error("Entity {0} not in scope")]
    EntityNotInScope(String),
    #[error("Disagreeing constraint types for fields in subassemble filter")]
    DisagreeingConstraintTypes,
    #[error("No targetPath when embedded constraint is in use {0}")]
    MissingTargetPathInReference(String),
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
                        set! {a.entity.to_string()},
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
    parent_entities: HashSet<String>,
    subassemble: Subassemble,
    entities: &HashMap<String, Entity>,
) -> Result<Stage> {
    let mut pipeline = Vec::new();
    if subassemble.filter.is_none() {
        return Err(Error::MissingFilterInSubassemble(
            subassemble.entity.clone(),
        ));
    }
    let filter = subassemble.filter.unwrap();
    let mut filter_uses = HashMap::new();
    for u in filter.uses().into_iter() {
        let u_split: Vec<_> = u.split('.').map(|x| x.to_string()).collect();
        if u_split.len() < 2 {
            return Err(Error::FieldInFilterHasNoEntity(u, print_json!(&filter)));
        }
        let entity_name = &u_split[0];
        if !parent_entities.contains(entity_name) && entity_name != subassemble.entity.as_str() {
            return Err(Error::EntityNotInScope(entity_name.to_string()));
        }
        let field = u_split[1..].join(".");
        if !filter_uses.contains_key(entity_name) {
            filter_uses.insert(entity_name.clone(), vec![field]);
        } else {
            filter_uses.get_mut(entity_name).unwrap().push(field);
        }
    }
    if filter_uses.len() > 2 {
        todo!("Implement multi-entity filters");
    }

    let input_entity_names: Vec<_> = filter_uses
        .keys()
        .filter_map(|x| {
            if **x != subassemble.entity {
                Some(x.clone())
            } else {
                None
            }
        })
        .collect();

    if input_entity_names.len() != 1 {
        todo!("Implement multi-entity filters");
    }

    let input_entity_name = input_entity_names.first().unwrap();

    let input_entity = entities
        .get(input_entity_name.as_str())
        .ok_or_else(|| Error::EntityMissingFromErd(input_entity_name.clone()))?;
    let subassemble_entity = entities
        .get(subassemble.entity.as_str())
        .ok_or_else(|| Error::EntityMissingFromErd(subassemble.entity.clone()))?;
    let input_references = input_entity
        .get_references()
        .ok_or_else(|| Error::NoReferencesInErd(input_entity_name.clone()))?;
    let input_fields = filter_uses.get(input_entity_name.as_str()).ok_or_else(|| {
        Error::MissingKeyInFilter(input_entity_name.clone(), print_json!(&filter))
    })?;
    let subassemble_references = subassemble_entity
        .get_references()
        .ok_or_else(|| Error::NoReferencesInErd(input_entity_name.clone()))?;
    let subassemble_fields = filter_uses
        .get(subassemble.entity.as_str())
        .ok_or_else(|| {
            Error::MissingKeyInFilter(subassemble.entity.clone(), print_json!(&filter))
        })?;
    let mut constraints = Vec::new();
    for field in input_fields {
        if let Some(r) = input_references.get(field.as_str()) {
            let storage_constraint = r.storage_constraints.first().ok_or_else(|| {
                Error::StorageConstraintsNotFoundInEntity(field.clone(), print_json!(&input_entity))
            })?;
            let target_path = storage_constraint.target_path.clone();
            constraints.push((storage_constraint.constraint_type, target_path));
        }
    }
    for field in subassemble_fields {
        if let Some(r) = subassemble_references.get(field.as_str()) {
            let storage_constraint = r.storage_constraints.first().ok_or_else(|| {
                Error::StorageConstraintsNotFoundInEntity(
                    field.clone(),
                    print_json!(&subassemble_entity),
                )
            })?;
            let target_path = storage_constraint.target_path.clone();
            constraints.push((storage_constraint.constraint_type, target_path));
        }
    }
    for (constraint_type, target_path) in constraints {
        if constraint_type == ConstraintType::Reference {
            let collection = subassemble_entity.collection.clone();
            pipeline.push(Stage::Lookup(Lookup::Subquery(SubqueryLookup {
            from: Some(LookupFrom::Collection(collection)),
            let_body: Some(map! {input_entity_name.clone() => Expression::Ref(Ref::FieldRef(input_entity_name.clone()))}),
            pipeline: Pipeline { pipeline: vec![
                Stage::Project(ProjectStage {
                    items: map! {
                        subassemble.entity.clone() => ProjectItem::Assignment(Expression::Ref(Ref::VariableRef("ROOT".to_string()))),
                        "_id".to_string() => ProjectItem::Exclusion,
                    },
                }),
                Stage::Match(MatchStage {
                    expr: vec![MatchExpression::Expr(MatchExpr {
                        expr: Box::new(filter.clone().substitute(map! {input_entity_name.clone() => Expression::Ref(Ref::VariableRef(input_entity_name.clone()))})),
                    })],
                    numbering: None,
                }),
            ] },
            as_var: subassemble.entity.clone(),
        })));
            pipeline.push(Stage::Unwind(Unwind::Document(UnwindExpr {
                path: Box::new(Expression::Ref(Ref::FieldRef(subassemble.entity.clone()))),
                preserve_null_and_empty_arrays: Some(
                    subassemble.join == Some(AssembleJoinType::Left),
                ),
                include_array_index: None,
            })));
        } else if constraint_type == ConstraintType::Embedded {
            let target_path = target_path
                .ok_or_else(|| Error::MissingTargetPathInReference(print_json!(input_entity)))?;
            pipeline.push(Stage::Unwind(Unwind::Document(UnwindExpr {
                path: Box::new(Expression::Ref(Ref::FieldRef(target_path.clone()))),
                preserve_null_and_empty_arrays: Some(
                    subassemble.join == Some(AssembleJoinType::Left),
                ),
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
        } else {
            todo!("Implement other constraint types");
        }
    }
    Ok(Stage::SubPipeline(Pipeline { pipeline: pipeline }))
}
