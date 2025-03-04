use lazy_static::lazy_static;
use std::collections::HashSet;
lazy_static! {
    pub static ref COMPOUND_TYPES: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.insert("Box");
        s.insert("HashMap");
        s.insert("LinkedHashMap");
        s.insert("UniqueLinkedHashMap");
        s.insert("BindingTuple");
        s.insert("BTreeMap");
        s.insert("Option");
        s.insert("Vec");
        s
    };
}

/// convert_to_snake_case converts a &str assumed to be in camelCase
/// into snake case. It does this by replacing a capital letter, e.g., X,
/// with _x.
pub fn convert_to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for (i, c) in s.chars().enumerate() {
        if i == 0 {
            out.push_str(&c.to_lowercase().collect::<String>());
            continue;
        }
        if c.is_uppercase() {
            out.push('_');
            out.push_str(&c.to_lowercase().collect::<String>());
        } else {
            out.push(c);
        }
    }
    out
}
