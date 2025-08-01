use crate::{erd::{Relationships}, erd_graph::{GetErdData, EdgeData, ErdGraph}};
use ast::{
    definitions::{
        visitor::Visitor, Derived, EqualityLookup, Expression, Join, JoinExpression, Lookup, LookupFrom, MatchExpr, MatchExpression, MatchStage, Pipeline, ProjectItem, ProjectStage, Ref, Stage, Unwind, UnwindExpr
    },
    map,
};
use petgraph::graph::NodeIndex;
use std::collections::HashSet;
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
    #[error("Top level join must have root")]
    NoRoot,
    #[error("Root in subjoin is not allowed")]
    RootInSubjoin,
    #[error("Relationship missing between {0} and {1}")]
    RelationshipMissingBetween(String, String),
    #[error("Derived entity {0} already in scope")]
    DerivedEntityAlreadyInScope(String),
    #[error("No path to entity: {0}")]
    NoPathToEntity(String),
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
                let erd_json = handle_error!(
                    std::fs::read_to_string("assets/rel.json")
                        .map_err(|_| Error::CouldNotFindErd("assets/rel.json".to_string()))
                );
                let erd: Relationships =
                    handle_error!(serde_json::from_str(&erd_json).map_err(Error::CouldNotParseErd));
                let mut generator = JoinGenerator::new(erd);
                handle_error!(generator.generate_join(*j));
                Stage::SubPipeline(generator.pipeline)
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
    entities: Relationships,
    erd_graph: ErdGraph,
    nodes_in_scope: HashSet<NodeIndex>,
    pipeline: Pipeline,
}

impl JoinGenerator {
    fn new(entities: Relationships) -> Self {
        let erd_graph = ErdGraph::new(&entities);
        println!("{}", erd_graph);
        JoinGenerator {
            entities,
            erd_graph,
            nodes_in_scope: HashSet::new(),
            pipeline: Pipeline::default(),
        }
    }

    fn generate_for_derived(
        &mut self,
        is_left: bool,
        root: NodeIndex,
        derived: &Derived,
    ) -> Result<()> {
        let entity = &derived.entity;
        let entity_index = self.erd_graph
            .get_index(entity)
            .ok_or_else(|| Error::EntityMissingFromErd(entity.to_string()))?;
        if self.nodes_in_scope.contains(&entity_index) {
            // already in scope, this will be an error, derived entities must be unique in scope.
            return Err(Error::DerivedEntityAlreadyInScope(entity.to_string()));
        }
        let Some(path) = self.erd_graph.path_to(root, entity_index) else {
            return Err(Error::NoPathToEntity(entity.to_string()));
        };
        let mut current_index = root;
        for target_index in path.into_iter() {
            if self.nodes_in_scope.contains(&target_index) {
                current_index = target_index;
                continue;
            }
            self.nodes_in_scope.insert(target_index);
            let edge_data = self.erd_graph.get_edge_data(current_index, target_index);
            match edge_data {
                Some(EdgeData::Embedded {
                    source_entity,
                    target_path,
                    relationship_type: _,
                }) => {
                    if target_index == entity_index {
                        let pipeline = derived.pipeline.pipeline.clone();
                        // If the entity is the current entity, we prefix in the pipeline
                        self.pipeline.push(Stage::SubPipeline(Pipeline {pipeline}));
                    }
                    self.pipeline.push(self.generate_for_embedded(
                            is_left,
                     source_entity,
                    self.erd_graph.get_entity_name(target_index).unwrap(),
                        &target_path,
                    )?);
                }
                Some(EdgeData::Foreign {
                    db: _,
                    collection,
                    foreign_key,
                    local_key,
                    relationship_type: _,
                }) => {
                    if target_index == entity_index {
                        let pipeline = derived.pipeline.pipeline.clone();
                        // If the entity is the current entity, we prefix in the pipeline
                        self.pipeline.push(Stage::SubPipeline(Pipeline {pipeline}));
                    }
                    self.pipeline.push(self.generate_for_foreign(
                            is_left,
                    self.erd_graph.get_entity_name(current_index).unwrap(),
                    self.erd_graph.get_entity_name(target_index).unwrap(),
                        &collection,
                        &local_key,
                        &foreign_key,
                    )?);
                }
                // This should actually be impossible since we shouldn't be able to
                // find a path to this entity.
                None => 
                    Err(Error::RelationshipMissingBetween(
                        self.erd_graph.get_entity_name(current_index).unwrap().to_string(),
                        self.erd_graph.get_entity_name(target_index).unwrap().to_string(),
                    ))?,
            }
            current_index = target_index;
        }
        Ok(())
    }

