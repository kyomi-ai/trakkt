// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compile the issue-list composable filter clauses to SQL.
//!
//! [`trakkt_types::api::FilterClause::matches`] is the one definition of what
//! a clause means. This module compiles the same clauses to `WHERE` conditions
//! over the `issues` table so a query can apply them before ranking, sorting,
//! capping and paging, instead of filtering a page after the fact. The two
//! must agree: `apps/server/tests/postgres_dialect.rs` runs every
//! field/operator pair through both on SQLite and Postgres.
//!
//! The conditions reference the issues table as alias `i` and need no other
//! join, so they can be appended to any query that selects `FROM issues i`.
//! Placeholders are numbered `$N` starting at the index the caller passes, and
//! the matching values come back in [`ClauseSql::args`] in placeholder order.
//! Everything that is not a bound value (relation subqueries, counts) is a
//! constant, so no caller-supplied text is ever interpolated into SQL.

use trakkt_types::api::{FilterClause, BOOLEAN_FILTER_FIELDS};

/// A value to bind, in placeholder order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlArg {
    Text(String),
    Int(i32),
}

/// The SQL for a set of clauses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClauseSql {
    /// Conditions to AND into the `WHERE` clause (one per effective clause).
    pub conditions: Vec<String>,
    /// Values for the placeholders used by `conditions`, in order.
    pub args: Vec<SqlArg>,
    /// The first placeholder index not used by these clauses. Invariant:
    /// `next_param == start_param + args.len()`, because every placeholder has
    /// exactly one arg and the boolean conditions consume neither. Callers that
    /// append their own args can rely on it to continue numbering.
    pub next_param: usize,
}

/// `EXISTS (…)` over the relation table for a fixed relation predicate.
fn relation_exists(predicate: &str) -> String {
    format!("EXISTS (SELECT 1 FROM issue_relations r2 WHERE {predicate})")
}

const IS_PARENT: &str = "r2.source_issue_id = i.issue_id AND r2.relation_type = 'parent'";
const IS_SUB_ISSUE: &str = "r2.target_issue_id = i.issue_id AND r2.relation_type = 'parent'";
const IS_BLOCKING: &str = "r2.source_issue_id = i.issue_id AND r2.relation_type = 'blocks'";
const HAS_RELATIONS: &str = "r2.source_issue_id = i.issue_id OR r2.target_issue_id = i.issue_id";
const IS_BLOCKED: &str = "EXISTS (SELECT 1 FROM issue_relations r2 \
     JOIN issues blocker ON blocker.issue_id = r2.source_issue_id \
     JOIN statuses s2 ON s2.status_id = blocker.status_id \
     WHERE r2.target_issue_id = i.issue_id AND r2.relation_type = 'blocks' \
     AND s2.category NOT IN ('completed', 'cancelled'))";

fn boolean_condition(field: &str, want_true: bool) -> Option<String> {
    let exists = match field {
        "is_sub_issue" => relation_exists(IS_SUB_ISSUE),
        "is_parent" => relation_exists(IS_PARENT),
        "is_blocked" => IS_BLOCKED.to_string(),
        "is_blocking" => relation_exists(IS_BLOCKING),
        "has_relations" => relation_exists(HAS_RELATIONS),
        _ => return None,
    };
    Some(if want_true { exists } else { format!("NOT {exists}") })
}

/// Distinct values, first occurrence order (so placeholders are stable).
fn distinct(values: &[String]) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    values
        .iter()
        .map(String::as_str)
        .filter(|v| seen.insert(*v))
        .collect()
}

/// A UUID set bound as one text parameter instead of one parameter per ID, so
/// a large starred set cannot exceed the backend's parameter limit.
fn issue_set_arg(values: &[&str], is_pg: bool) -> String {
    if is_pg {
        values.join(",")
    } else {
        serde_json::to_string(values).expect("a list of strings always serialises")
    }
}

