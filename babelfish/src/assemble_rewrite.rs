use ast::{
    definitions::{
        visitor::Visitor, Assemble, AssembleJoinType, EqualityLookup, Expression, Lookup,
        LookupFrom, MatchExpr, MatchExpression, MatchStage, Pipeline, ProjectItem, ProjectStage,
        Ref, Stage, Subassemble, SubqueryLookup, Unwind, UnwindExpr,
    },
    map,
};
use linked_hash_map::LinkedHashMap;
use schema::{ConstraintType, Direction, Entity, Erd, Relationship};
use std::collections::BTreeMap;
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
    #[error("Reference not found in Subassemble")]
    ReferenceNotFoundInSubassemble,
    #[error("Reference key not found: {0}")]
    ReferenceKeyNotFound(String),
    #[error("Embedded constraints must have targetPath: {0}")]
    MissingTargetPathInEmbedded(String),
    #[error("Project key {0} not found in entity: {1}")]
    ProjectKeyNotFound(String, String),
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
                let project_keys = handle_error!(check_and_collect_project_keys(a, &entities));
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

fn check_and_collect_project_keys(
    assemble: Assemble,
    entities: &BTreeMap<String, Entity>,
) -> Result<Vec<String>> {
    let mut ret = Vec::new();
    check_and_collect_project_keys_aux(
        assemble.entity.as_str(),
        assemble.project,
        Some(assemble.subassemble),
        entities,
        &mut ret,
    )?;
    Ok(ret)
}

fn check_and_collect_project_keys_aux(
    entity_name: &str,
    project: Vec<String>,
    subassembles: Option<Vec<Subassemble>>,
    entities: &BTreeMap<String, Entity>,
    ret: &mut Vec<String>,
) -> Result<()> {
    let entity = entities
        .get(entity_name)
        .ok_or(Error::EntityMissingFromErd(entity_name.to_string()))?;
    for field in project {
        if !entity.json_schema.can_contain_field(field.as_str()) {
            return Err(Error::ProjectKeyNotFound(field, print_json!(entity)));
        }
        ret.push(format!("{}.{}", entity_name, field));
    }
    for subassemble in subassembles.into_iter().flatten() {
        check_and_collect_project_keys_aux(
            subassemble.entity.as_str(),
            subassemble.project,
            subassemble.subassemble,
            entities,
            ret,
        )?;
    }
    Ok(())
}
