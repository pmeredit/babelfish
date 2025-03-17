macro_rules! test_expression_conjunctive_normal_form {
    ($func_name:ident, expected = $expected:expr, input = $input:expr) => {
        #[test]
        fn $func_name() {
            use crate::definitions::Expression;

            let input: Expression = serde_json::from_str($input).unwrap();
            let expected: Expression = serde_json::from_str($expected).unwrap();
            let result = input.get_conjunctive_normal_form();
            assert_eq!(expected, result);
        }
    };
}

#[cfg(test)]
mod constants {
    test_expression_conjunctive_normal_form!(literal, expected = r#"2"#, input = r#"2"#);

    test_expression_conjunctive_normal_form!(array, expected = r#"[2]"#, input = r#"[2]"#);

    test_expression_conjunctive_normal_form!(
        document,
        expected = r#"{"a": 2}"#,
        input = r#"{"a": 2}"#
    );
}

#[cfg(test)]
mod logical {
    test_expression_conjunctive_normal_form!(
        simple_or,
        expected = r#"{"$not": {"$and": [{"$gt": ["$foo", null]}, {"$ne": ["$foo", 0]}, {"$ne": ["$foo", false]}]}}"#,
        input = r#"{"$or": [{"$lte": ["$foo", null]}, {"$eq": ["$foo", 0]}, {"$eq": ["$foo", false]}]}"#
    );

    test_expression_conjunctive_normal_form!(
        nested_or,
        expected = r#"{"$not": {"$and": [{"$gt": ["$foo", {"$not": {"$and": [false, true]}}]}, {"$ne": ["$foo", 0]}, {"$ne": ["$foo", false]}]}}"#,
        input = r#"{"$or": [{"$lte": ["$foo", {"$or": [true, false]}]}, {"$eq": ["$foo", 0]}, {"$eq": ["$foo", false]}]}"#
    );
}