    fn generate_for_entity(
        &mut self,
        is_left: bool,
        root: NodeIndex,
        entity: &str,
    ) -> Result<()> {
          let entity_index = self.erd_graph
              .get_index(entity)
              .ok_or_else(|| Error::EntityMissingFromErd(entity.to_string()))?;
          if self.nodes_in_scope.contains(&entity_index) {
              // already in scope, skip.
              // Ideally a join should have unique entities, but this is a safeguard.
              return Ok(());
          }
          let Some(path) = self.erd_graph.path_to(root, entity_index) else {
              return Err(Error::NoPathToEntity(entity.to_string()));
          };
          let mut current_index = root;
          for target_index in path.into_iter() {
              if self.nodes_in_scope.contains(&target_index) {
                  current_index = target_index;
                  continue;
              }
              self.nodes_in_scope.insert(target_index);
              let edge_data = self.erd_graph.get_edge_data(current_index, target_index);
              match edge_data {
                  Some(EdgeData::Embedded {
                      source_entity,
                      target_path,
                      relationship_type: _,
                  }) => {
                      self.pipeline.push(self.generate_for_embedded(
                              is_left,
                       source_entity,
                      self.erd_graph.get_entity_name(target_index).unwrap(),
                          &target_path,
                      )?);
                  }
                  Some(EdgeData::Foreign {
                      db: _,
                      collection,
                      foreign_key,
                      local_key,
                      relationship_type: _,
                  }) => {
                      self.pipeline.push(self.generate_for_foreign(
                              is_left,
                      self.erd_graph.get_entity_name(current_index).unwrap(),
                      self.erd_graph.get_entity_name(target_index).unwrap(),
                          &collection,
                          &local_key,
                          &foreign_key,
                      )?);
                  }
                  // This should actually be impossible since we shouldn't be able to
                  // find a path to this entity.
                  None => 
                      Err(Error::RelationshipMissingBetween(
                          self.erd_graph.get_entity_name(current_index).unwrap().to_string(),
                          self.erd_graph.get_entity_name(target_index).unwrap().to_string(),
                      ))?,
              }
              current_index = target_index;
          }
          Ok(())
    }

    fn generate_join_aux(
        &mut self,
        is_left: bool,
        root_entity: &str,
        args: &[Join],
        condition: Option<Expression>,
    ) -> Result<()> {
        let root = self.erd_graph
            .get_index(root_entity)
            .ok_or_else(|| Error::EntityMissingFromErd(root_entity.to_string())).unwrap();
            for arg in args {
                match arg {
                    Join::Entity(entity) => {
                        self.generate_for_entity(
                            is_left,
                            root,
                            entity.as_str(),
                        )?
                    }
                    Join::Derived(derived) => {
                        self.generate_for_derived(
                            is_left,
                            root,
                            derived,
                        )?;
                    }
                    Join::Inner(JoinExpression {
                        root,
                        args,
                        condition,
                    }) => {
                        if root.is_some() {
                            return Err(Error::RootInSubjoin);
                        }
                        self.generate_join_aux(
                            false,
                            root_entity,
                            args,
                            condition.clone(),
                        )?;
                    }
                    Join::Left(JoinExpression {
                        root,
                        args,
                        condition,
                    }) => {
                        if root.is_some() {
                            return Err(Error::RootInSubjoin);
                        }
                        self.generate_join_aux(
                            true,
                            root_entity,
                            args,
                            condition.clone(),
                        )?;
                    }
                }
            }
            if let Some(condition) = condition {
                self.pipeline.push(Stage::Match(MatchStage {
                    expr: vec![MatchExpression::Expr(MatchExpr {
                        expr: Box::new(condition),
                    })],
                    numbering: None,
                }));
            }
        Ok(())
    }

