use ast::{
    definitions::{
        visitor::Visitor, AssembleJoinType, EqualityLookup, Expression, Lookup, LookupFrom,
        Pipeline, ProjectItem, ProjectStage, Ref, Stage, Subassemble, Unwind, UnwindExpr,
    },
    map,
};
use linked_hash_map::LinkedHashMap;
use schema::{ConstraintType, Direction, Entity, Erd, Relationship};
use std::collections::BTreeMap;
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
    #[error("Reference not found in Subassemble")]
    ReferenceNotFoundInSubassemble,
    #[error("Reference key not found: {0}")]
    ReferenceKeyNotFound(String),
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

fn handle_embedded_constraint(
    reference: &schema::Reference,
    subassemble: &Subassemble,
) -> Vec<Stage> {
    match reference.relationship_type {
        Relationship::Many => {
            vec![Stage::Unwind(Unwind::Document(UnwindExpr {
                path: Expression::Ref(Ref::FieldRef(
                    reference.storage_constraints[0].target_path.clone(),
                ))
                .into(),
                preserve_null_and_empty_arrays: Some(
                    subassemble.join == Some(AssembleJoinType::Left),
                ),
                include_array_index: None,
            }))]
        }
        Relationship::One => {
            vec![]
        }
    }
}

fn handle_reference_constraint(
    entity_name: &str,
    key: &str,
    reference: &schema::Reference,
    subassemble: &Subassemble,
    entities: &BTreeMap<String, Entity>,
) -> Result<Vec<Stage>> {
    let Some(filter) = subassemble.filter.as_ref() else {
        return Err(Error::MissingFilterInSubassemble(key.to_string()));
    };
    let Some(foreign_field) = filter.get(key).clone() else {
        return Err(Error::MissingKeyInFilter(
            key.to_string(),
            // we know this will pretty print because it parsed from json
            serde_json::to_string_pretty(filter).unwrap(),
        ));
    };
    let foreign_field = if let Expression::Ref(Ref::FieldRef(foreign_field)) = foreign_field {
        foreign_field.clone()
    } else {
        // TODO: We probably want to handle other expressions, so I'm leaving this as a panic for
        // now
        todo!("Expected field ref in subassemble filter");
    };
    let from_name = if reference.storage_constraints[0].direction == Direction::Child {
        entities.get(entity_name).unwrap().collection.clone()
    } else {
        entities.get(&reference.entity).unwrap().collection.clone()
    };
    Ok(vec![
        Stage::Lookup(Lookup::Equality(EqualityLookup {
            from: LookupFrom::Collection(from_name.clone()),
            foreign_field,
            local_field: reference.storage_constraints[0].target_path.clone(),
            as_var: from_name.clone(),
        })),
        Stage::Unwind(Unwind::Document(UnwindExpr {
            path: Expression::Ref(Ref::FieldRef(from_name)).into(),
            preserve_null_and_empty_arrays: Some(subassemble.join == Some(AssembleJoinType::Left)),
            include_array_index: None,
        })),
    ])
}

fn handle_subassemble(
    project_keys: &[String],
    subassemble: Subassemble,
    entities: &BTreeMap<String, Entity>,
) -> Result<Vec<Stage>> {
    let mut output = Vec::new();
    let subassemble_entity = entities
        .get(&subassemble.entity)
        .ok_or(Error::EntityMissingFromErd(subassemble.entity.to_string()))?;
    let filter_keys = subassemble
        .filter
        .clone()
        .ok_or(Error::MissingFilterInSubassemble(
            serde_json::to_string_pretty(&subassemble).unwrap(),
        ))?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for key in filter_keys {
        // TODO: Don't take for granted that the filter is correct like we
        // currently do.
        let reference = subassemble_entity
            .json_schema
            .references()
            .ok_or(Error::ReferenceNotFoundInSubassemble)?
            .get(&key)
            .ok_or(Error::ReferenceKeyNotFound(key.clone()))?;
        match reference.storage_constraints[0].constraint_type {
            ConstraintType::Embedded => {
                output.extend(handle_embedded_constraint(&reference, &subassemble));
            }
            ConstraintType::Reference => {
                output.extend(handle_reference_constraint(
                    subassemble.entity.as_str(),
                    key.as_str(),
                    &reference,
                    &subassemble,
                    entities,
                )?);
            }
            _ => todo!("Unsupported constraint type for now"),
        }
    }
    // handle project, eventually we'll need to union all the subassembles
    output.push(handle_project({
        let mut project = subassemble.project;
        project.extend(project_keys.iter().cloned());
        project
    }));
    Ok(output)
}

fn handle_project(project: Vec<String>) -> Stage {
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

impl Visitor for AssembleRewrite {
    // visit_stage is here to handle Assemble stages and replace them with SubPipelines
    fn visit_stage(&mut self, stage: Stage) -> Stage {
        if self.error.is_some() {
            return Stage::SubPipeline(Vec::new());
        }
        macro_rules! handle_error {
            ($e:expr) => {
                match $e {
                    Err(e) => {
                        self.error = Some(e);
                        return Stage::SubPipeline(Vec::new());
                    }
                    Ok(v) => v,
                }
            };
        }
        match stage {
            Stage::Assemble(a) => {
                let erd_json = handle_error!(std::fs::read_to_string(&a.erd)
                    .map_err(|_| Error::CouldNotFindErd(a.erd.clone())));
                let erd: Erd =
                    handle_error!(serde_json::from_str(&erd_json).map_err(Error::CouldNotParseErd));
                let entities = erd.entities;
                // TODO: use input entity for checking
                let _input_entity = handle_error!(entities
                    .get(&a.entity)
                    .ok_or(Error::EntityMissingFromErd(a.entity.clone())));
                let mut output = vec![Stage::Project(ProjectStage {
                    items: map! {
                        a.entity.clone() => ProjectItem::Assignment(Expression::Ref(Ref::VariableRef("ROOT".to_string()))),
                    },
                })];
                for subassemble in a.subassemble.into_iter() {
                    let ret = handle_subassemble(a.project.as_ref(), subassemble, &entities);
                    if let Err(e) = ret {
                        self.error = Some(e);
                        return Stage::SubPipeline(Vec::new());
                    }
                    output.extend(ret.unwrap().into_iter());
                }
                Stage::SubPipeline(output)
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
                    Stage::SubPipeline(sub_pipeline) => sub_pipeline,
                    stage => vec![stage],
                })
                .collect(),
        }
    }
}
