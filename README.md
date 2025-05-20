# Babelfish

A Rust-based tool for generating and assembling MongoDB documents based on entity relationships and storage constraints, with intelligent MongoDB aggregation pipeline generation.

## Overview

The babelfish project introduces the $assemble pipeline stage to bridge the gap between normalized entity-relationship data models and MongoDB's document-oriented storage. It provides a declarative way to:

1. Define entity relationships and storage constraints in a schema
2. Generate physical MongoDB documents based on those constraints
3. Assemble logical views of data by joining entities as needed
4. Generate optimized MongoDB aggregation pipelines to query the data

The tool automatically analyzes document structures and relationships to determine the most efficient way to retrieve data based on your queries.

## Key Concepts

### Storage Constraints as an Abstraction Layer

One of the key architectural features of the MongoDB Document Assembler is how it uses storage constraints to create an abstraction layer between the logical data model and the physical document storage:

![MongoDB Document Assembler - Storage Abstraction Layers](./assets/storage-abstraction-diagram.svg)

This abstraction provides several benefits:

1. **Decoupling**: Assembly queries can be written against the logical schema without knowledge of physical storage
2. **Flexibility**: The physical storage structure can be changed by modifying storage constraints without impacting assembly queries
3. **Optimization**: The system automatically determines optimal collections to query based on storage constraints
4. **Evolution**: As your data model evolves, you can modify storage constraints to optimize for new access patterns while maintaining backward compatibility

### Schema Definition

The core of the system is the schema definition (new_schema.json), which contains:

- **Entities**: Represent business objects like Customer, Account, Order, Invoice, etc.
- **JSON Schema**: Defines the expected structure and validation rules for each entity
- **References**: Define relationships between entities
- **Storage Constraints**: Specify how data should be physically stored:
  - **Embedding**: Embed data from one entity into another
  - **Reference**: Store references to other documents
  - **Bucket**: Group child documents into buckets within a parent

The schema now uses MongoDB's JSON Schema format, providing better integration with MongoDB's validation capabilities.

# Note recently updated to also support a new concise form and new Schema format specified in the
assets/natty_join_test.json and assets/new_erd respectively. The old $assemble stage and ERD
described in this README are also still supported.

### Storage Constraint Types

The system supports three main types of storage constraints:

#### 1. Embedding Constraints

Embedding constraints specify how to embed data from one entity into another. They have a `direction` property:

- **Parent**: Child entity data is embedded in the parent entity
- **Child**: Parent entity data is embedded in the child entity

Example configuration:
```json
{
  "constraintType": "embedded",
  "consistency": "strong",
  "direction": "child",
  "targetPath": "contact",
  "projection": ["customerName", "customerAddress"]
}
```

Note that in the new schema format, constraint types are now lowercase ("embedded" instead of "Embedding").

#### 2. Reference Constraints

Reference constraints store just the ID or a reference value from one entity to another.

Example configuration:
```json
{
  "constraintType": "reference",
  "consistency": "strong",
  "direction": "child",
  "targetPath": "customerId",
  "extendedProperties": {
    "blueprint": "sourceId#ISOTIME"
  }
}
```

#### 3. Bucket Constraints

Bucket constraints allow grouping multiple child documents into arrays within a parent document, based on time or volume dimensions.

Example configuration:
```json
{
  "constraintType": "bucket",
  "consistency": "strong",
  "direction": "child",
  "targetPath": "events",
  "extendedProperties": {
    "dimension": "time",
    "size": 10
  }
}
```

### Assembly Configuration

The assembly configuration is used by the `$assemble` operator within a MongoDB pipeline, defined as follows:

```json
{
  "$assemble": {
    "erd": "./assets/schema.json",
    "entity": "Customer",
    "project": ["customerAddress", "customerName"],
    "subassemble": [...]
  }
}
```