fn issue_set_membership(param: &str, is_pg: bool, negated: bool) -> String {
    let membership = if is_pg {
        format!("i.issue_id = ANY(string_to_array({param}, ','))")
    } else {
        format!("i.issue_id IN (SELECT value FROM json_each({param}))")
    };
    if negated {
        format!("NOT ({membership})")
    } else {
        membership
    }
}

/// Compile `clauses` to SQL, numbering placeholders from `start_param`.
///
/// Clauses that [`FilterClause::matches`] treats as pass-everything (no
/// values on a non-boolean field, unknown field or operator) produce no
/// condition.
pub fn clauses_to_sql(
    clauses: &[FilterClause],
    start_param: usize,
    is_pg: bool,
) -> ClauseSql {
    let mut out = ClauseSql {
        next_param: start_param,
        ..ClauseSql::default()
    };

    for clause in clauses {
        let field = clause.field.as_str();
        let op = clause.operator.as_str();

        if BOOLEAN_FILTER_FIELDS.contains(&field) {
            let want_true = match op {
                "any_of" => true,
                "none_of" => false,
                _ => continue,
            };
            out.conditions
                .extend(boolean_condition(field, want_true));
            continue;
        }
        if clause.values.is_empty() {
            // An empty issue set is a restriction only when it is the set to
            // include ("no starred issues"); excluding nothing constrains
            // nothing.
            if field == "issue_id" && op == "any_of" {
                out.conditions.push("1 = 0".to_string());
            }
            continue;
        }

        let values = distinct(&clause.values);

        // Placeholder list for text values.
        let text_in = |out: &mut ClauseSql| -> String {
            let placeholders: Vec<String> = values
                .iter()
                .enumerate()
                .map(|(k, _)| format!("${}", out.next_param + k))
                .collect();
            out.next_param += values.len();
            out.args
                .extend(values.iter().map(|v| SqlArg::Text((*v).to_string())));
            placeholders.join(", ")
        };

        let condition = match (field, op) {
            ("status", "any_of") => format!("i.status_id IN ({})", text_in(&mut out)),
            ("status", "none_of") => format!("i.status_id NOT IN ({})", text_in(&mut out)),
            // The Starred view's issue set: one bound parameter however many
            // IDs it holds.
            ("issue_id", "any_of" | "none_of") => {
                let param = format!("${}", out.next_param);
                out.next_param += 1;
                out.args.push(SqlArg::Text(issue_set_arg(&values, is_pg)));
                issue_set_membership(&param, is_pg, op == "none_of")
            }
            ("project", "any_of") => format!("i.project_id IN ({})", text_in(&mut out)),
            // NULL project_id is "not in" the set, as in `FilterClause::matches`.
            ("project", "none_of") => format!(
                "(i.project_id IS NULL OR i.project_id NOT IN ({}))",
                text_in(&mut out)
            ),
            ("priority", "any_of" | "none_of") => {
                // A value is a match only if it is the canonical text of the
                // stored integer (`matches` compares `priority.to_string()`).
                let ints: Vec<i32> = values
                    .iter()
                    .filter_map(|v| v.parse::<i32>().ok().filter(|n| n.to_string() == *v))
                    .collect();
                if ints.is_empty() {
                    if op == "any_of" {
                        out.conditions.push("1 = 0".to_string());
                    }
                    continue;
                }
                let placeholders: Vec<String> = (0..ints.len())
                    .map(|k| format!("${}", out.next_param + k))
                    .collect();
                out.next_param += ints.len();
                out.args.extend(ints.into_iter().map(SqlArg::Int));
                let not = if op == "none_of" { "NOT " } else { "" };
                format!("i.priority {not}IN ({})", placeholders.join(", "))
            }
            ("label", "any_of" | "not_any_of" | "none_of") => {
                let not = if op == "any_of" { "" } else { "NOT " };
                format!(
                    "{not}EXISTS (SELECT 1 FROM issue_labels il WHERE il.issue_id = i.issue_id \
                     AND il.label_id IN ({}))",
                    text_in(&mut out)
                )
            }
            ("label", "all_of" | "not_all_of") => {
                let wanted = values.len();
                let cmp = if op == "all_of" { "=" } else { "<" };
                format!(
                    "(SELECT COUNT(DISTINCT il.label_id) FROM issue_labels il \
                     WHERE il.issue_id = i.issue_id AND il.label_id IN ({})) {cmp} {wanted}",
                    text_in(&mut out)
                )
            }
            _ => continue,
        };
        out.conditions.push(condition);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(field: &str, op: &str, values: &[&str]) -> FilterClause {
        FilterClause {
            field: field.to_string(),
            operator: op.to_string(),
            values: values.iter().map(|v| v.to_string()).collect(),
        }
    }

    #[test]
    fn placeholders_continue_from_the_start_index_across_clauses() {
        let sql = clauses_to_sql(
            &[
                clause("status", "any_of", &["a", "b"]),
                clause("priority", "none_of", &["1", "2", "x"]),
                clause("label", "all_of", &["l1", "l1", "l2"]),
            ],
            5,
            true,
        );
        assert_eq!(sql.conditions[0], "i.status_id IN ($5, $6)");
        assert_eq!(sql.conditions[1], "i.priority NOT IN ($7, $8)");
        assert!(sql.conditions[2].contains("IN ($9, $10)) = 2"));
        assert_eq!(sql.next_param, 11);
        assert_eq!(
            sql.args,
            vec![
                SqlArg::Text("a".into()),
                SqlArg::Text("b".into()),
                SqlArg::Int(1),
                SqlArg::Int(2),
                SqlArg::Text("l1".into()),
                SqlArg::Text("l2".into()),
            ]
        );
    }

    #[test]
    fn pass_through_clauses_add_nothing() {
        let sql = clauses_to_sql(
            &[
                clause("status", "any_of", &[]),
                clause("bogus", "any_of", &["x"]),
                clause("status", "wat", &["x"]),
                clause("is_parent", "wat", &[]),
            ],
            3,
            true,
        );
        assert!(sql.conditions.is_empty());
        assert!(sql.args.is_empty());
        assert_eq!(sql.next_param, 3);
    }

    #[test]
    fn unmatchable_priority_values_never_match_and_never_exclude() {
        assert_eq!(
            clauses_to_sql(&[clause("priority", "any_of", &["x", "03"])], 3, false).conditions,
            vec!["1 = 0"]
        );
        assert!(clauses_to_sql(&[clause("priority", "none_of", &["x"])], 3, false)
            .conditions
            .is_empty());
    }

    #[test]
    fn issue_sets_bind_one_parameter_per_backend() {
        let pg = clauses_to_sql(&[clause("issue_id", "any_of", &["a", "b"])], 4, true);
        assert_eq!(pg.conditions, vec!["i.issue_id = ANY(string_to_array($4, ','))"]);
        assert_eq!(pg.args, vec![SqlArg::Text("a,b".into())]);
        let sq = clauses_to_sql(&[clause("issue_id", "none_of", &["a", "b"])], 4, false);
        assert_eq!(
            sq.conditions,
            vec!["NOT (i.issue_id IN (SELECT value FROM json_each($4)))"]
        );
        assert_eq!(sq.args, vec![SqlArg::Text(r#"["a","b"]"#.into())]);
        assert_eq!(sq.next_param, 5);
    }

    #[test]
    fn empty_issue_sets_match_none_or_all() {
        let none = clauses_to_sql(&[clause("issue_id", "any_of", &[])], 4, true);
        assert_eq!(none.conditions, vec!["1 = 0"]);
        assert!(none.args.is_empty());
        let all = clauses_to_sql(&[clause("issue_id", "none_of", &[])], 4, true);
        assert!(all.conditions.is_empty());
    }

    #[test]
    fn boolean_clauses_ignore_values_and_need_no_binds() {
        let sql = clauses_to_sql(&[clause("is_blocked", "none_of", &["ignored"])], 3, false);
        assert!(sql.conditions[0].starts_with("NOT EXISTS"));
        assert!(sql.args.is_empty());
    }
}
