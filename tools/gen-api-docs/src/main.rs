// SPDX-License-Identifier: AGPL-3.0-or-later

//! API documentation generator for Trakkt.
//!
//! Reads the OpenAPI 3.1.0 specification produced by `trakkt_api::openapi` and
//! generates one markdown file per API tag group. Also updates the mdBook
//! SUMMARY.md to include the generated pages.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn main() {
    let docs_dir = parse_docs_dir();
    let summary_path = docs_dir
        .parent()
        .expect("docs_dir should have a parent")
        .join("SUMMARY.md");

    let spec = trakkt_api::openapi::generate_openapi_spec();

    let groups = group_operations_by_tag(&spec);
    let mut generated_entries: Vec<(String, String)> = Vec::new();

    for (tag, operations) in &groups {
        let filename = tag_to_filename(tag);
        let markdown = render_tag_page(tag, operations);
        let out_path = docs_dir.join(&filename);
        std::fs::write(&out_path, &markdown)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", out_path.display()));
        eprintln!("  wrote {}", out_path.display());
        generated_entries.push((tag.clone(), filename));
    }

    update_summary(&summary_path, &generated_entries);
    eprintln!(
        "  updated {} ({} API reference entries)",
        summary_path.display(),
        generated_entries.len()
    );
}

/// Parse the `--docs-dir` argument, defaulting to `docs/book/src/api-reference/`.
fn parse_docs_dir() -> PathBuf {
    let args: Vec<String> = std::env::args().collect();
    let mut docs_dir: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--docs-dir" {
            i += 1;
            if i < args.len() {
                docs_dir = Some(PathBuf::from(&args[i]));
            } else {
                eprintln!("error: --docs-dir requires a value");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let dir = docs_dir.unwrap_or_else(|| PathBuf::from("docs/book/src/api-reference"));
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("failed to create docs dir {}: {e}", dir.display()));
    dir
}

/// A single parsed API operation from the OpenAPI spec.
struct Operation {
    operation_id: String,
    method: String,
    path: String,
    summary: String,
    description: String,
    parameters: Vec<Parameter>,
    request_body_schema: Option<Value>,
    is_post: bool,
}

struct Parameter {
    name: String,
    location: String,
    required: bool,
    param_type: String,
    description: String,
}

/// Group all operations by their tag, returning a sorted map.
fn group_operations_by_tag(spec: &Value) -> BTreeMap<String, Vec<Operation>> {
    let mut groups: BTreeMap<String, Vec<Operation>> = BTreeMap::new();

    let paths = spec["paths"]
        .as_object()
        .expect("spec should have paths object");

    for (path, methods) in paths {
        let methods_obj = match methods.as_object() {
            Some(o) => o,
            None => continue,
        };

        for (method, op_value) in methods_obj {
            let tag = op_value["tags"]
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
                .unwrap_or("Other")
                .to_string();

            let operation_id = op_value["operationId"]
                .as_str()
                .unwrap_or("")
                .to_string();

            let summary = op_value["summary"]
                .as_str()
                .unwrap_or("")
                .to_string();

            let description = op_value["description"]
                .as_str()
                .unwrap_or("")
                .to_string();

            let parameters = parse_parameters(&op_value["parameters"]);

            let request_body_schema = op_value
                .pointer("/requestBody/content/application~1json/schema")
                .cloned();

            let is_post = method == "post";

            let op = Operation {
                operation_id,
                method: method.to_uppercase(),
                path: path.clone(),
                summary,
                description,
                parameters,
                request_body_schema,
                is_post,
            };

            groups.entry(tag).or_default().push(op);
        }
    }

    groups
}

/// Parse the parameters array from an OpenAPI operation.
fn parse_parameters(params_value: &Value) -> Vec<Parameter> {
    let arr = match params_value.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };

    arr.iter()
        .map(|p| {
            let name = p["name"].as_str().unwrap_or("").to_string();
            let location = p["in"].as_str().unwrap_or("query").to_string();
            let required = p["required"].as_bool().unwrap_or(false);
            let param_type = extract_type(&p["schema"]);
            let description = p["description"].as_str().unwrap_or("").to_string();

            Parameter {
                name,
                location,
                required,
                param_type,
                description,
            }
        })
        .collect()
}

/// Extract a human-readable type string from a JSON Schema value.
fn extract_type(schema: &Value) -> String {
    // Handle anyOf (nullable types from schemars)
    if let Some(any_of) = schema.get("anyOf").and_then(|v| v.as_array()) {
        let types: Vec<String> = any_of
            .iter()
            .filter_map(|s| {
                if s.get("type").and_then(|t| t.as_str()) == Some("null") {
                    None
                } else {
                    Some(extract_type(s))
                }
            })
            .collect();
        if types.is_empty() {
            return "any".to_string();
        }
        let base = types.join(" | ");
        if any_of.iter().any(|s| s.get("type").and_then(|t| t.as_str()) == Some("null")) {
            return format!("{base} (nullable)");
        }
        return base;
    }

    // Handle $ref
    if let Some(ref_path) = schema.get("$ref").and_then(|v| v.as_str()) {
        return ref_path
            .rsplit('/')
            .next()
            .unwrap_or("object")
            .to_string();
    }

    // Handle type field
    match schema.get("type").and_then(|v| v.as_str()) {
        Some("array") => {
            let items_type = schema
                .get("items")
                .map(|items| extract_type(items))
                .unwrap_or_else(|| "any".to_string());
            format!("array<{items_type}>")
        }
        Some("object") => "object".to_string(),
        Some("integer") => {
            let format = schema.get("format").and_then(|v| v.as_str());
            match format {
                Some("int64") => "integer (int64)".to_string(),
                Some("int32") => "integer (int32)".to_string(),
                _ => "integer".to_string(),
            }
        }
        Some("number") => "number".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("string") => {
            let format = schema.get("format").and_then(|v| v.as_str());
            match format {
                Some("date-time") => "string (date-time)".to_string(),
                Some("uuid") => "string (uuid)".to_string(),
                Some(f) => format!("string ({f})"),
                None => "string".to_string(),
            }
        }
        Some(other) => other.to_string(),
        None => "any".to_string(),
    }
}

