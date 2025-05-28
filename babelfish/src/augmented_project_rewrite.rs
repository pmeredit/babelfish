use ast::{
    definitions::{
        visitor::Visitor, Expression, JoinExpression, Pipeline, ProjectItem, Ref, Stage, Join,
    },
};
use linked_hash_map::LinkedHashMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    #[error("An Entity must contain at least one element before a '.', found: {0}")]
    EntityNameMissing(String),
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct AugmentedProjectRewrite {
    error: Option<Error>,
}

pub fn rewrite_pipeline(pipeline: Pipeline) -> Result<Pipeline> {
    let mut visitor = AugmentedProjectRewrite { error: None };
    let pipeline = visitor.visit_pipeline(pipeline);
    if let Some(e) = visitor.error {
        Err(e)
    } else {
        Ok(pipeline)
    }
}

impl Visitor for AugmentedProjectRewrite {
    // visit_stage is here to handle Project stages and replace them with SubPipelines when an
    // Entity is found in the project.
    fn visit_stage(&mut self, mut stage: Stage) -> Stage {
        match stage {
            Stage::Project(ref mut p) => {
                let entites = p.items.iter_mut().filter_map(|(name, item)| {
                    if let ProjectItem::Assignment(Expression::Ref(Ref::VariableRef(v))) = item {
                        if v == "E" {
                            let split = name.split('.').collect::<Vec<_>>();
                            if split.len() < 2 {
                                self.error = Some(Error::EntityNameMissing(v.to_string()));
                                return None;
                            }
                            *item = ProjectItem::Inclusion;
                            Some((split[0].to_string(), ()))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }).collect::<LinkedHashMap<_, _>>();
                if entites.is_empty() {
                    return stage; // No entities found, return the original stage
                }
                Stage::SubPipeline( Pipeline {
                    pipeline: vec![
                        Stage::Join (
                            Box::new(
                                Join::Inner(
                                    JoinExpression {
                                        args: entites.into_iter().map(|(e, _)| Join::Entity(e)).collect(),
                                        condition: None,
                                    }
                                )
                            )
                        ),
                        stage
                    ],
                })
            }
            _ => stage,
        }
    }

    // visit_pipeline is here to flatten out SubPipelines introduced as replacements
    // for AugmentedProject stages
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
