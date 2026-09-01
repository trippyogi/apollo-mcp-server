---
default: patch
---

# Preserve GraphQL input nullability in generated MCP schemas

Generated tool input schemas now emit an explicit `oneOf` null union for nullable GraphQL variables (scalars, enums, input objects, custom scalars, and lists). Strict function-calling clients that mark every property required can send `null` instead of fabricated placeholders, while non-null variables and the existing `required` list are unchanged.
