use crate::{erd::{ConstraintType, Erd, ErdRelationship, RelationshipType}, erd_graph::{self, EdgeData}};
use ast::{
    definitions::{
        visitor::Visitor, EqualityLookup, Expression, Join, JoinExpression, Lookup, LookupFrom,
        MatchExpr, MatchExpression, MatchStage, Pipeline, ProjectItem, ProjectStage, Ref, Stage,
        Unwind,
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
    #[error("No entities provided for join")]
    NoEntities,
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct JoinRewrite {
    error: Option<Error>,
}

pub fn rewrite_pipeline(pipeline: Pipeline) -> Result<Pipeline> {
    let mut visitor = JoinRewrite { error: None };
    let pipeline = visitor.visit_pipeline(pipeline);
    if let Some(e) = visitor.error {
        Err(e)
    } else {
        Ok(pipeline)
    }
}

impl Visitor for JoinRewrite {
    // visit_stage is here to handle Join stages and replace them with SubPipelines
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
            Stage::Join(j) => {
                let erd_json = handle_error!(std::fs::read_to_string("assets/new_erd.json")
                    .map_err(|_| Error::CouldNotFindErd("assets/new_erd.json".to_string())));
                let erd: Erd =
                    handle_error!(serde_json::from_str(&erd_json).map_err(Error::CouldNotParseErd));
                let mut generator = JoinGenerator { entities: erd };
                let sub_pipeline = handle_error!(generator.generate_join(*j));
                Stage::SubPipeline(sub_pipeline)
            }
            _ => stage,
        }
    }

    // visit_pipeline is here to flatten out SubPipelines introduced as replacements
    // for Join stages
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

struct JoinGenerator {
    entities: Erd,
}

impl JoinGenerator {
    fn generate_join(&mut self, join: Join) -> Result<Pipeline> {
        let mut pipeline = Pipeline {
            pipeline: Vec::new(),
        };

        // assume only inner and only one level
        match join {
            Join::Inner(JoinExpression {
                args,
                condition,
            }) => {
                let erd_graph = erd_graph::ErdGraph::new(&self.entities);
                let entities = args.iter().map(|e| if let Join::Entity(e) = e { e.to_string() } else { panic!("only supporting Entities right now") }).collect::<Vec<_>>();
                let tree = erd_graph.get_steiner_tree(&entities);
                println!("{}", erd_graph);
                println!("Steiner Tree: {:?}", tree);
                let mut nodes = tree.node_indices();
                let mut current = nodes.next().ok_or(Error::NoEntities)?;
                pipeline.push(self.generate_for_source(tree.node_weight(current).unwrap())?);
                while let Some(next) = nodes.next() {
                    let source_entity = tree.node_weight(current).unwrap();
                    let target_entity = tree.node_weight(next).unwrap();
                    let edge_data = erd_graph.get_edge_data(current, next)
                        .ok_or_else(|| Error::EntityMissingFromErd(format!("{} -> {}", source_entity, target_entity)))?;
                    match edge_data {
                        EdgeData::EmbeddedSource {..} => {
                            pipeline.push(self.generate_for_source(&target_entity)?);
                        }
                        EdgeData::Embedded { source_entity, target_path, relationship_type: _ } => {
                            pipeline.push(self.generate_for_embedded(source_entity, target_entity, target_path)?);
                        }
                        EdgeData::Foreign { db: _, collection, foreign_key, local_key, relationship_type: _} => {
                            pipeline.push(self.generate_for_foreign(source_entity, target_entity, collection, &local_key, &foreign_key )?)
                        }
                    }
                    current = next;
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

    fn generate_for_source(&self, entity: &str) -> Result<Stage> {
        let source = self.entities
            .get_source(entity)
            .ok_or_else(|| Error::EntityMissingFromErd(entity.to_string()))?;
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

    fn generate_for_embedded(&self, parent_entity: &str,  embedded_entity: &str, target_path: &str) -> Result<Stage> {
        let field = format!("{}.{}", parent_entity, target_path);
        Ok(Stage::SubPipeline(Pipeline {
            pipeline: vec![
                Stage::Unwind(Unwind::FieldPath(Expression::Ref(Ref::FieldRef(
                    field.clone(),
                )))),
                Stage::AddFields(map! {
                        embedded_entity.to_string() => Expression::Ref(Ref::FieldRef(field)),
                    },
                ),
            ],
        }))
    }

    fn generate_for_foreign(&self, local_entity: &str, foreign_entity: &str, coll: &str, local_key: &str, foreign_key: &str) -> Result<Stage> {
        Ok(Stage::SubPipeline(Pipeline {
            pipeline: vec![
                Stage::Lookup(Lookup::Equality(EqualityLookup {
                    from: LookupFrom::Collection(coll.to_string()),
                    local_field: format!("{}.{}", local_entity, local_key),
                    foreign_field: format!("{}.{}", foreign_entity, foreign_key),
                    as_var: foreign_entity.to_string(),
                })),
                Stage::Unwind(Unwind::FieldPath(Expression::Ref(Ref::FieldRef(
                    foreign_entity.to_string(),
                )))),
            ],
        }))
    }
}