    fn generate_join(&mut self, join: Join) -> Result<()> {
        // assume only inner and only one level
        let (is_left, root_entity, args, condition) = match join {
            Join::Inner(JoinExpression { root, args, condition }) => (false, root, args, condition),
            Join::Left(JoinExpression { root, args, condition }) => (true, root, args, condition),
            _ => unreachable!("Only handling Inner and Left joins right now!")
        };
        let root_entity = root_entity.ok_or(Error::NoRoot)?;
        let root = self.erd_graph
            .get_index(&root_entity)
            .ok_or_else(|| Error::EntityMissingFromErd(root_entity.clone())).unwrap();
        self.pipeline
            .push(self.generate_for_root_source(root_entity.as_str())?);
        self.nodes_in_scope.insert(root);
        self.generate_join_aux(
            is_left,
            &root_entity,
            &args,
            condition,
        )?;
        Ok(())
    }

    fn generate_for_root_source(&self, entity: &str) -> Result<Stage> {
        // removing source from erds? Always assume root is ROOT of whatever we are running on?
        //let source = self
        //    .entities
        //    .get_source(entity)
        //    .ok_or_else(|| Error::EntityMissingFromErd(entity.to_string())).unwrap();
        //if source.target_path.is_none() {
        //    return 
            Ok(Stage::Project(ProjectStage {
                items: map! {
                    entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::VariableRef("ROOT".to_string()))),
                    "_id".to_string() => ProjectItem::Exclusion,
                },
            }))
        //}
//        Ok(Stage::SubPipeline(Pipeline {
//            pipeline: vec![
//                Stage::Unwind(Unwind::FieldPath(Expression::Ref(Ref::FieldRef(
//                    source.target_path.as_ref().unwrap().to_string(),
//                )))),
//                Stage::Project(ProjectStage {
//                    items: map! {
//                        entity.to_string() => ProjectItem::Assignment(Expression::Ref(Ref::FieldRef(source.target_path.as_ref().unwrap().to_string()))),
//                        "_id".to_string() => ProjectItem::Exclusion,
//                    },
//                }),
//            ],
//        }))
    }

    fn generate_for_embedded(
        &self,
        is_left: bool,
        parent_entity: &str,
        embedded_entity: &str,
        target_path: &str,
    ) -> Result<Stage> {
        let field = format!("{}.{}", parent_entity, target_path);
        Ok(Stage::SubPipeline(Pipeline {
            pipeline: vec![
                Stage::Unwind(Unwind::Document(
                        UnwindExpr {
                            path: Box::new(Expression::Ref(Ref::FieldRef(field.clone()))),
                            include_array_index: None,
                            preserve_null_and_empty_arrays: Some(is_left),
                        }
                )),
                Stage::AddFields(map! {
                    embedded_entity.to_string() => Expression::Ref(Ref::FieldRef(field)),
                }),
            ],
        }))
    }

    fn generate_for_foreign(
        &self,
        is_left: bool,
        local_entity: &str,
        foreign_entity: &str,
        coll: &str,
        local_key: &str,
        foreign_key: &str,
    ) -> Result<Stage> {
        Ok(Stage::SubPipeline(Pipeline {
            pipeline: vec![
                Stage::Lookup(Lookup::Equality(EqualityLookup {
                    from: LookupFrom::Collection(coll.to_string()),
                    local_field: format!("{}.{}", local_entity, local_key),
                    foreign_field: foreign_key.to_string(),
                    as_var: foreign_entity.to_string(),
                })),
                Stage::Unwind(Unwind::Document(
                        UnwindExpr {
                            path: Box::new(Expression::Ref(Ref::FieldRef(foreign_entity.to_string()))),
                            include_array_index: None,
                            preserve_null_and_empty_arrays: Some(is_left),
                        }
                )),
            ],
        }))
    }
}
