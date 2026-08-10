//! `df-mcp` — an MCP server exposing the DataForge tool surface over stdio.
//!
//! This is the transport boundary ADR-0043 §2 puts the bounded vocabulary on.
//! Any agent runtime, with any model provider, can drive the engine through it
//! without linking Rust — and, more to the point, without holding a shell.
//! What the server exposes is exactly [`df_tools::TOOLS`] and nothing else:
//! there is no arbitrary-filesystem tool, no raw SQL, no command execution. A
//! model that decides to do something outside the engine's vocabulary finds no
//! way to express it, which is a stronger property than a model that is asked
//! not to.
//!
//! # The actor is not a parameter
//!
//! Every call is attributed to [`Actor::Agent`], hard-coded. A caller cannot
//! present itself as `cli`, because a ledger that cannot tell "a person decided
//! this" from "a model decided this" is worthless as an audit trail, and in an
//! evidential archive that distinction is the whole point. Attribution records
//! *who*; it grants nothing.
//!
//! # Transport
//!
//! JSON-RPC 2.0 over stdio, one JSON object per line. No network, no listening
//! socket, no session state — the server holds nothing between calls because
//! the whole state lives in SQLite (ADR-0043 §5).
//!
//! The protocol is implemented directly rather than pulled from a crate. That
//! is the resolution of the dependency question ADR-0043 left open: an MCP SDK
//! would add a dependency tree to a project that pins versions and runs
//! `cargo deny` in CI, to save a few hundred lines of line-delimited JSON-RPC.
//!
//! # Stdout is the protocol
//!
//! Nothing but protocol messages is ever written to stdout. Diagnostics go to
//! stderr; a stray `println!` would corrupt the stream.

use std::io::{BufRead, Write};

use df_tools::{Actor, Capability, Tool};
use serde_json::{json, Value};

/// MCP revision this server implements.
const PROTOCOL_VERSION: &str = "2025-06-18";

const SERVER_NAME: &str = "dataforge";

// JSON-RPC 2.0 error codes.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Whether this process will run `Capability::Commit` tools.
///
/// Closed unless `--allow-commit` is passed. `Capability::requires_authorization`
/// marks the three commit tools, but nothing consults it yet: `df-rules` is not
/// wired, so a commit tool reaching `df_tools::invoke` runs with neither a human
/// nor a rule in front of it. That is L2 without the rules engine — the one rung
/// RFC-0002 says must not exist — and it would be the state of `main` from the
/// moment this surface lands until the gate does, several blocks later.
///
/// So the default is a server an agent can look and think with but not commit
/// with, which is exactly L1: the human still holds the gate. The flag exists
/// for developing the gate itself; it is not a supported way to run. When
/// `df-rules` becomes the authority this is replaced by the real authorization
/// call and the flag goes away.
fn commit_allowed() -> bool {
    let allowed = std::env::args().any(|arg| arg == "--allow-commit");
    if allowed {
        eprintln!(
            "df-mcp: WARNING --allow-commit: approve_plan, execute_plan and \
             verify_project_output are reachable with no gate in front of them. \
             For developing the gate, not for driving real archives."
        );
    }
    allowed
}

fn main() {
    let commit_allowed = commit_allowed();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        match stdin.lock().read_line(&mut line) {
            // Clean EOF: the client closed the pipe.
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                eprintln!("df-mcp: cannot read stdin: {error}");
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // A notification (no `id`) gets no reply — that is the JSON-RPC
        // contract, and answering one would desynchronise a strict client.
        if let Some(response) = handle_line(trimmed, commit_allowed) {
            if let Err(error) = writeln!(stdout, "{response}") {
                eprintln!("df-mcp: cannot write stdout: {error}");
                break;
            }
            if let Err(error) = stdout.flush() {
                eprintln!("df-mcp: cannot flush stdout: {error}");
                break;
            }
        }
    }
}

