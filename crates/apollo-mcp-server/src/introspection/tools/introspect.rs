use crate::errors::McpError;
use crate::introspection::minify::MinifyExt as _;
use crate::schema_from_type;
use crate::schema_tree_shake::{DepthLimit, SchemaTreeShaker};
use apollo_compiler::Schema;
use apollo_compiler::ast::OperationType;
use apollo_compiler::schema::ExtendedType;
use apollo_compiler::validation::Valid;
use rmcp::model::{CallToolResult, ContentBlock, Tool};
use rmcp::schemars::JsonSchema;
use rmcp::serde_json::Value;
use rmcp::{schemars, serde_json};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::description::append_description_hint;

/// The name of the tool to get GraphQL schema type information
pub const INTROSPECT_TOOL_NAME: &str = "introspect";

/// A tool to get detailed information about specific types from the GraphQL schema.
#[derive(Clone)]
pub struct Introspect {
    schema: Arc<RwLock<Valid<Schema>>>,
    allow_mutations: bool,
    minify: bool,
    pub tool: Tool,
}

/// Input for the introspect tool.
#[derive(JsonSchema, Deserialize, Debug)]
pub struct Input {
    /// The name of the type to get information about.
    type_name: String,
    /// How far to recurse the type hierarchy. Use 0 for no limit. Defaults to 1.
    #[serde(default = "default_depth")]
    depth: usize,
}

