# MCP mutation receipts

MCP mutation responses confirm the applied write and retain the identity needed for
the next operation. The MCP boundary projects the shared handler response before
serializing compact JSON into the tool result's text content. REST handlers retain
their existing response payloads. Authentication, authorization, persistence,
activity recording and sync delivery continue through the existing handlers and
services.

## Response policies

The 21 mutation tools have the following policies: nine entity projections, three
small metadata responses, and nine existing acknowledgements. Optional fields are
included only when returned by the handler; null fields in projected receipts are
omitted except when reporting an explicitly requested clear.

| Tool | Retained fields / outcome |
| --- | --- |
| `create_issue` | `success`, `issue_id`, canonical `key`, `team_id`, status ID/name/category, priority, non-null lifecycle timestamps, `relations_created`, `relation_warnings` |
| `update_issue` | Same issue identity/status/timestamps policy; the returned key reflects a team move |
| `add_comment` | `success`, `comment_id`, `issue_id`, optional `parent_id`, creation/update timestamps |
| `create_project` | `success`, `project_id`, status, creation/update timestamps |
| `update_project` | Same project policy |
| `create_milestone` | `success`, `milestone_id`, `project_id`, target date, sort order, creation timestamp |
| `update_milestone` | Same milestone policy |
| `create_release` | `success`, `release_id`, `team_key`, tag, previous tag, creation timestamp, `issue_count` |
| `update_team_settings` | `success`, `team_id`, team `key`, effective `settings` |
| `create_label` | Existing small label metadata: ID, name, color, workspace/team IDs, creation timestamp |
| `add_relation` | Existing relation metadata plus requested duplicate `auto_close` outcome |
| `upload_attachment` | Existing attachment ID, filename, content type, size, download URL; requested `issue_link` outcome |
| `delete_issue` | Existing deletion message |
| `delete_project` | Existing deletion message |
| `delete_milestone` | Existing deletion message |
| `delete_attachment` | Existing deletion message |
| `remove_relation` | Existing removal message |
| `attach_to_issue` | Existing link acknowledgement |
| `detach_from_issue` | Existing unlink acknowledgement |
| `star_issue` | Existing star acknowledgement |
| `unstar_issue` | Existing unstar acknowledgement |

Issue receipts also retain effective assignee, due date, project, milestone and
estimate when those inputs were supplied. Project receipts retain effective lead,
start date, target date and archive timestamp when supplied. Milestone updates
retain an explicitly supplied target date. These fields include null when a clear
was explicitly requested. Receipts omit descriptions, comment bodies, release
notes, issue labels and unrelated entity metadata.

## Partial outcomes and errors

A receipt describes the primary write. It does not assert successful GitHub
notification, object storage cleanup or any other untracked secondary action.

- Issue creation retains inline relation warnings and created relation metadata.
  An MCP parent-link failure after issue creation also returns the created issue
  with a warning, so callers can repair the relation without duplicating the issue.
  Each inline relation warning is capped at 512 Unicode characters; the number of
  warnings is preserved.
- Attachment upload reports `issue_link.status` as `linked` or `failed` when a
  nonempty issue ID was supplied. A failed link retains the saved attachment ID
  and instructs the caller to retry linking rather than uploading again.
- Duplicate relation creation reports `auto_close.status` as `closed`,
  `already_cancelled` or `failed`. A failed auto-close preserves the relation and
  instructs the caller to update the source issue separately.
- Primary handler errors remain errors; projection does not turn them into a
  success receipt. The existing JSON-RPC error transport is unchanged.

## Separate audit follow-ups

Read response selection is tracked in TRA-10074, tool error transport in
TRA-10089, and attachment file bytes in default MCP text in TRA-10108. Those
changes are independent of mutation receipts.

## Measured response sizes

The route test `nine_priority_tools_exclude_prose_and_have_size_independent_receipts`
uses fresh SQLite fixtures with `PROSE_SENTINEL markdown text\n` repeated once and
4,000 times in descriptions, comment bodies and release notes. It checks that
success receipts contain none of that prose and have equal serialized sizes after
normalizing generated IDs and timestamp precision. Existing team description prose
is varied too, and the issue update changes only status.

Observed UTF-8 text-content sizes on 2026-10-29 (actual receipts, without the MCP
envelope):

| Tool | Compact receipt bytes |
| --- | ---: |
| `create_issue` | 229 |
| `update_issue` | 324 |
| `add_comment` | 190 |
| `create_project` | 157 |
| `update_project` | 157 |
| `create_milestone` | 172 |
| `update_milestone` | 172 |
| `create_release` | 152 |
| `update_team_settings` | 177 |

For the representative status-only update with the long existing description,
pretty-printing the full shared issue entity as before produces **120,946 bytes**;
the compact MCP receipt produces **324 bytes**. This is a synthetic serialized-byte
comparison, not a token benchmark. Counts, IDs and timestamps can affect individual
receipt lengths; submitted prose length does not.

The other route scenarios cover canonical keys after a team move, effective clears,
primary errors, parent/inline relation warnings, duplicate auto-close outcomes,
upload-link outcomes and unchanged shared REST payloads. A release test preserves
a nonzero issue count while deduplicating repeated commit references. Boundary
unit tests guard all 21 registered mutation policies and bounded warning output.
