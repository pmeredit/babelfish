use ast::definitions::{
    visitor::Visitor, Expression, MatchExpr, MatchExpression, MatchStage, Pipeline, Stage,
    UntaggedOperator, UntaggedOperatorName,
};
use std::collections::HashSet;

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

struct MatchMover;

impl Visitor for MatchMover {
    fn visit_pipeline(&mut self, mut pipeline: Pipeline) -> Pipeline {
        let len = pipeline.pipeline.len();
        let mut i = len - 1;
        // the most swaps we can do is the number of matches * len of pipeline.
        let mut total_swaps = pipeline
            .pipeline
            .iter()
            .filter(|x| matches!(x, Stage::Match(_)))
            .collect::<Vec<_>>()
            .len()
            * len;
        // we never move the first stage
        while i > 1 && total_swaps > 0 {
            total_swaps -= 1;
            let stage = std::mem::take(pipeline.pipeline.get_mut(i).unwrap()).walk(self);
            if let Stage::Match(MatchStage { expr }) = stage {
                if !move_match(expr, &mut pipeline, i) {
                    i -= 1;
                }
                // do not decrement i here, because i could not be a stage that was moved down when
                // we moved the match up.
            } else {
                pipeline.pipeline[i] = stage;
                i -= 1;
            }
        }
        pipeline
    }
}

// TODO: in the future we may want to support more users instead of just Match, like in mongosql
fn move_match(mut expr: Vec<MatchExpression>, pipeline: &mut Pipeline, i: usize) -> bool {
    macro_rules! terminal_case {
        ($expr:expr, $idx:expr, $moved:expr) => {{
            pipeline.pipeline[$idx] = Stage::Match(MatchStage { expr: $expr });
            return $moved;
        }};
    }
    if expr.is_empty() {
        terminal_case!(expr, i, false);
    }
    // Matches should be split already
    // Currently, we only handle $expr, because that's all we need for $assemble
    let MatchExpression::Expr(MatchExpr { expr }) = expr.remove(0) else {
        terminal_case!(expr, i, false);
    };
    let mut moved = false;
    let mut expr = *expr;
    for j in (1..=i).rev() {
        let uses = expr.uses();
        let swap_stage = pipeline.pipeline.get(j - 1).unwrap();
        let opaque_defines = swap_stage.opaque_defines();
        if opaque_defines.is_some()
            && !uses
                .intersection(opaque_defines.as_ref().unwrap())
                .collect::<Vec<_>>()
                .is_empty()
        {
            terminal_case!(
                vec![MatchExpression::Expr(MatchExpr {
                    expr: Box::new(expr),
                }),],
                j,
                moved
            );
        }
        let defines = swap_stage.defines();
        if let Some(defines) = defines {
            expr = expr.substitute(defines);
            let swap_stage = std::mem::take(pipeline.pipeline.get_mut(j - 1).unwrap());
            pipeline.pipeline[j] = swap_stage;
            moved = true;
        } else {
            terminal_case!(
                vec![MatchExpression::Expr(MatchExpr {
                    expr: Box::new(expr),
                }),],
                j,
                moved
            );
        }
    }
    terminal_case!(
        vec![MatchExpression::Expr(MatchExpr {
            expr: Box::new(expr),
        }),],
        0,
        moved
    );
}

pub fn rewrite_match_move(pipeline: Pipeline) -> Pipeline {
    let mut visitor = MatchSplitter;
    let pipeline = visitor.visit_pipeline(pipeline);
    let mut visitor = SubpipelineFlatten;
    let pipeline = visitor.visit_pipeline(pipeline);
    let mut visitor = MatchMover;
    visitor.visit_pipeline(pipeline)
}
