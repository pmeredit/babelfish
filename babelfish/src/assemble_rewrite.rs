use std::collections::BTreeMap;

use ast::definitions::{
    visitor::Visitor, AssembleJoinType, EqualityLookup, Expression, Lookup, LookupFrom, Pipeline,
    Ref, Stage, Subassemble, Unwind, UnwindExpr,
};
use schema::{ConstraintType, Entity, Erd, Relationship};

pub struct AssembleRewrite;

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
) -> Vec<Stage> {
    let f = subassemble.filter.as_ref().unwrap();
    let foreign_field = f.get(key).clone().unwrap();
    let foreign_field = if let Expression::Ref(Ref::FieldRef(foreign_field)) = foreign_field {
        foreign_field.clone()
    } else {
        panic!("Expected field ref in subassemble filter");
    };
    vec![
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
    ]
}

fn handle_subassemble(subassemble: Subassemble, entities: &BTreeMap<String, Entity>) -> Vec<Stage> {
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
                ));
            }
            _ => panic!("Unsupported constraint type for now"),
        }
    }
    output
}

impl Visitor for AssembleRewrite {
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
                    output.extend(handle_subassemble(subassemble, &entities).into_iter());
                }
                Stage::SubPipeline(output)
            }
            _ => stage,
        }
    }

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
