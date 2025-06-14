use ast::definitions::{
    Join, JoinExpression, Pipeline, ProjectItem, ProjectStage, Stage, visitor::Visitor,
};
use linked_hash_map::LinkedHashMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    #[error("An Entity must contain at least one element before a '.', found: {0}")]
    EntityNameMissing(String),
    #[error("No Entities found in the Conjure stage")]
    NoEntitiesFound,
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct ConjureRewrite {
    error: Option<Error>,
}

pub fn rewrite_pipeline(pipeline: Pipeline) -> Result<Pipeline> {
    let mut visitor = ConjureRewrite { error: None };
    let pipeline = visitor.visit_pipeline(pipeline);
    if let Some(e) = visitor.error {
        Err(e)
    } else {
        Ok(pipeline)
    }
}

impl Visitor for ConjureRewrite {
    // visit_stage is here to handle Project stages and replace them with SubPipelines when an
    // Entity is found in the project.
    fn visit_stage(&mut self, stage: Stage) -> Stage {
        match stage {
            Stage::Conjure(ref v) => {
                let (entites, project_stage) = v
                    .into_iter()
                    .filter_map(|item| {
                        let sp: Vec<_> = item.split('.').collect();
                        if sp.len() < 2 {
                            self.error = Some(Error::EntityNameMissing(item.to_string()));
                            return None; // Skip this entity, as it is invalid
                        }
                        match sp[1] {
                            "*" => Some((
                                (sp[0].to_string(), ()),
                                (sp[0].to_string(), ProjectItem::Inclusion),
                            )),
                            _ => Some((
                                (sp[0].to_string(), ()),
                                (item.to_string(), ProjectItem::Inclusion),
                            )),
                        }
                    })
                    .collect::<(LinkedHashMap<_, _>, LinkedHashMap<_, _>)>();
                if entites.is_empty() {
                    if self.error.is_none() {
                        self.error = Some(Error::NoEntitiesFound);
                    }
                    return Stage::Sentinel; // return anything, this is an Error case
                }
                Stage::SubPipeline(Pipeline {
                    pipeline: vec![
                        Stage::Join(Box::new(Join::Inner(JoinExpression {
                            args: entites.into_iter().map(|(e, _)| Join::Entity(e)).collect(),
                            condition: None,
                        }))),
                        Stage::Project(ProjectStage {
                            items: project_stage,
                        }),
                    ],
                })
            }
            _ => stage,
        }
    }

    // visit_pipeline is here to flatten out SubPipelines introduced as replacements
    // for Conjure stages
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