/// Sanitize a string for use inside a markdown table cell.
///
/// Pipes break the column structure and newlines break the row structure, so
/// we escape pipes and collapse newlines into spaces.
fn sanitize_table_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ").replace('\r', "")
}

/// Render a complete markdown page for one API tag group.
fn render_tag_page(tag: &str, operations: &[Operation]) -> String {
    let mut md = String::new();
    md.push_str(&format!("# {tag}\n\n"));

    for op in operations {
        // Heading: METHOD /path
        md.push_str(&format!("## `{} {}`\n\n", op.method, op.path));
        md.push_str(&format!("**Operation:** `{}`\n\n", op.operation_id));

        if !op.description.is_empty() {
            md.push_str(&op.description);
            md.push_str("\n\n");
        } else if !op.summary.is_empty() {
            md.push_str(&op.summary);
            md.push_str("\n\n");
        }

        // Parameters table
        if !op.parameters.is_empty() {
            md.push_str("### Parameters\n\n");
            md.push_str("| Name | In | Type | Required | Description |\n");
            md.push_str("|------|----|------|----------|-------------|\n");
            for p in &op.parameters {
                let req = if p.required { "Yes" } else { "No" };
                md.push_str(&format!(
                    "| `{}` | {} | {} | {} | {} |\n",
                    sanitize_table_cell(&p.name),
                    sanitize_table_cell(&p.location),
                    sanitize_table_cell(&p.param_type),
                    req,
                    sanitize_table_cell(&p.description),
                ));
            }
            md.push('\n');
        }

        // Request body
        if let Some(ref schema) = op.request_body_schema {
            md.push_str("### Request Body\n\n");
            render_schema_properties(&mut md, schema);
        }

        // Response format
        md.push_str("### Response\n\n");
        if op.is_post {
            md.push_str("Returns `201 Created` on success with the created resource as JSON.\n\n");
        } else {
            md.push_str("Returns `200 OK` on success with the result as JSON.\n\n");
        }

        md.push_str("---\n\n");
    }

    md
}

/// Render a table of properties from a JSON Schema object.
fn render_schema_properties(md: &mut String, schema: &Value) {
    let properties = match schema.get("properties").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => {
            md.push_str("JSON object.\n\n");
            return;
        }
    };

    let required_fields: Vec<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    md.push_str("| Field | Type | Required | Description |\n");
    md.push_str("|-------|------|----------|-------------|\n");

    for (name, prop_schema) in properties {
        let prop_type = sanitize_table_cell(&extract_type(prop_schema));
        let required = if required_fields.contains(&name.as_str()) {
            "Yes"
        } else {
            "No"
        };
        let description = prop_schema
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        md.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            sanitize_table_cell(name),
            prop_type,
            required,
            sanitize_table_cell(description),
        ));
    }

    md.push('\n');
}

/// Convert a tag name to a filename (e.g. "GitHub" -> "github.md").
fn tag_to_filename(tag: &str) -> String {
    format!("{}.md", tag.to_lowercase())
}

/// Update SUMMARY.md by replacing content between the API_REFERENCE markers.
fn update_summary(summary_path: &Path, entries: &[(String, String)]) {
    let content = match std::fs::read_to_string(summary_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "warning: could not read {}: {e}. Skipping SUMMARY.md update.",
                summary_path.display()
            );
            return;
        }
    };

    let start_marker = "<!-- API_REFERENCE_START -->";
    let end_marker = "<!-- API_REFERENCE_END -->";

    let Some(start_idx) = content.find(start_marker) else {
        eprintln!(
            "warning: {start_marker} not found in {}. Skipping SUMMARY.md update.",
            summary_path.display()
        );
        return;
    };

    let Some(end_idx) = content.find(end_marker) else {
        eprintln!(
            "warning: {end_marker} not found in {}. Skipping SUMMARY.md update.",
            summary_path.display()
        );
        return;
    };

    let before = &content[..start_idx + start_marker.len()];
    let after = &content[end_idx..];

    let mut generated = String::new();
    generated.push('\n');
    for (tag, filename) in entries {
        generated.push_str(&format!("- [{tag}](api-reference/{filename})\n"));
    }

    let new_content = format!("{before}{generated}{after}");
    std::fs::write(summary_path, &new_content)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", summary_path.display()));
}
