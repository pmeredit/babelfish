use crate::{
    definitions::{
        visitor::Visitor, visitor_ref::VisitorRef, Expression, GetField, Lookup, ProjectItem, Ref,
        Stage, TaggedOperator, Unwind,
    },
    set,
};
use std::collections::{HashMap, HashSet};

pub struct Uses(HashSet<String>);

impl Uses {
    pub fn prefix_overlap(&self, other: &HashSet<String>) -> bool {
        self.0.iter().any(|s| {
            other.iter().any(|o| {
                if s == o {
                    true
                } else {
                    let prefix = format!("{}.", o);
                    s.starts_with(prefix.as_str())
                }
            })
        })
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

struct UsesVisitor {
    u: HashSet<String>,
}

impl VisitorRef for UsesVisitor {
    fn visit_expression(&mut self, expression: &Expression) {
        expression.walk_ref(self);
        if let Expression::Ref(Ref::FieldRef(s)) = expression {
            self.u.insert(s.clone());
        }
    }
}

struct VarUsesVisitor {
    u: HashSet<String>,
}

impl VisitorRef for VarUsesVisitor {
    fn visit_expression(&mut self, expression: &Expression) {
        expression.walk_ref(self);
        if let Expression::Ref(Ref::VariableRef(s)) = expression {
            self.u.insert(s.clone());
        }
    }
}

struct SubstituteVisitor {
    theta: HashMap<String, Expression>,
}

// TODO: we may want to use Refs for substitution rather than splitting implementation for
// Variables and Fields
impl Visitor for SubstituteVisitor {
    fn visit_expression(&mut self, expression: Expression) -> Expression {
        match expression {
            Expression::Ref(Ref::FieldRef(ref s)) => {
                let path = s.split('.').collect::<Vec<_>>();
                let mut current_path = path[0].to_string();
                for (i, part) in path.iter().enumerate().skip(1) {
                    if let Some(expr) = self.theta.get(&current_path) {
                        if i == path.len() - 1 {
                            return expr.clone();
                        }
                        let mut ret = expr.clone();
                        for part in path.iter().skip(i) {
                            ret = Expression::TaggedOperator(TaggedOperator::GetField(GetField {
                                input: Box::new(ret),
                                field: part.to_string(),
                            }));
                        }
                        return ret;
                    }
                    current_path = format!("{}.{}", current_path, part);
                }
                if let Some(expr) = self.theta.get(&current_path) {
                    return expr.clone();
                }
                expression
            }
            _ => expression.walk(self),
        }
    }
}

struct VarSubstituteVisitor {
    theta: HashMap<String, Expression>,
}

impl Visitor for VarSubstituteVisitor {
    fn visit_expression(&mut self, expression: Expression) -> Expression {
        match expression {
            Expression::Ref(Ref::VariableRef(ref s)) => {
                let path = s.split('.').collect::<Vec<_>>();
                let mut current_path = path[0].to_string();
                for (i, part) in path.iter().enumerate().skip(1) {
                    if let Some(expr) = self.theta.get(&current_path) {
                        if i == path.len() - 1 {
                            return expr.clone();
                        }
                        let mut ret = expr.clone();
                        for part in path.iter().skip(i) {
                            ret = Expression::TaggedOperator(TaggedOperator::GetField(GetField {
                                input: Box::new(ret),
                                field: part.to_string(),
                            }));
                        }
                        return ret;
                    }
                    current_path = format!("{}.{}", current_path, part);
                }
                if let Some(expr) = self.theta.get(&current_path) {
                    return expr.clone();
                }
                expression
            }
            _ => expression.walk(self),
        }
    }
}

impl Expression {
    pub fn uses(&self) -> Uses {
        let mut visitor = UsesVisitor { u: HashSet::new() };
        visitor.visit_expression(self);
        Uses(visitor.u)
    }

    pub fn variable_uses(&self) -> Uses {
        let mut visitor = VarUsesVisitor { u: HashSet::new() };
        visitor.visit_expression(self);
        Uses(visitor.u)
    }

    pub fn substitute(self, theta: HashMap<String, Expression>) -> Expression {
        let mut visitor = SubstituteVisitor { theta };
        visitor.visit_expression(self)
    }

    pub fn variable_substitute(self, theta: HashMap<String, Expression>) -> Expression {
        let mut visitor = VarSubstituteVisitor { theta };
        visitor.visit_expression(self)
    }
}

impl Stage {
    pub fn opaque_defines(&self) -> Option<HashSet<String>> {
        Some(match self {
            Stage::Group(group) => group
                .aggregations
                .keys()
                .cloned()
                .chain(["_id".to_string()].into_iter())
                .collect(),
            Stage::Lookup(Lookup::Equality(lookup)) => set![lookup.as_var.clone()],
            Stage::Lookup(Lookup::ConciseSubquery(lookup)) => set![lookup.as_var.clone()],
            Stage::Lookup(Lookup::Subquery(lookup)) => set![lookup.as_var.clone()],
            Stage::Unwind(Unwind::Document(expr)) => {
                let mut ret: HashSet<_> =
                    if let Expression::Ref(Ref::FieldRef(ref field)) = *expr.path {
                        set![field.clone()]
                    } else {
                        unreachable!()
                    };
                if let Some(include_array_index) = &expr.include_array_index {
                    ret.insert(include_array_index.clone());
                }
                ret
            }
            // TODO
            _ => None?,
        })
    }

    // For now a None defines means we cannot swap, TODO: decouple this?
    pub fn defines(&self) -> Option<HashMap<String, Expression>> {
        Some(match self {
            Stage::AddFields(fields) => {
                fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            }
            Stage::Project(stage) => stage
                .items
                .iter()
                .flat_map(|(k, v)| {
                    Some((
                        k.clone(),
                        match v {
                            ProjectItem::Assignment(expr) => expr.clone(),
                            ProjectItem::Inclusion => Expression::Ref(Ref::FieldRef(k.clone())),
                            _ => None?,
                        },
                    ))
                })
                .collect(),
            Stage::Match(_) | Stage::Lookup(_) | Stage::Unwind(_) => HashMap::new(),
            _ => None?,
        })
    }
}