Key changes in the assembly format:
- Uses `$assemble` within a standard MongoDB pipeline
- Filters use MongoDB's `$expr` format for more consistent expression handling
- Can be combined with other MongoDB pipeline stages like `$limit` and `$skip`

## Installation




### Running the Tool

The tool parses the specified $assemble config and generates the appropriate pipeline based on the specified schema or validates the specified schema file.

// to run $assemble pipeline generation and optimization

cargo run --bin babelfish-cli -- -p assets/simple_test.json

cargo run --bin babelfish-cli -- -p assets/simple_test1.json

^^ show embedded vs reference

// to parse an ERD

cargo run --bin babelfish-cli -- -e assets/simple_schema.json

## Schema and Assembly Examples

### Schema Example (MongoDB JSON Schema Format)

```json
{
  "schemaName": "company_erd",
  "entities": {
    "Account": {
      "db": "company",
      "collection": "Account",
      "primaryKey": "_id",
      "jsonSchema": {
        "bsonType": "object",
        "properties": {
          "_id": { "bsonType": "objectId" },
          "accountId": { "bsonType": "string" },
          "accountName": { "bsonType": "string" },
          "customerId": { "bsonType": "objectId" }
        },
        "references": {
          "customerId": {
            "entity": "Customer",
            "field": "_id",
            "relationshipType": "many-one",
            "storageConstraints": [
              {
                "constraintType": "embedded",
                "consistency": "strong",
                "direction": "child",
                "targetPath": "contact",
                "projection": ["customerName", "customerAddress"]
              }
            ]
          }
        },
        "required": ["_id", "accountId", "accountName", "customerId"],
        "additionalProperties": false
      }
    },
    "Customer": {
      "db": "company",
      "collection": "Customer",
      "primaryKey": "_id",
      "jsonSchema": {
        "bsonType": "object",
        "properties": {
          "_id": { "bsonType": "objectId" },
          "customerName": { "bsonType": "string" },
          "customerAddress": { "bsonType": "string" }
        },
        "required": ["_id", "customerName", "customerAddress"],
        "additionalProperties": false
      }
    }
    // Other entities follow the same pattern...
  }
}
```

### Assembly Example

```json
[
  {
    "$assemble": {
      "erd": "./assets/schema.json",
      "entity": "Customer",
      "project": ["customerAddress", "customerName"],
      "subassemble": [{
        "entity": "Account",
        "join": "inner",
        "project": ["accountName"],
        "subassemble": [{
          "entity": "Order",
          "join": "inner",
          "project": ["amount"],
          "filter": { "$gte": ["$Order.amount", 100] }
        }]
      }]
    }
  },
  { "$limit": 10 },
  { "$skip": 0 }
]
```

## Advanced Features

### Projection Control

Each subassembly in the assembly configuration can include a `project` property to control which fields are included:

```json
{
  "entity": "Order",
  "join": "inner",
  "filter": { "$gte": ["$Order.amount", 100] },
  "project": ["orderId", "amount"],  // Only include these fields
  "subassemble": [...]
}
```

The behavior of the `project` property:
- If omitted (null): Include all fields from the entity
- If empty array ([]): Include no fields (only _id)
- If specified with field names: Include only those specific fields

### Using MongoDB Expression Operators

The new assembly format supports MongoDB's expression operators in filters:

```json
"filter": { 
  "$and": [
    { "$gte": ["$Order.amount", 100] },
    { "$lte": ["$Order.amount", 1000] }
  ]
}
```

This allows for more complex filtering conditions and better compatibility with MongoDB's query capabilities.

### Integration with MongoDB Pipelines

The `$assemble` operator can be used as part of a larger MongoDB aggregation pipeline:

```json
[
  { "$match": { "customerStatus": "active" } },
  { "$assemble": { ... } },
  { "$sort": { "Customer.customerName": 1 } },
  { "$limit": 10 }
]
```

This enables combining the document assembly capabilities with MongoDB's rich aggregation framework.
