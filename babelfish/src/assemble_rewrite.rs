use std::collections::BTreeMap;

use ast::definitions::{
    visitor::Visitor, AssembleJoinType, EqualityLookup, Expression, Lookup, LookupFrom, Pipeline,
    Ref, Stage, Subassemble, Unwind, UnwindExpr,
};
use schema::{ConstraintType, Entity, Erd, Relationship};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Could not parse ERD: {0}")]
    CouldNotParseErd(#[from] serde_json::Error),
    #[error("Entity: {0} missing from ERD")]
    EntityMissingFromErd(String),
    #[error("Missing filter in subassemble: {0}")]
    MissingFilterInSubassemble(String),
    #[error("Missing key in filter, key: {0}, filter: {1}")]
    MissingKeyInFilter(String, String),
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
    Ok(vec![
        Stage::Lookup(Lookup::Equality(EqualityLookup {
            from: LookupFrom::Collection(
                entities.get(&reference.entity).unwrap().collection.clone(),
            ),
            foreign_field,
            local_field: reference.storage_constraints[0].target_path.clone(),
            as_var: reference.storage_constraints[0].target_path.clone(),
        })),
        Stage::Unwind(Unwind::Document(UnwindExpr {
            path: Expression::Ref(Ref::FieldRef(
                reference.storage_constraints[0].target_path.clone(),
            ))
            .into(),
            preserve_null_and_empty_arrays: Some(subassemble.join == Some(AssembleJoinType::Left)),
            include_array_index: None,
        })),
    ])
}

fn handle_subassemble(
    subassemble: Subassemble,
    entities: &BTreeMap<String, Entity>,
) -> Result<Vec<Stage>> {
    let mut output = Vec::new();
    let subassemble_entity = entities.get(&subassemble.entity).unwrap();
    let filter_keys = subassemble
        .filter
        .clone()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for key in filter_keys {
        // TODO: Don't take for granted that the filter is correct like we
        // currently do.
        let reference = subassemble_entity
            .json_schema
            .references()
            .unwrap()
            .get(&key)
            .unwrap();
        match reference.storage_constraints[0].constraint_type {
            ConstraintType::Embedded => {
                output.extend(handle_embedded_constraint(&reference, &subassemble));
            }
            ConstraintType::Reference => {
                output.extend(handle_reference_constraint(
                    key.as_str(),
                    &reference,
                    &subassemble,
                    entities,
                )?);
            }
            _ => panic!("Unsupported constraint type for now"),
        }
    }
    Ok(output)
}

impl Visitor for AssembleRewrite {
    // visit_stage is here to handle Assemble stages and replace them with SubPipelines
    fn visit_stage(&mut self, stage: Stage) -> Stage {
        match stage {
            Stage::Assemble(a) => {
                let erd = std::fs::read_to_string(&a.erd).unwrap();
                // TODO: handle errors gracefully
                let erd: Erd = serde_json::from_str(&erd).unwrap();
                let entities = erd.entities;
                // TODO use input entity for checking
                let _input_entity = entities.get(&a.entity).unwrap();
                let mut output = Vec::new();
                for subassemble in a.subassemble.into_iter() {
                    let ret = handle_subassemble(subassemble, &entities);
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
