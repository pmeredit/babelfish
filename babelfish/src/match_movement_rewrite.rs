use ast::{
    definitions::{
        Expression, LiteralValue, Lookup, MatchExpr, MatchExpression, MatchStage, Pipeline, Stage,
        UntaggedOperator, UntaggedOperatorName, visitor::Visitor,
    },
    set,
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
            Stage::Match(MatchStage { expr, .. }) => {
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
                                        numbering: None,
                                    }));
                                }
                            } else {
                                stages.push(Stage::Match(MatchStage {
                                    expr: vec![MatchExpression::Expr(MatchExpr {
                                        expr: Box::new(expr),
                                    })],
                                    numbering: None,
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

// TODO: Support moving matches out of subpipelines, probably easiest to do as a separate pass with
// a changed output that we can then iterator to fix point with MatchMover
impl Visitor for MatchMover {
    fn visit_pipeline(&mut self, mut pipeline: Pipeline) -> Pipeline {
        // first number the match stages so that we do not continually swap multiple moves with
        // each other.
        for (i, stage) in pipeline.pipeline.iter_mut().enumerate() {
            if let Stage::Match(MatchStage { expr: _, numbering }) = stage {
                *numbering = Some(i);
            }
        }
        let len = pipeline.pipeline.len();
        let mut i = len - 1;
        let mut visited = HashSet::new();
        // we never move the first stage
        while i > 0 {
            let stage = std::mem::take(pipeline.pipeline.get_mut(i).unwrap()).walk(self);
            if let Stage::Match(MatchStage { expr, numbering }) = stage {
                if visited.contains(&numbering.unwrap()) {
                    pipeline.pipeline[i] = Stage::Match(MatchStage { expr, numbering });
                    i -= 1;
                    continue;
                }
                visited.insert(numbering.unwrap());
                if !move_match(expr, &mut pipeline, i, numbering) {
                    i -= 1;
                }
            } else {
                pipeline.pipeline[i] = stage;
                i -= 1;
            }
        }
        pipeline
    }
}

// TODO: in the future we may want to support more users instead of just Match, like in mongosql
fn move_match(
    mut expr: Vec<MatchExpression>,
    pipeline: &mut Pipeline,
    i: usize,
    numbering: Option<usize>,
) -> bool {
    macro_rules! terminal_case {
        ($expr:expr, $idx:expr, $moved:expr) => {{
            pipeline.pipeline[$idx] = Stage::Match(MatchStage {
                expr: $expr,
                numbering,
            });
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
        if opaque_defines.is_some() && uses.prefix_overlap(opaque_defines.as_ref().unwrap()) {
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

struct SubpipelineMatchMover {
    changed: bool,
}

impl Visitor for SubpipelineMatchMover {
    fn visit_pipeline(&mut self, mut pipeline: Pipeline) -> Pipeline {
        let mut i = 0;
        loop {
            // This needs to be inside the loop becuase the pipeline length can change
            if i >= pipeline.pipeline.len() {
                break;
            }
            // first walk the stage for recurisve subpipelines
            pipeline.pipeline[i] = std::mem::take(pipeline.pipeline.get_mut(i).unwrap()).walk(self);
            // now get a mutable reference to that stage. The borrow checker makes this a bit cumbersome
            let mut stage = std::mem::take(pipeline.pipeline.get_mut(i).unwrap());
            match stage {
                // only supporting SubqueryLookup for now
                Stage::Lookup(Lookup::Subquery(ref mut subquery)) => {
                    let mut j = 0;
                    // move all match stages at the beginning of the subpipeline into the parent
                    // iff there are no field uses in the subpipeline, substitute any variable
                    // uses. We need to use j here because it is possible that MatchMover ordered
                    // the Matches in a way where one match may depend on fields and another does
                    // not but the field depending Match is before the other Match.
                    loop {
                        // This needs to be inside the loop becuase the pipeline length can change
                        if j >= subquery.pipeline.pipeline.len() {
                            break;
                        }
                        let sub_stage = subquery.pipeline.pipeline.get_mut(j).unwrap();
                        match sub_stage {
                            Stage::Match(MatchStage { expr, numbering: _ }) => {
                                let MatchExpression::Expr(MatchExpr { expr }) =
                                    expr.into_iter().next().unwrap()
                                else {
                                    // TODO: perhaps handle other types of match stages, it
                                    // basically won't work, however.
                                    j += 1;
                                    continue;
                                };
                                // If there are field uses, we cannot move the match stage because
                                // they come from the subpipeline source
                                if !expr.uses().is_empty() {
                                    j += 1;
                                    continue;
                                }
                                // If there is a var use of $$ROOT, we cannot move the match stage
                                // because it comes from the subpipeline source
                                if expr
                                    .variable_uses()
                                    .prefix_overlap(&set! {"ROOT".to_string()})
                                {
                                    j += 1;
                                    continue;
                                }
                                self.changed = true;
                                let mut expr = *(std::mem::replace(
                                    expr,
                                    Box::new(Expression::Literal(LiteralValue::Null)),
                                ));
                                // If the subquery has a let body, substitute the variables in the
                                // expression.
                                if let Some(ref vars) = subquery.let_body {
                                    expr = expr.variable_substitute(
                                        vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                                    );
                                }
                                subquery.pipeline.pipeline.remove(j);
                                // we do not increment j because we removed the element
                                pipeline.pipeline.insert(
                                    i,
                                    Stage::Match(MatchStage {
                                        expr: vec![MatchExpression::Expr(MatchExpr {
                                            expr: Box::new(expr),
                                        })],
                                        numbering: None,
                                    }),
                                );
                                // i must increase because we have inserted in the parent pipeline
                                i += 1;
                            }
                            // If we see a non-match stage we break because any matches following a
                            // non-match must be blocked by the non-match
                            _ => {
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
            pipeline.pipeline[i] = stage;
            i += 1;
        }
        pipeline
    }
}

struct MatchCoalescer;

impl Visitor for MatchCoalescer {
    fn visit_pipeline(&mut self, pipeline: Pipeline) -> Pipeline {
        let mut out = vec![];
        let mut current_match = Vec::new();
        for stage in pipeline.pipeline.into_iter() {
            let stage = stage.walk(self);
            match stage {
                Stage::Match(MatchStage { expr, .. }) => {
                    let MatchExpression::Expr(MatchExpr { expr }) =
                        expr.into_iter().next().unwrap()
                    else {
                        todo!("handle other types of match stages");
                    };
                    current_match.push(*expr);
                }
                stage => {
                    if !current_match.is_empty() {
                        // We'll merge these into one $expr just for clenliness of reading, this is
                        // not needed. Actually this whole pass is not needed, the query planner
                        // should coallesce these into one match stage.
                        let expr = vec![MatchExpression::Expr(MatchExpr {
                            expr: Box::new(Expression::UntaggedOperator(UntaggedOperator {
                                op: UntaggedOperatorName::And,
                                args: current_match,
                            })),
                        })];
                        out.push(Stage::Match(MatchStage {
                            expr,
                            numbering: None,
                        }));
                        current_match = Vec::new();
                    }
                    out.push(stage);
                }
            }
        }
        if !current_match.is_empty() {
            let expr = vec![MatchExpression::Expr(MatchExpr {
                expr: Box::new(Expression::UntaggedOperator(UntaggedOperator {
                    op: UntaggedOperatorName::And,
                    args: current_match,
                })),
            })];
            out.push(Stage::Match(MatchStage {
                expr,
                numbering: None,
            }));
        }
        Pipeline { pipeline: out }
    }
}

pub fn rewrite_match_move(pipeline: Pipeline) -> Pipeline {
    let mut visitor = MatchSplitter;
    let mut pipeline = visitor.visit_pipeline(pipeline);
    let mut visitor = SubpipelineFlatten;
    pipeline = visitor.visit_pipeline(pipeline);
    let mut changed = true;
    while changed {
        let mut visitor = MatchMover;
        pipeline = visitor.visit_pipeline(pipeline);
        let mut visitor = SubpipelineMatchMover { changed: false };
        pipeline = visitor.visit_pipeline(pipeline);
        changed = visitor.changed;
    }
    let mut visitor = MatchCoalescer;
    visitor.visit_pipeline(pipeline)
}
