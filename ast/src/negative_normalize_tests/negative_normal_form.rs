use crate::definitions::MatchExpression;
use crate::negative_normalize::NegativeNormalize;

macro_rules! test_nnf {
    ($func_name:ident, expected = $expected:expr, input = $input:expr) => {
        #[test]
        fn $func_name() {
            let input: MatchExpression = serde_json::from_str($input).unwrap();
            let expected: MatchExpression = serde_json::from_str($expected).unwrap();
            let result = input.get_negative_normal_form();
            assert_eq!(result, expected);
        }
    };
}

test_nnf!(
    expr_func_noop,
    expected = r#"{"$expr": {"$eq": ["$foo", 0]}}"#,
    input = r#"{"$expr": {"$eq": ["$foo", 0]}}"#
);

test_nnf!(
    nor,
    expected = r#"{"$and": [{"foo": {"$ne": 0}}, {"bar": {"$lte": 42}}]}"#,
    input = r#"{"$nor": [{"foo": {"$eq": 0}}, {"bar": {"$gt": 42}}]}"#
);

test_nnf!(
    nor_propagated_to_nested,
    expected = r#"{"$and": [{"foo": {"$ne": 0}}, {"$and": [{"bar": {"$lte": 42}}, {"baz": {"$gt": 42}}]}]}"#,
    input = r#"{"$nor": [{"foo": {"$eq": 0}}, {"$or": [{"bar": {"$gt": 42}}, {"baz": {"$lte": 42}}]}]}"#
);

test_nnf!(
    nor_in_subexpr_normalized,
    expected = r#"{"$or": [{"foo": {"$eq": 0}}, {"$and": [{"bar": {"$lte": 42}}, {"baz": {"$gt": 42}}]}]}"#,
    input = r#"{"$or": [{"foo": {"$eq": 0}}, {"$nor": [{"bar": {"$gt": 42}}, {"baz": {"$lte": 42}}]}]}"#
);

test_nnf!(
    not_op,
    expected = r#"{"foo": {"$ne": 0}}"#,
    input = r#"{"foo": {"$not": {"$eq": 0}}}"#
);

test_nnf!(
    not_ops,
    expected = r#"{"$or": [{"foo": {"$ne": 0}}, {"foo": {"$lte": 42}}]}"#,
    input = r#"{"foo": {"$not": {"$eq": 0, "$gt": 42}}}"#
);

test_nnf!(
    not_regex_is_noop,
    expected = r#"{"foo": {"$not": {"$regex": "bar", "$options": "i"}}}"#,
    input = r#"{"foo": {"$not": {"$regex": "bar", "$options": "i"}}}"#
);

test_nnf!(
    not_expr_func_gets_negation,
    expected = r#"{"$expr": {"$ne": ["$bar", 1]}}"#,
    input = r#"{"$expr": {"$not": {"$eq": ["$bar", 1]}}}"#
);

test_nnf!(
    not_misc_elem_match,
    expected =
        r#"{"$or": [{"a": {"$elemMatch": {"$lte": 3}}}, {"a": {"$elemMatch": {"$ne": 5}}}]}"#,
    input = r#"{"a": {"$not": {"$elemMatch": {"$gt": 3, "$eq": 5}}}}"#
);
