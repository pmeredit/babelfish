use schema::Schema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Erd(HashMap<String, ErdItem>);

impl Erd {
    pub fn get_relationship(&self, entity: &str, foreign_entity: &str) -> Option<&ErdRelationship> {
        self.0.get(entity).and_then(|item| {
            item.relationships.get(foreign_entity)
        })
    }

    pub fn get_source(&self, entity: &str) -> Option<&Source> {
        self.0.get(entity).map(|item| &item.source)
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErdItem {
    pub source: Source,
    pub primary_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub relationships: HashMap<String, ErdRelationship>,
    #[serde(serialize_with = "schema::serialize_json_schema")]
    #[serde(deserialize_with = "schema::deserialize_json_schema")]
    pub json_schema: Schema,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub db: String,
    pub collection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ErdRelationship {
    pub relationship_type: RelationshipType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consistency: Option<Consistency>,
    pub constraint: Constraint,
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
    pub constraint_type: ConstraintType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<ConstraintDirection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreign_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projection: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConstraintType {
    Foreign,
    Embedded,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ConstraintDirection {
    Parent,
    Child,
}
