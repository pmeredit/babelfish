use ast::{
    definitions::{
        visitor::Visitor, AssembleJoinType, EqualityLookup, Expression, Lookup, LookupFrom,
        MatchExpr, MatchExpression, MatchStage, Pipeline, ProjectItem, ProjectStage, Ref, Stage,
        Subassemble, SubqueryLookup, Unwind, UnwindExpr,
    },
    map,
    uses::FilterAndUsePartition,
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
    #[error("Embedded constraints must have targetPath: {0}")]
    MissingTargetPathInEmbedded(String),
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

fn handle_embedded_constraint(
    entity_name: &str,
    reference: &schema::Reference,
    subassemble: &Subassemble,
) -> Result<Vec<Stage>> {
    match reference.relationship_type {
        Relationship::Many => {
            let Some(target_path) = reference.storage_constraints[0].target_path.clone() else {
                return Err(Error::MissingTargetPathInEmbedded(print_json!(
                    &reference.storage_constraints[0]
                )));
            };
            Ok(vec![
                Stage::Unwind(Unwind::Document(UnwindExpr {
                    path: Expression::Ref(Ref::FieldRef(target_path.clone())).into(),
                    preserve_null_and_empty_arrays: Some(
                        subassemble.join == Some(AssembleJoinType::Left),
                    ),
                    include_array_index: None,
                })),
                Stage::AddFields(map! {
                    entity_name.to_string() => Expression::Ref(Ref::FieldRef(target_path.clone())),
                }),
                Stage::Project(ProjectStage {
                    items: map! {
                        target_path => ProjectItem::Exclusion,
                    },
                }),
            ])
        }
        Relationship::One => Ok(vec![]),
    }
}

struct ReplaceVisitor {
    entity_prefix: String,
}

impl Visitor for ReplaceVisitor {
    fn visit_expression(&mut self, expression: Expression) -> Expression {
        match expression {
            Expression::Ref(Ref::FieldRef(s)) => {
                if s.starts_with(&self.entity_prefix) {
                    Expression::Ref(Ref::VariableRef(s))
                } else {
                    Expression::Ref(Ref::FieldRef(s))
                }
            }
            _ => expression.walk(self),
        }
    }
}

fn replace_with_variable(expression: Expression, entity_name: &str) -> Expression {
    let mut visitor = ReplaceVisitor {
        entity_prefix: format!("{}.", entity_name),
    };
    visitor.visit_expression(expression)
}

fn handle_reference_constraint(
    entity_name: &str,
    subassemble_entity: &str,
    subassemble_join: Option<AssembleJoinType>,
    partition: FilterAndUsePartition,
) -> Result<Vec<Stage>> {
    // TODO: Handle checking and entity names that do not match collection names
    let pipeline = if let Some(matcher) = partition.right {
        let matcher = replace_with_variable(matcher, entity_name);
        vec![Stage::Match(MatchStage {
            expr: vec![MatchExpression::Expr(MatchExpr {
                expr: Box::new(matcher),
            })],
        })]
    } else {
        vec![]
    };
    let mut output = if let Some(matcher) = partition.left {
        vec![Stage::Match(MatchStage {
            expr: vec![MatchExpression::Expr(MatchExpr {
                expr: Box::new(matcher),
            })],
        })]
    } else {
        vec![]
    };
    output.push(Stage::Lookup(Lookup::Subquery(SubqueryLookup {
        from: Some(LookupFrom::Collection(subassemble_entity.to_string())),
        let_body: Some(map! {
            entity_name.to_string() => Expression::Ref(Ref::FieldRef(entity_name.to_string())),
        }),
        pipeline: Pipeline { pipeline },
        as_var: subassemble_entity.to_string(),
    })));
    output.push(Stage::Unwind(Unwind::Document(UnwindExpr {
        path: Box::new(Expression::Ref(Ref::FieldRef(
            subassemble_entity.to_string(),
        ))),
        include_array_index: None,
        preserve_null_and_empty_arrays: Some(subassemble_join == Some(AssembleJoinType::Left)),
    })));
    Ok(output)
}

fn handle_subassemble(
    entity_name: &str,
    subassemble: Subassemble,
    entities: &BTreeMap<String, Entity>,
) -> Result<(Vec<Stage>, Vec<String>)> {
    let mut output = Vec::new();
    let subassemble_entity = entities
        .get(&subassemble.entity)
        .ok_or(Error::EntityMissingFromErd(subassemble.entity.to_string()))?;
    let filter_partition = subassemble
        .filter
        .unwrap()
        .filter_partition(entity_name, subassemble.entity.as_str());
    // TODO:: Handle embedded constraints again
    output.extend(handle_reference_constraint(
        entity_name,
        &subassemble.entity,
        subassemble.join,
        filter_partition,
    )?);
    Ok((
        output,
        subassemble
            .project
            .into_iter()
            .map(|x| format!("{}.{}", subassemble.entity, x))
            .collect(),
    ))
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
                        "_id".to_string() => ProjectItem::Exclusion,
                    },
                })];
                let mut project_keys = a
                    .project
                    .into_iter()
                    .map(|x| format!("{}.{}", a.entity, x))
                    .collect::<Vec<_>>();
                for subassemble in a.subassemble.into_iter() {
                    let ret = handle_subassemble(&a.entity, subassemble, &entities);
                    if let Err(e) = ret {
                        self.error = Some(e);
                        return Stage::SubPipeline(Pipeline {
                            pipeline: Vec::new(),
                        });
                    }
                    let (stages, keys) = ret.unwrap();
                    project_keys.extend(keys.into_iter());
                    output.extend(stages.into_iter());
                }
                output.push(handle_project(project_keys));
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
