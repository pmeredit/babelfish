use crate::erd::{ConstraintType, Erd, ErdRelationship, Source};
use ast::{
    definitions::{
        visitor::Visitor, EqualityLookup, Expression, Lookup, LookupFrom, MatchExpr,
        MatchExpression, MatchStage, NattyJoin, NattyJoinExpression, Pipeline, ProjectItem,
        ProjectStage, Ref, Stage, Unwind,
    },
    map,
};
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

pub struct NattyJoinRewrite {
    error: Option<Error>,
}

pub fn rewrite_pipeline(pipeline: Pipeline) -> Result<Pipeline> {
    let mut visitor = NattyJoinRewrite { error: None };
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

impl Visitor for NattyJoinRewrite {
    // visit_stage is here to handle NattyJoin stages and replace them with SubPipelines
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
            Stage::NattyJoin(j) => {
                let erd_json = handle_error!(std::fs::read_to_string("assets/new_erd.json")
                    .map_err(|_| Error::CouldNotFindErd("assets/new_erd.json".to_string())));
                let erd: Erd =
                    handle_error!(serde_json::from_str(&erd_json).map_err(Error::CouldNotParseErd));
                let mut generator = NattyJoinGenerator { entities: erd };
                let sub_pipeline = handle_error!(generator.generate_natty_join(*j));
                Stage::SubPipeline(sub_pipeline)
            }
            _ => stage,
        }
    }

    // visit_pipeline is here to flatten out SubPipelines introduced as replacements
    // for NattyJoin stages
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

struct NattyJoinGenerator {
    entities: Erd,
}

impl NattyJoinGenerator {
    fn generate_natty_join(&mut self, natty_join: NattyJoin) -> Result<Pipeline> {
        let mut pipeline = Pipeline {
            pipeline: Vec::new(),
        };

        // assume only inner and only one level
        match natty_join {
            NattyJoin::Inner(NattyJoinExpression {
                mut args,
                mut condition,
            }) => {
                let NattyJoin::Entity(mut current) = args.remove(0) else {
                    panic!("Only supporting entities")
                };
                let source = self
                    .entities
                    .get_source(&current)
                    .expect(format!("Missing source for entity {}", current).as_str());
                pipeline.push(Self::generate_for_source(source, current.clone())?);
                for arg in args {
                    let NattyJoin::Entity(entity) = arg else {
                        panic!("Only supporting entities")
                    };

                    let relationship = self
                        .entities
                        .get_relationship(&current, &entity)
                        .expect(format!("Missing relationship {} => {}", current, entity).as_str());

                    match relationship.constraint.constraint_type {
                        ConstraintType::Embedded => {
                            pipeline.push(Self::generate_for_embedded(relationship)?);
                        }
                        ConstraintType::Foreign => {
                            pipeline.push(Self::generate_for_foreign(relationship)?);
                        }
                    }
                    current = entity;
                }
                if let Some(condition) = condition {
                    pipeline.push(Stage::Match(MatchStage {
                        expr: vec![MatchExpression::Expr(MatchExpr {
                            expr: Box::new(condition),
                        })],
                        numbering: None,
                    }));
                }
            }
            _ => panic!("Not supporting $left yet, and if this is an Entity... this isn't a join"),
        }
        Ok(pipeline)
    }

    fn generate_for_source(source: &Source, entity: String) -> Result<Stage> {
        if source.target_path.is_none() {
            return Ok(Stage::Project(ProjectStage {
                items: map! {
                    entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::VariableRef("ROOT".to_string()))),
                    "_id".to_string() => ProjectItem::Exclusion,
                },
            }));
        }
        Ok(Stage::SubPipeline(Pipeline {
            pipeline: vec![
                Stage::Unwind(Unwind::FieldPath(Expression::Ref(Ref::FieldRef(
                    source.target_path.as_ref().unwrap().to_string(),
                )))),
                Stage::Project(ProjectStage {
                    items: map! {
                        entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::FieldRef(source.target_path.as_ref().unwrap().to_string()))),
                        "_id".to_string() => ProjectItem::Exclusion,
                    },
                }),
            ],
        }))
    }

    fn generate_for_embedded(relationship: &ErdRelationship) -> Result<Stage> {
        Ok(Stage::SubPipeline(Pipeline {
            pipeline: vec![
                Stage::Unwind(Unwind::FieldPath(Expression::Ref(Ref::FieldRef(
                    relationship
                        .constraint
                        .target_path
                        .as_ref()
                        .unwrap()
                        .to_string(),
                )))),
                Stage::Project(ProjectStage {
                    items: map! {
                        relationship.foreign_entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::FieldRef(relationship.constraint.target_path.as_ref().unwrap().to_string()))),
                        "_id".to_string() => ProjectItem::Exclusion,
                    },
                }),
            ],
        }))
    }

    fn generate_for_foreign(relationship: &ErdRelationship) -> Result<Stage> {
        Ok(Stage::SubPipeline(Pipeline {
            pipeline: vec![
                Stage::Lookup(Lookup::Equality(EqualityLookup {
                    from: LookupFrom::Collection(
                        relationship
                            .constraint
                            .collection
                            .as_ref()
                            .expect("Collection not found in foreign constraint")
                            .to_string(),
                    ),
                    local_field: relationship
                        .constraint
                        .local_key
                        .as_ref()
                        .expect("Missing localKey in foreign constraint")
                        .to_string(),
                    foreign_field: relationship
                        .constraint
                        .foreign_key
                        .as_ref()
                        .expect("Missing foreign key in foreign constraint")
                        .to_string(),
                    as_var: relationship.foreign_entity.to_string(),
                })),
                Stage::Unwind(Unwind::FieldPath(Expression::Ref(Ref::FieldRef(
                    relationship.foreign_entity.to_string(),
                )))),
            ],
        }))
    }
}
