use apollo_compiler::{Schema as GraphQLSchema, ast::Type as GraphQLType};
use schemars::{Schema as JSONSchema, json_schema};
use serde_json::{Map, Value};

use crate::custom_scalar_map::CustomScalarMap;

use super::name::Name;

pub(super) struct Type<'a> {
    /// The definition cache which contains full schemas for nested types
    pub(super) cache: &'a mut Map<String, Value>,

    /// Custom scalar map for supplementing information from the GraphQL schema
    pub(super) custom_scalar_map: Option<&'a CustomScalarMap>,

    /// The optional description of the type, from comments in the schema
    pub(super) description: &'a Option<String>,

    /// The original GraphQL schema with all type information
    pub(super) schema: &'a GraphQLSchema,

    /// The actual type to translate into a JSON schema
    pub(super) r#type: &'a GraphQLType,
}

/// Mark a JSON Schema as explicitly accepting `null` in addition to `inner`.
///
/// GraphQL nullable variables remain optional in the generated object (they are
/// omitted from `required`), but strict function-calling clients rewrite every
/// property into `required`. An explicit `oneOf` null branch keeps `null` valid
/// after that transform without changing non-null types.
///
/// Open schemas (`{}` or description-only) already accept `null`. Wrapping
/// those in `oneOf` with a null branch makes `null` match both sides and fail
/// validation. `$ref` is always wrapped; unmapped custom-scalar definitions
/// exclude null so that union stays valid.
fn nullable(inner: JSONSchema) -> JSONSchema {
    if already_allows_null(inner.as_value()) {
        return inner;
    }

    json_schema!({
        "oneOf": [
            inner,
            {"type": "null"},
        ]
    })
}

fn type_includes_null(ty: &Value) -> bool {
    match ty {
        Value::String(s) => s == "null",
        Value::Array(types) => types.iter().any(|t| t.as_str() == Some("null")),
        _ => false,
    }
}

fn already_allows_null(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };

    // Always wrap `$ref`. Input-object placeholders are empty while fields
    // are being walked; following them here would drop the null union from
    // recursive nullable fields. Open custom-scalar definitions are adjusted
    // separately so `oneOf` + `$ref` still accepts explicit null.
    if obj.contains_key("$ref") {
        return false;
    }

    if let Some(ty) = obj.get("type") {
        return type_includes_null(ty);
    }

    if let Some(one_of) = obj
        .get("oneOf")
        .or_else(|| obj.get("anyOf"))
        .and_then(Value::as_array)
    {
        return one_of.iter().any(already_allows_null);
    }

    if let Some(values) = obj.get("enum").and_then(Value::as_array) {
        return values.iter().any(Value::is_null);
    }

    if let Some(constant) = obj.get("const") {
        return constant.is_null();
    }

    true
}

impl From<Type<'_>> for JSONSchema {
    fn from(
        Type {
            cache,
            custom_scalar_map,
            description,
            schema,
            r#type,
        }: Type,
    ) -> Self {
        match r#type {
            GraphQLType::List(list) => {
                let items: JSONSchema = Type {
                    cache,
                    custom_scalar_map,
                    description,
                    schema,
                    r#type: list,
                }
                .into();

                nullable(json_schema!({
                    "type": "array",
                    "items": items,
                }))
            }

            GraphQLType::NonNullList(list) => {
                let items: JSONSchema = Type {
                    cache,
                    custom_scalar_map,
                    description,
                    schema,
                    r#type: list,
                }
                .into();

                json_schema!({
                    "type": "array",
                    "items": items,
                })
            }

            GraphQLType::Named(name) => nullable(JSONSchema::from(Name {
                cache,
                custom_scalar_map,
                description,
                name,
                schema,
            })),

            GraphQLType::NonNullNamed(name) => JSONSchema::from(Name {
                cache,
                custom_scalar_map,
                description,
                name,
                schema,
            }),
        }
    }
}
