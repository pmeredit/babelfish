use crate::erd::{ConstraintType, Erd, ErdRelationship, Source};
use ast::{
    definitions::{
        visitor::Visitor, ProjectItem, EqualityLookup, Expression,
        Lookup, LookupFrom, MatchExpr, MatchExpression, MatchStage, Pipeline,
        ProjectStage, Ref, Stage, Unwind,
    },
    map,
};
use thiserror::Error;

pub struct AugmentedProjectRewrite;

pub fn rewrite_pipeline(pipeline: Pipeline) -> Pipeline {
    let mut visitor = AugmentedProjectRewrite;
    visitor.visit_pipeline(pipeline)
}

impl Visitor for AugmentedProjectRewrite {
    // visit_stage is here to handle AugmentedProject stages and replace them with SubPipelines
    fn visit_stage(&mut self, stage: Stage) -> Stage {
        match stage {
            Stage::Project(p) => {Stage::Project(p)}
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
