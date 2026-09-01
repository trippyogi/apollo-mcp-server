---
default: patch
---

# Include relevant custom directive definitions in introspect results

The `introspect` tool now returns custom directive definitions that apply to the retained schema slice, not just the type SDL that uses them. Unused and built-in directive definitions are still omitted. In minify mode, those definitions are returned as standard GraphQL SDL because there is no minified directive-definition contract.