/// Parse one line and produce its response, if it deserves one.
fn handle_line(line: &str, commit_allowed: bool) -> Option<Value> {
    let request: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        // No id is recoverable from unparseable input, so the error carries a
        // null one, as JSON-RPC requires.
        Err(error) => {
            return Some(error_response(
                Value::Null,
                PARSE_ERROR,
                &format!("invalid JSON: {error}"),
            ))
        }
    };

    let id = request.get("id").cloned();
    let method = request.get("method").and_then(Value::as_str);

    let Some(method) = method else {
        return Some(error_response(
            id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "missing `method`",
        ));
    };

    // Notifications carry no id and expect no answer.
    let id = id?;

    let params = request.get("params").cloned().unwrap_or(Value::Null);
    Some(dispatch(id, method, params, commit_allowed))
}

fn dispatch(id: Value, method: &str, params: Value, commit_allowed: bool) -> Value {
    match method {
        "initialize" => success(id, initialize_result()),
        "ping" => success(id, json!({})),
        "tools/list" => success(id, json!({ "tools": tool_descriptors(commit_allowed) })),
        "tools/call" => call_tool(id, params, commit_allowed),
        other => error_response(id, METHOD_NOT_FOUND, &format!("unknown method `{other}`")),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
        // Surfaced so an operator can see, from the handshake alone, which
        // frozen surface this server speaks.
        "instructions": format!(
            "DataForge engine. Tool surface {}. Tools are classified OBSERVE (read), \
             BUILD (advance analysis and plan, copies nothing) and COMMIT (changes real \
             state: freezes the manifest, copies bytes, verifies output). The origin is \
             never written to. Every call is recorded in the ledger as an agent action.",
            df_tools::TOOL_SURFACE_VERSION
        ),
    })
}

fn tool_descriptors(commit_allowed: bool) -> Vec<Value> {
    df_tools::TOOLS
        .iter()
        .filter(|tool| commit_allowed || tool.capability != Capability::Commit)
        .map(descriptor)
        .collect()
}

fn descriptor(tool: &Tool) -> Value {
    json!({
        "name": tool.name,
        "description": format!("[{}] {}", tool.capability.as_str(), tool.summary),
        "inputSchema": input_schema(tool.name),
        // A read-only hint lets a client present observe tools without a
        // confirmation prompt. It is advisory: the real boundary is that this
        // server exposes nothing else.
        "annotations": {
            "readOnlyHint": tool.capability == Capability::Observe,
            "destructiveHint": false,
        },
    })
}

fn call_tool(id: Value, params: Value, commit_allowed: bool) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, INVALID_PARAMS, "missing tool `name`");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // Leaving a commit tool out of `tools/list` hides it; this refuses it.
    // Nothing stops a client calling a name it was never offered, and a
    // boundary that depends on the caller only using what it was shown is not
    // a boundary — the same reason the vocabulary lives in the transport
    // rather than in an instruction to the model.
    if !commit_allowed
        && df_tools::tool(name).is_some_and(|tool| tool.capability == Capability::Commit)
    {
        return success(
            id,
            json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "`{name}` changes real state and this server runs with \
                         the commit class closed: the deterministic gate \
                         (df-rules) is not wired yet, so there is nothing to \
                         authorise it. Propose the plan and let a human approve \
                         it — `plan tree` shows what it would write. See RFC-0002."
                    ),
                }],
                "isError": true,
            }),
        );
    }

    // The actor is fixed, never taken from the caller: see the module docs.
    match df_tools::invoke(name, arguments, Actor::Agent) {
        Ok(output) => {
            let text = serde_json::to_string_pretty(&output)
                .unwrap_or_else(|error| format!("could not render output: {error}"));
            success(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false,
                }),
            )
        }
        // A tool that refuses is a normal result, not a protocol failure: the
        // agent has to see the refusal and react to it, and a JSON-RPC error
        // would look to most clients like the server broke.
        Err(error) => success(
            id,
            json!({
                "content": [{ "type": "text", "text": error.to_string() }],
                "isError": true,
            }),
        ),
    }
}

