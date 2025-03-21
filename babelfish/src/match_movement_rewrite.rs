use ast::definitions::{
    visitor::Visitor, Expression, MatchExpr, MatchExpression, MatchStage, Pipeline, Stage,
    UntaggedOperator, UntaggedOperatorName,
};

pub struct SubpipelineFlatten;

impl Visitor for SubpipelineFlatten {
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

pub struct MatchSplitter;

impl Visitor for MatchSplitter {
    fn visit_stage(&mut self, stage: Stage) -> Stage {
        match stage {
            Stage::Match(MatchStage { expr }) => {
                let mut stages = vec![];
                for e in expr {
                    match e {
                        MatchExpression::Expr(MatchExpr { expr }) => {
                            let expr = expr.get_conjunctive_normal_form();
                            if let Expression::UntaggedOperator(UntaggedOperator {
                                op: UntaggedOperatorName::And,
                                args,
                            }) = expr
                            {
                                for arg in args {
                                    stages.push(Stage::Match(MatchStage {
                                        expr: vec![MatchExpression::Expr(MatchExpr {
                                            expr: Box::new(arg),
                                        })],
                                    }));
                                }
                            } else {
                                stages.push(Stage::Match(MatchStage {
                                    expr: vec![MatchExpression::Expr(MatchExpr {
                                        expr: Box::new(expr),
                                    })],
                                }));
                            }
                        }
                        // TODO: this isn't needed for mql assemble, but will be useful later
                        _ => todo!(),
                    }
                }
                Stage::SubPipeline(Pipeline { pipeline: stages })
            }
            _ => stage.walk(self),
        }
    }
}

pub fn rewrite_match_split(pipeline: Pipeline) -> Pipeline {
    let mut visitor = MatchSplitter;
    let pipeline = visitor.visit_pipeline(pipeline);
    let mut visitor = SubpipelineFlatten;
    visitor.visit_pipeline(pipeline)
}
