use codex_mobile_bridge::{interactions, live::LiveState, state, Result};
use serde_json::{json, Value};

#[test]
fn preserves_existing_mobile_protocol() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/contracts.json")).unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let args = &case["args"];
        let result: Result<Value> = match case["name"].as_str().unwrap() {
            "pendingInteractions" => {
                let mut live = LiveState::default();
                live.turns = args[1].as_array().unwrap().clone();
                for request in args[0].as_array().unwrap() {
                    live.requests
                        .insert(interactions::request_id(&request["id"]), request.clone());
                }
                Ok(json!(live.pending()))
            }
            "appServerReply" => interactions::app_server_reply(&args[0], &args[1]),
            "asyncReply" => interactions::async_reply(
                args[0].as_array().unwrap(),
                args[1].as_str().unwrap(),
                &args[2],
            )
            .map(Value::String),
            "summarizeItem" => Ok(state::summarize_item(&args[0])),
            "textAppend" => Ok(state::text_append(&args[0], &args[1], 512).unwrap_or(Value::Null)),
            "permissionSettings" => state::permissions(args[0].as_str().unwrap()),
            "readPermissionSettings" => Ok(state::read_permissions(&args[0])),
            name => panic!("Unknown fixture: {name}"),
        };
        let matched = match (&result, case.get("error")) {
            (Err(error), Some(expected)) => {
                expected["status"].is_null() || expected["status"] == error.status
            }
            (Ok(actual), None) => actual == &case["expected"],
            _ => false,
        };
        if !matched {
            failures.push(format!(
                "{} {}: expected {} / error {}, received {:?}",
                case["source"], case["name"], case["expected"], case["error"], result
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} contract differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