/// The JSON Schema of a tool's input.
///
/// Written out rather than derived, because these schemas are part of the
/// frozen contract and a derive would let them drift with an unrelated
/// refactor of the Rust types.
fn input_schema(name: &str) -> Value {
    let project_dir = json!({
        "type": "string",
        "description": "Absolute path to the DataForge project directory.",
    });

    // Reports list detail in windows. Declaring the window here is what makes
    // it usable: a caller that cannot see `limit` in the schema cannot page,
    // and would read the first window as the whole answer.
    if let Some(collections) = df_tools::report_collections(name) {
        let collections = collections.join(", ");
        return json!({
            "type": "object",
            "properties": {
                "project_dir": project_dir,
                "limit": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": df_tools::MAX_REPORT_ITEMS,
                    "default": df_tools::DEFAULT_REPORT_ITEMS,
                    "description": format!(
                        "Items listed per collection ({collections}). Totals are never \
                         windowed. 0 returns totals only; a larger value is clamped, and \
                         `pages.<collection>.has_more` always says whether more remain.",
                    ),
                },
                "offset": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Where the window starts. Past the end is empty, not an error.",
                },
            },
            "required": ["project_dir"],
            "additionalProperties": false,
        });
    }

    match name {
        "plan_destination_tree" => json!({
            "type": "object",
            "properties": {
                "project_dir": project_dir,
                "depth": {
                    "type": "integer",
                    "minimum": 1,
                    "default": 2,
                    "description": "Levels shown under the output root. A view, never a filter.",
                },
            },
            "required": ["project_dir"],
            "additionalProperties": false,
        }),
        "create_plan" => json!({
            "type": "object",
            "properties": {
                "project_dir": project_dir,
                "policy": {
                    "type": "string",
                    "enum": [
                        "REPORT_ONLY",
                        "CONSOLIDATE_WITHIN_CONTEXT",
                        "CONSOLIDATE_GENERIC_COPIES",
                        "CONSOLIDATE_ALL",
                    ],
                    "default": "REPORT_ONLY",
                    "description":
                        "What to do with exact duplicates. Consolidation is always opt-in.",
                },
            },
            "required": ["project_dir"],
            "additionalProperties": false,
        }),
        "content_search" => json!({
            "type": "object",
            "properties": {
                "project_dir": project_dir,
                "query": {
                    "type": "string",
                    "description": "Tantivy query text.",
                },
                "run_id": {
                    "type": "string",
                    "description":
                        "Completed extraction run. Defaults to the latest of the latest snapshot.",
                },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
                "offset": { "type": "integer", "minimum": 0, "default": 0 },
                "snippet_chars": { "type": "integer", "minimum": 1, "default": 240 },
            },
            "required": ["project_dir", "query"],
            "additionalProperties": false,
        }),
        "content_query" => json!({
            "type": "object",
            "properties": {
                "project_dir": project_dir,
                "sql": {
                    "type": "string",
                    "description":
                        "Read-only SQL over the derived Parquet evidence, run in an isolated \
                         worker. The sole registered table is `content`. This never reaches the \
                         engine's SQLite.",
                },
                "run_id": { "type": "string" },
                "max_rows": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Lower the engine default only; the worker enforces its own ceiling.",
                },
                "timeout_seconds": { "type": "integer", "minimum": 1 },
            },
            "required": ["project_dir", "sql"],
            "additionalProperties": false,
        }),
        "decide_structural_review" => json!({
            "type": "object",
            "properties": {
                "project_dir": project_dir,
                "item_id": { "type": "string" },
                "decision": { "type": "string", "enum": rule_actions() },
                "rationale": {
                    "type": "string",
                    "description": "Why. Recorded in the ledger with the decision.",
                },
            },
            "required": ["project_dir", "item_id", "decision", "rationale"],
            "additionalProperties": false,
        }),
        "decide_structural_review_batch" => json!({
            "type": "object",
            "properties": {
                "project_dir": project_dir,
                "decisions": {
                    "type": "array",
                    "minItems": 1,
                    "description":
                        "Applied atomically. Each keeps its own rationale and its own ledger \
                         event: a batch is transport, never a weaker record.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "item_id": { "type": "string" },
                            "decision": { "type": "string", "enum": rule_actions() },
                            "rationale": { "type": "string" },
                        },
                        "required": ["item_id", "decision", "rationale"],
                        "additionalProperties": false,
                    },
                },
            },
            "required": ["project_dir", "decisions"],
            "additionalProperties": false,
        }),
        _ => json!({
            "type": "object",
            "properties": { "project_dir": project_dir },
            "required": ["project_dir"],
            "additionalProperties": false,
        }),
    }
}

