// SPDX-License-Identifier: AGPL-3.0-or-later

//! OpenAPI 3.1 spec generator from the operation registry.

use axum::http::Method;
use serde_json::{json, Value};

/// Generate a complete OpenAPI 3.1.0 specification from the operation registry.
pub fn generate_openapi_spec() -> Value {
    let ops = crate::all_operations();
    let mut paths: serde_json::Map<String, Value> = serde_json::Map::new();

    for op in &ops {
        let path_key = op.rest_path.to_string();
        let method = method_str(&op.rest_method);
        let schema = (op.json_schema)();
        let schema_value = match serde_json::to_value(&schema) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(tool = %op.name, error = %e, "failed to serialize schema for OpenAPI");
                continue;
            }
        };

        let tag = derive_tag(op.rest_path);

        let mut operation = json!({
            "operationId": op.name,
            "summary": first_sentence(op.description),
            "description": op.description,
            "tags": [tag],
            "security": [{ "bearerAuth": [op.scope] }],
        });

        let mut parameters = extract_path_params(op.rest_path);

        if method == "get" {
            parameters.extend(extract_query_params(&schema_value));
        }

        if !parameters.is_empty() {
            operation["parameters"] = Value::Array(parameters);
        }

        if method == "post" || method == "patch" {
            operation["requestBody"] = json!({
                "required": true,
                "content": {
                    "application/json": { "schema": schema_value }
                }
            });
        }

        operation["responses"] = build_responses(method == "post");

        let path_entry = paths
            .entry(path_key)
            .or_insert_with(|| json!({}));
        path_entry[method] = operation;
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Trakkt API",
            "version": "0.1.0",
            "description": "REST API for the Trakkt issue tracker. All endpoints require Bearer authentication (OAuth 2.0 JWT or API token)."
        },
        "paths": paths,
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                    "description": "OAuth 2.0 JWT access token or API token"
                }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string", "description": "Error message" }
                    },
                    "required": ["error"]
                }
            }
        }
    })
}

fn method_str(m: &Method) -> &'static str {
    match *m {
        Method::GET => "get",
        Method::POST => "post",
        Method::PATCH => "patch",
        Method::DELETE => "delete",
        Method::PUT => "put",
        _ => "get",
    }
}

fn derive_tag(rest_path: &str) -> &'static str {
    if rest_path.contains("/milestones") {
        return "Milestones";
    }
    if rest_path.contains("/relations") {
        return "Relations";
    }
    if rest_path.contains("/comments") {
        return "Comments";
    }
    if rest_path.starts_with("/issues") {
        return "Issues";
    }
    if rest_path.starts_with("/labels") {
        return "Labels";
    }
    if rest_path.starts_with("/teams") {
        return "Teams";
    }
    if rest_path.starts_with("/statuses") {
        return "Statuses";
    }
    if rest_path.starts_with("/projects") {
        return "Projects";
    }
    "Other"
}

fn first_sentence(desc: &str) -> &str {
    desc.split('.').next().unwrap_or(desc).trim()
}

fn extract_path_params(rest_path: &str) -> Vec<Value> {
    rest_path
        .split('/')
        .filter(|s| s.starts_with('{') && s.ends_with('}'))
        .map(|s| {
            let name = &s[1..s.len() - 1];
            json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" }
            })
        })
        .collect()
}

fn extract_query_params(schema_value: &Value) -> Vec<Value> {
    let Some(properties) = schema_value.pointer("/properties").and_then(|v| v.as_object()) else {
        return vec![];
    };

    let required_fields: Vec<String> = schema_value
        .pointer("/required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    properties
        .iter()
        .map(|(name, prop_schema)| {
            json!({
                "name": name,
                "in": "query",
                "required": required_fields.contains(name),
                "schema": prop_schema,
            })
        })
        .collect()
}

fn build_responses(is_post: bool) -> Value {
    let mut responses = json!({
        "200": { "description": "Success", "content": { "application/json": { "schema": { "type": "object" } } } },
        "400": { "description": "Bad request", "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } },
        "401": { "description": "Authentication required" },
        "403": { "description": "Insufficient permissions" },
        "404": { "description": "Not found" },
    });
    if is_post {
        responses["201"] = json!({
            "description": "Created",
            "content": { "application/json": { "schema": { "type": "object" } } }
        });
    }
    responses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_version() {
        let spec = generate_openapi_spec();
        assert_eq!(spec["openapi"], "3.1.0");
    }

    #[test]
    fn info_fields() {
        let spec = generate_openapi_spec();
        assert_eq!(spec["info"]["title"], "Trakkt API");
        assert_eq!(spec["info"]["version"], "0.1.0");
    }

    #[test]
    fn all_23_operations_present() {
        let spec = generate_openapi_spec();
        let paths = spec["paths"].as_object().expect("paths should be an object");

        let mut operation_ids: Vec<String> = Vec::new();
        for (_path, methods) in paths {
            if let Some(obj) = methods.as_object() {
                for (_method, op) in obj {
                    if let Some(id) = op["operationId"].as_str() {
                        operation_ids.push(id.to_string());
                    }
                }
            }
        }
        operation_ids.sort();

        assert_eq!(operation_ids.len(), 23, "expected 23 operations, got {}: {operation_ids:?}", operation_ids.len());
    }

    #[test]
    fn path_params_documented() {
        let spec = generate_openapi_spec();
        let get_issue = &spec["paths"]["/issues/{identifier}"]["get"];
        let params = get_issue["parameters"].as_array().expect("should have parameters");
        let has_identifier = params.iter().any(|p| {
            p["name"] == "identifier" && p["in"] == "path"
        });
        assert!(has_identifier, "get_issue should have {{identifier}} path param");
    }

    #[test]
    fn post_has_request_body() {
        let spec = generate_openapi_spec();
        let create_issue = &spec["paths"]["/issues"]["post"];
        assert!(create_issue["requestBody"].is_object(), "POST should have requestBody");
    }

    #[test]
    fn post_has_201_response() {
        let spec = generate_openapi_spec();
        let create_issue = &spec["paths"]["/issues"]["post"];
        assert!(create_issue["responses"]["201"].is_object(), "POST should have 201 response");
    }

    #[test]
    fn security_scheme_defined() {
        let spec = generate_openapi_spec();
        let scheme = &spec["components"]["securitySchemes"]["bearerAuth"];
        assert_eq!(scheme["type"], "http");
        assert_eq!(scheme["scheme"], "bearer");
    }

    #[test]
    fn tag_derivation() {
        assert_eq!(derive_tag("/issues"), "Issues");
        assert_eq!(derive_tag("/issues/{identifier}"), "Issues");
        assert_eq!(derive_tag("/issues/{identifier}/comments"), "Comments");
        assert_eq!(derive_tag("/issues/{identifier}/relations"), "Relations");
        assert_eq!(derive_tag("/projects"), "Projects");
        assert_eq!(derive_tag("/projects/{id}/milestones"), "Milestones");
        assert_eq!(derive_tag("/milestones/{id}"), "Milestones");
        assert_eq!(derive_tag("/labels"), "Labels");
    }
}
