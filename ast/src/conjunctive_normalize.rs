use crate::{
    definitions::{visitor::Visitor, Expression, UntaggedOperator, UntaggedOperatorName},
    negative_normalize::NegativeNormalize,
};

struct CNFVisitor;

impl Visitor for CNFVisitor {
    fn visit_expression(&mut self, expression: Expression) -> Expression {
        match expression {
            Expression::UntaggedOperator(UntaggedOperator {
                op: UntaggedOperatorName::Or,
                args,
            }) => {
                let args = args
                    .into_iter()
                    .map(|operand| self.visit_expression(operand).get_negation())
                    .collect::<Vec<_>>();
                Expression::UntaggedOperator(UntaggedOperator {
                    op: UntaggedOperatorName::And,
                    args,
                })
            }
            _ => expression.walk(self),
        }
    }
}

impl Expression {
    pub fn conjunctive_normal_form(&self) -> Expression {
        CNFVisitor.visit_expression(self.clone())
    }
}