fn rule_actions() -> Value {
    json!([
        "COPY_ACTIVE",
        "COPY_REVIEW",
        "COPY_SEPARATED",
        "COPY_TEMPORARY"
    ])
}

fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, params: Value) -> String {
        json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string()
    }

    #[test]
    fn initialize_announces_the_frozen_surface() {
        let response = handle_line(&request("initialize", json!({})), true).expect("a reply");
        let result = &response["result"];
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["instructions"]
            .as_str()
            .expect("instructions")
            .contains(df_tools::TOOL_SURFACE_VERSION));
    }

    #[test]
    fn tools_list_exposes_every_tool_and_nothing_else() {
        let response = handle_line(&request("tools/list", json!({})), true).expect("a reply");
        let listed = response["result"]["tools"]
            .as_array()
            .expect("tools array")
            .len();
        assert_eq!(
            listed,
            df_tools::TOOLS.len(),
            "the server must expose the registry exactly"
        );
    }

    /// Until `df-rules` is the authority, a commit tool reaching the engine
    /// would freeze a manifest or copy bytes with neither a human nor a rule
    /// behind it. The server closes that class by default so the shipped state
    /// is L1 — the human still holds the gate — and not L2 without the rules
    /// engine, which is the one rung RFC-0002 rules out.
    #[test]
    fn the_commit_class_is_closed_unless_asked_for() {
        let response = handle_line(&request("tools/list", json!({})), false).expect("a reply");
        let listed: Vec<&str> = response["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();

        for closed in ["approve_plan", "execute_plan", "verify_project_output"] {
            assert!(!listed.contains(&closed), "{closed} must not be offered");
        }
        // Everything an agent needs to look and think with is still there.
        assert!(listed.contains(&"project_status"));
        assert!(listed.contains(&"create_plan"));
        assert_eq!(
            listed.len(),
            df_tools::TOOLS
                .iter()
                .filter(|tool| tool.capability != Capability::Commit)
                .count()
        );
    }

    /// Hiding a tool is not refusing it. Nothing stops a client calling a name
    /// it was never offered, so a boundary that relies on the caller only using
    /// what it saw is not a boundary at all — the same reason the vocabulary
    /// lives in the transport instead of in an instruction to the model.
    #[test]
    fn a_closed_commit_tool_is_refused_even_when_called_directly() {
        for closed in ["approve_plan", "execute_plan", "verify_project_output"] {
            let response = handle_line(
                &request(
                    "tools/call",
                    json!({ "name": closed, "arguments": { "project_dir": r"D:\p" } }),
                ),
                false,
            )
            .expect("a reply");

            assert_eq!(response["result"]["isError"], true, "{closed}");
            let text = response["result"]["content"][0]["text"]
                .as_str()
                .expect("text");
            assert!(
                text.contains("commit class closed"),
                "{closed}: unexpected text: {text}"
            );
        }
    }

    #[test]
    fn every_listed_tool_carries_a_closed_schema() {
        // `additionalProperties: false` is what stops a caller smuggling a
        // parameter the engine never agreed to read.
        let response = handle_line(&request("tools/list", json!({})), true).expect("a reply");
        for tool in response["result"]["tools"].as_array().expect("tools") {
            let schema = &tool["inputSchema"];
            assert_eq!(
                schema["additionalProperties"], false,
                "tool `{}` has an open input schema",
                tool["name"]
            );
            assert!(
                schema["properties"]["project_dir"].is_object(),
                "tool `{}` does not take a project",
                tool["name"]
            );
        }
    }

    #[test]
    fn every_windowed_report_advertises_its_window() {
        // A report that pages without saying so hands back a first window that
        // reads as the whole answer. The schema is the only place a caller
        // learns otherwise, so it is the schema this asserts — and it reads the
        // same list the dispatch does, which is what keeps the two from
        // drifting apart.
        let response = handle_line(&request("tools/list", json!({})), true).expect("a reply");
        let mut checked = 0;
        for listed in response["result"]["tools"].as_array().expect("tools") {
            let name = listed["name"].as_str().expect("name");
            if df_tools::report_collections(name).is_none() {
                continue;
            }
            let schema = &listed["inputSchema"];
            assert_eq!(
                schema["properties"]["limit"]["maximum"],
                df_tools::MAX_REPORT_ITEMS,
                "report `{name}` advertises a ceiling the dispatch does not enforce"
            );
            assert_eq!(
                schema["properties"]["limit"]["default"],
                df_tools::DEFAULT_REPORT_ITEMS,
                "report `{name}` advertises the wrong default window"
            );
            assert!(
                schema["properties"]["offset"].is_object(),
                "`{name}` cannot page"
            );
            checked += 1;
        }
        assert_eq!(
            checked, 7,
            "the windowed reports are seven; a new one needs a schema"
        );
    }

    #[test]
    fn observe_tools_are_marked_read_only_and_none_is_destructive() {
        let response = handle_line(&request("tools/list", json!({})), true).expect("a reply");
        for listed in response["result"]["tools"].as_array().expect("tools") {
            let name = listed["name"].as_str().expect("name");
            let tool = df_tools::tool(name).expect("registered");
            assert_eq!(
                listed["annotations"]["readOnlyHint"],
                tool.capability == Capability::Observe,
                "tool `{name}` is mislabelled"
            );
            assert_eq!(listed["annotations"]["destructiveHint"], false);
        }
    }

    #[test]
    fn a_notification_gets_no_reply() {
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle_line(&notification.to_string(), true).is_none());
    }

    #[test]
    fn unparseable_input_is_answered_not_swallowed() {
        let response = handle_line("{not json", true).expect("a reply");
        assert_eq!(response["error"]["code"], PARSE_ERROR);
        assert_eq!(response["id"], Value::Null);
    }

    #[test]
    fn an_unknown_method_is_rejected() {
        let response = handle_line(&request("tools/exec", json!({})), true).expect("a reply");
        assert_eq!(response["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn calling_an_unregistered_tool_reports_a_tool_error() {
        // Not a JSON-RPC error: the agent has to see the refusal as a result
        // and react to it, rather than the client deciding the server broke.
        let response = handle_line(
            &request(
                "tools/call",
                json!({ "name": "run_shell", "arguments": {} }),
            ),
            true,
        )
        .expect("a reply");
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("unknown tool"), "unexpected text: {text}");
    }

    #[test]
    fn a_call_without_a_name_is_an_invalid_params_error() {
        let response = handle_line(&request("tools/call", json!({})), true).expect("a reply");
        assert_eq!(response["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn the_server_offers_no_way_to_reach_past_the_engine() {
        // The property that makes this a boundary rather than a convenience:
        // the method table is closed, so there is no transport-level escape
        // even for a client that asks for one.
        for method in ["tools/exec", "resources/read", "shell", "sql", "fs/read"] {
            let response = handle_line(&request(method, json!({})), true).expect("a reply");
            assert_eq!(
                response["error"]["code"], METHOD_NOT_FOUND,
                "method `{method}` must not resolve"
            );
        }
    }
}
