use schema::Schema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Erd(HashMap<String, ErdItem>);

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErdItem {
    source: Source,
    primary_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    relationships: Vec<ErdRelationship>,
    #[serde(serialize_with = "schema::serialize_json_schema")]
    #[serde(deserialize_with = "schema::deserialize_json_schema")]
    json_schema: Schema,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    db: String,
    collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    projection: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErdRelationship {
    foreign_entity: String,
    relationship_type: RelationshipType,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    consistency: Option<Consistency>,
    constraint: Constraint,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum RelationshipType {
    #[serde(rename = "one-to-one")]
    OneToOne,
    #[serde(rename = "many-to-one")]
    ManyToOne,
    #[serde(rename = "many-to-many")]
    ManyToMany,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum Consistency {
    #[serde(rename = "strong")]
    Strong,
    #[serde(rename = "weak")]
    Weak,
    #[serde(rename = "eventual")]
    Eventual,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Constraint {
    constraint_type: ConstraintType,
    #[serde(skip_serializing_if = "Option::is_none")]
    direction: Option<ConstraintDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    foreign_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    projection: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConstraintType {
    Foreign,
    Embedded,
    Root,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConstraintDirection {
    Parent,
    Child,
}
