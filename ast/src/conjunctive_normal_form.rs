use crate::definitions::{visitor::Visitor, Expression, UntaggedOperator, UntaggedOperatorName};

struct CNFVisitor;

impl Visitor for CNFVisitor {
    fn visit_expression(&mut self, expression: Expression) {
        match expression {
            Expression::UntaggedOperator(UntaggedOperator {
                op: UntaggedOperatorName::Or,
                args,
            }) => {
                let args = operands
                    .into_iter()
                    .map(|operand| match operand {})
                    .collect::<Vec<_>>();
                for operand in operands {
                    self.visit_expression(operand);
                }
            }
        }
    }
}

impl Expression {
    pub fn conjunctive_normal_form(&self) -> Expression {}
}