impl Introspect {
    pub fn new(
        schema: Arc<RwLock<Valid<Schema>>>,
        root_query_type: Option<String>,
        root_mutation_type: Option<String>,
        minify: bool,
        description_hint: Option<&str>,
    ) -> Self {
        let default_description = tool_description(
            root_query_type.as_deref(),
            root_mutation_type.as_deref(),
            minify,
        );
        let description =
            append_description_hint(&default_description, description_hint).into_owned();
        Self {
            schema,
            allow_mutations: root_mutation_type.is_some(),
            minify,
            tool: Tool::new(INTROSPECT_TOOL_NAME, description, schema_from_type!(Input)),
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn execute(&self, input: Input) -> Result<CallToolResult, McpError> {
        let schema = self.schema.read().await;
        let type_name = input.type_name.as_str();
        let mut tree_shaker = SchemaTreeShaker::new(&schema);
        match schema.types.get(type_name) {
            Some(extended_type) => tree_shaker.retain_type(
                extended_type,
                None,
                if input.depth > 0 {
                    DepthLimit::Limited(input.depth)
                } else {
                    DepthLimit::Unlimited
                },
            ),
            None => {
                return Ok(CallToolResult::success(vec![]));
            }
        }
        let shaken = tree_shaker.shaken().unwrap_or_else(|schema| schema.partial);

        // The tree shaker already retains used custom directive definitions
        // (and their argument types). Project those into the response; types
        // alone hide the definition even when the application is visible.
        let directives = shaken
            .directive_definitions
            .iter()
            .filter(|(_, def)| !def.is_built_in())
            .map(|(_, def)| ContentBlock::text(def.serialize().to_string()));

        let types = shaken
            .types
            .iter()
            .filter(|(_, extended_type)| {
                !extended_type.is_built_in()
                    && schema
                        .root_operation(OperationType::Mutation)
                        .is_none_or(|root_name| {
                            // Allow introspection of the mutation type itself even when mutations are disabled
                            extended_type.name() != root_name
                                || type_name == root_name.as_str()
                                || self.allow_mutations
                        })
                    && schema
                        .root_operation(OperationType::Subscription)
                        .is_none_or(|root_name| extended_type.name() != root_name)
            })
            .map(|(_, extended_type)| ContentBlock::text(self.serialize(extended_type)));

        Ok(CallToolResult::success(directives.chain(types).collect()))
    }

    fn serialize(&self, extended_type: &ExtendedType) -> String {
        if self.minify {
            extended_type.minify()
        } else {
            extended_type.serialize().to_string()
        }
    }
}

fn tool_description(
    root_query_type: Option<&str>,
    root_mutation_type: Option<&str>,
    minify: bool,
) -> String {
    if minify {
        "Get GraphQL type information - T=type,I=input,E=enum,U=union,F=interface;s=String,i=Int,f=Float,b=Boolean,d=ID;@D=deprecated;!=required,[]=list,<>=implements;".to_string()
    } else {
        format!(
            "Get information about a given GraphQL type defined in the schema. Instructions: Use this tool to explore the schema by providing specific type names. Start with the root query ({}) or mutation ({}) types to discover available fields. If the search tool is also available, use this tool first to get the fields, then use the search tool with relevant field return types and argument input types (ignore default GraphQL scalars) as search terms.",
            root_query_type.unwrap_or("Query"),
            root_mutation_type.unwrap_or("Mutation")
        )
    }
}

/// The default depth to recurse the type hierarchy.
fn default_depth() -> usize {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_compiler::Schema;
    use apollo_compiler::validation::Valid;
    use rstest::{fixture, rstest};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    const TEST_SCHEMA: &str = include_str!("testdata/schema.graphql");

    #[fixture]
    fn schema() -> Arc<RwLock<Valid<Schema>>> {
        Arc::new(RwLock::new(
            Schema::parse(TEST_SCHEMA, "schema.graphql")
                .expect("Failed to parse test schema")
                .validate()
                .expect("Failed to validate test schema"),
        ))
    }

    #[rstest]
    #[tokio::test]
    async fn introspect_tool_description_is_not_minified(schema: Arc<RwLock<Valid<Schema>>>) {
        let introspect = Introspect::new(schema, None, None, false, None);

        let description = introspect.tool.description.unwrap();

        assert!(
            description
                .contains("Get information about a given GraphQL type defined in the schema")
        );
        assert!(description.contains("Instructions: Use this tool to explore the schema"));
        // Should not contain minification legend
        assert!(!description.contains("T=type,I=input"));
        // Should mention conditional search tool usage
        assert!(description.contains("If the search tool is also available"));
    }

    #[rstest]
    #[tokio::test]
    async fn introspect_tool_description_is_minified_with_an_appropriate_legend(
        schema: Arc<RwLock<Valid<Schema>>>,
    ) {
        let introspect = Introspect::new(schema, None, None, true, None);

        let description = introspect.tool.description.unwrap();

        // Should contain minification legend
        assert!(description.contains("T=type,I=input,E=enum,U=union,F=interface"));
        assert!(description.contains("s=String,i=Int,f=Float,b=Boolean,d=ID"));
    }

    #[rstest]
    #[tokio::test]
    async fn introspect_query_depth_1_returns_fields(schema: Arc<RwLock<Valid<Schema>>>) {
        let introspect = Introspect::new(
            schema,
            Some("Query".to_string()),
            Some("Mutation".to_string()),
            false,
            None,
        );

        let result = introspect
            .execute(Input {
                type_name: "Query".to_string(),
                depth: 1,
            })
            .await
            .expect("Introspect execution failed");

        let content = result
            .content
            .iter()
            .filter_map(|c| {
                use rmcp::model::ContentBlock;
                match c {
                    ContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                }
            })
            .collect::<Vec<String>>()
            .join("\n");

        // Query with depth 1 should return the Query type with its fields
        assert!(!result.content.is_empty());
        assert!(content.contains("type Query"));
    }

    #[rstest]
    #[tokio::test]
    async fn introspect_mutation_depth_1_returns_fields(schema: Arc<RwLock<Valid<Schema>>>) {
        let introspect = Introspect::new(
            schema,
            Some("Query".to_string()),
            Some("Mutation".to_string()),
            false,
            None,
        );

        let result = introspect
            .execute(Input {
                type_name: "Mutation".to_string(),
                depth: 1,
            })
            .await
            .expect("Introspect execution failed");

        let content = result
            .content
            .iter()
            .filter_map(|c| {
                use rmcp::model::ContentBlock;
                match c {
                    ContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                }
            })
            .collect::<Vec<String>>()
            .join("\n");

        // Mutation with depth 1 should return the Mutation type with its fields, just like Query
        assert!(
            !result.content.is_empty(),
            "Mutation introspection should return content"
        );
        assert!(
            content.contains("type Mutation"),
            "Should contain Mutation type definition"
        );
    }

    #[rstest]
    #[tokio::test]
    async fn introspect_mutation_depth_1_with_mutations_disabled(
        schema: Arc<RwLock<Valid<Schema>>>,
    ) {
        // This test verifies the fix: when mutations are not allowed, mutation introspection should still work
        let introspect = Introspect::new(schema, Some("Query".to_string()), None, false, None);

        let result = introspect
            .execute(Input {
                type_name: "Mutation".to_string(),
                depth: 1,
            })
            .await
            .expect("Introspect execution failed");

        let content = result
            .content
            .iter()
            .filter_map(|c| {
                use rmcp::model::ContentBlock;
                match c {
                    ContentBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                }
            })
            .collect::<Vec<String>>()
            .join("\n");

        // After the fix: mutation introspection should work even when mutations are disabled
        assert!(
            !result.content.is_empty(),
            "Mutation introspection should return content even when mutations are disabled"
        );
        assert!(
            content.contains("type Mutation"),
            "Should contain Mutation type definition"
        );
    }

    fn parse_schema(sdl: &str) -> Arc<RwLock<Valid<Schema>>> {
        Arc::new(RwLock::new(
            Schema::parse(sdl, "schema.graphql")
                .expect("Failed to parse schema")
                .validate()
                .expect("Failed to validate schema"),
        ))
    }

    fn text_content(result: &CallToolResult) -> Vec<String> {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect()
    }

    async fn introspect_type(
        sdl: &str,
        type_name: &str,
        depth: usize,
        minify: bool,
    ) -> Vec<String> {
        let schema = parse_schema(sdl);
        let introspect = Introspect::new(schema, Some("Query".to_string()), None, minify, None);
        let result = introspect
            .execute(Input {
                type_name: type_name.to_string(),
                depth,
            })
            .await
            .expect("Introspect execution failed");
        text_content(&result)
    }

    #[tokio::test]
    async fn tree_shaker_retains_used_directive_definition() {
        let sdl = r#"
            directive @auth(role: String!) on FIELD_DEFINITION
            directive @unused(reason: String) on OBJECT

            type Query {
              secret: String @auth(role: "admin")
            }
        "#;
        let schema = parse_schema(sdl);
        let schema_guard = schema.read().await;
        let mut tree_shaker = SchemaTreeShaker::new(&schema_guard);
        let query = schema_guard
            .types
            .get("Query")
            .expect("Query type must exist");
        tree_shaker.retain_type(query, None, DepthLimit::Limited(1));
        let shaken = tree_shaker.shaken().unwrap_or_else(|schema| schema.partial);
        let shaken_sdl = shaken.to_string();
        assert!(
            shaken_sdl.contains("directive @auth"),
            "tree shaker already retains used custom directives: {shaken_sdl}"
        );
        assert!(
            !shaken_sdl.contains("directive @unused"),
            "tree shaker must omit unused custom directives: {shaken_sdl}"
        );
    }

    #[tokio::test]
    async fn field_applied_custom_directive_definition_is_returned() {
        let content = introspect_type(
            r#"
            directive @auth(role: String!) on FIELD_DEFINITION
            directive @unused(reason: String) on OBJECT

            type Query {
              secret: String @auth(role: "admin")
            }
            "#,
            "Query",
            1,
            false,
        )
        .await;
        let joined = content.join("\n");
        assert!(
            joined.contains("directive @auth(role: String!) on FIELD_DEFINITION"),
            "missing @auth definition: {joined}"
        );
        assert!(
            joined.contains("@auth(role: \"admin\")"),
            "missing @auth application: {joined}"
        );
        assert!(
            !joined.contains("directive @unused"),
            "unused directive must be omitted: {joined}"
        );
        assert_eq!(
            content
                .iter()
                .filter(|block| block.contains("directive @auth"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn type_applied_custom_directive_definition_is_returned() {
        let content = introspect_type(
            r#"
            directive @owner(team: String!) on OBJECT

            type Query @owner(team: "platform") {
              id: ID
            }
            "#,
            "Query",
            1,
            false,
        )
        .await;
        let joined = content.join("\n");
        assert!(
            joined.contains("directive @owner(team: String!) on OBJECT"),
            "{joined}"
        );
        assert!(joined.contains("@owner(team: \"platform\")"), "{joined}");
    }

    #[tokio::test]
    async fn directive_argument_custom_type_is_retained() {
        // SchemaTreeShaker decrements depth once for directive argument types
        // and again for nested input fields. Depth 1 therefore keeps @auth but
        // not AuthContext/Role; depth 3 matches should_retain_custom_directives.
        let content = introspect_type(
            r#"
            enum Role { ADMIN USER }
            input AuthContext { role: Role! }
            directive @auth(ctx: AuthContext!) on FIELD_DEFINITION

            type Query {
              secret: String @auth(ctx: { role: ADMIN })
            }
            "#,
            "Query",
            3,
            false,
        )
        .await;
        let joined = content.join("\n");
        assert!(
            joined.contains("directive @auth(ctx: AuthContext!) on FIELD_DEFINITION"),
            "{joined}"
        );
        assert!(joined.contains("enum Role"), "{joined}");
        assert!(joined.contains("input AuthContext"), "{joined}");
    }

    #[tokio::test]
    async fn repeated_application_returns_one_definition() {
        let content = introspect_type(
            r#"
            directive @auth(role: String!) on FIELD_DEFINITION

            type Query {
              a: String @auth(role: "admin")
              b: String @auth(role: "user")
            }
            "#,
            "Query",
            1,
            false,
        )
        .await;
        assert_eq!(
            content
                .iter()
                .filter(|block| block.contains("directive @auth"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn builtin_directive_definitions_are_not_dumped() {
        let content = introspect_type(
            r#"
            type Query {
              legacy: String @deprecated(reason: "use secret")
              secret: String
            }
            "#,
            "Query",
            1,
            false,
        )
        .await;
        let joined = content.join("\n");
        assert!(joined.contains("@deprecated"), "{joined}");
        assert!(
            !joined.contains("directive @deprecated"),
            "built-in directive definitions must not be dumped: {joined}"
        );
    }

    #[tokio::test]
    async fn nested_directive_respects_introspection_depth() {
        let sdl = r#"
            directive @auth(role: String!) on FIELD_DEFINITION

            type Query {
              user: User
            }

            type User {
              secret: String @auth(role: "admin")
            }
        "#;
        let shallow = introspect_type(sdl, "Query", 1, false).await.join("\n");
        assert!(
            !shallow.contains("directive @auth"),
            "depth 1 should not retain nested @auth: {shallow}"
        );

        let deep = introspect_type(sdl, "Query", 2, false).await.join("\n");
        assert!(
            deep.contains("directive @auth(role: String!) on FIELD_DEFINITION"),
            "depth 2 should retain nested @auth: {deep}"
        );
    }

    #[tokio::test]
    async fn regular_serialization_is_deterministic() {
        let sdl = r#"
            directive @auth(role: String!) on FIELD_DEFINITION
            type Query {
              secret: String @auth(role: "admin")
            }
        "#;
        let first = introspect_type(sdl, "Query", 1, false).await;
        let second = introspect_type(sdl, "Query", 1, false).await;
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn minify_returns_standard_sdl_directive_definition() {
        // MinifyExt only minifies type SDL and selected applications such as
        // @deprecated. There is no custom directive-definition minifier, so
        // definitions are returned as regular SDL even in minify mode.
        let content = introspect_type(
            r#"
            directive @auth(role: String!) on FIELD_DEFINITION
            type Query {
              secret: String @auth(role: "admin")
            }
            "#,
            "Query",
            1,
            true,
        )
        .await;
        let joined = content.join("\n");
        assert!(
            joined.contains("directive @auth(role: String!) on FIELD_DEFINITION"),
            "minify mode should still return standard SDL for custom directive definitions: {joined}"
        );
        assert!(
            content.iter().any(|block| block.starts_with("T:")),
            "retained types should still be minified: {joined}"
        );
    }
}
