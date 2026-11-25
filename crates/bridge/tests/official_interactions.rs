use codex_mobile_bridge::interactions::app_server_reply;
use serde_json::{json, Value};

fn request(method: &str, params: Value) -> Value {
    let mut params = params;
    params["threadId"] = json!("chat");
    json!({"id":731,"method":method,"params":params})
}

#[test]
fn user_input_answers_and_skip_are_official_result_payloads() {
    let request = request(
        "item/tool/requestUserInput",
        json!({"questions":[{
            "id":"continue","header":"下一步","question":"如何继续？","options":[{"label":"继续","description":"开始下一步"}]
        }]}),
    );
    assert_eq!(
        app_server_reply(&request, &json!({"answers":{"continue":["继续"]}})).unwrap(),
        json!({"answers":{"continue":{"answers":["继续"]}}})
    );
    assert_eq!(
        app_server_reply(&request, &json!({"skip":true,"answers":{}})).unwrap(),
        json!({"answers":{}})
    );
    assert_eq!(
        app_server_reply(&request, &json!({"answers":{"unknown":["继续"]}}))
            .unwrap_err()
            .status,
        400
    );
    assert!(app_server_reply(
        &request,
        &json!({"skip":true,"answers":{"continue":["继续"]}})
    )
    .is_err());
}

#[test]
fn mcp_form_responses_preserve_action_and_user_content_without_a_follower_envelope() {
    let form = request(
        "mcpServer/elicitation/request",
        json!({
            "serverName":"fixture","mode":"form","message":"允许调用吗？",
            "requestedSchema":{"type":"object","properties":{"allow":{"type":"boolean"}},"required":["allow"]}
        }),
    );
    assert_eq!(
        app_server_reply(&form, &json!({"action":"accept","content":{"allow":true}})).unwrap(),
        json!({"action":"accept","content":{"allow":true},"_meta":null})
    );
    for action in ["decline", "cancel"] {
        assert_eq!(
            app_server_reply(&form, &json!({"action":action})).unwrap(),
            json!({"action":action,"content":null,"_meta":null})
        );
    }
    assert!(app_server_reply(&form, &json!({"action":"accept","content":{}})).is_err());
    let mut url_form = form;
    url_form["params"]["mode"] = json!("url");
    assert_eq!(
        app_server_reply(
            &url_form,
            &json!({"action":"accept","content":{"allow":true}})
        )
        .unwrap_err()
        .status,
        422
    );
}

#[test]
fn approval_results_only_grant_the_requested_permissions_for_the_turn() {
    for method in [
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
    ] {
        let request = request(method, json!({}));
        assert_eq!(
            app_server_reply(&request, &json!({"decision":"accept"})).unwrap(),
            json!({"decision":"accept"})
        );
        assert_eq!(
            app_server_reply(&request, &json!({"decision":"decline"})).unwrap(),
            json!({"decision":"decline"})
        );
    }
    let permissions = json!({"fileSystem":{"read":["/workspace/file"]}});
    let request = request(
        "item/permissions/requestApproval",
        json!({"permissions":permissions}),
    );
    assert_eq!(
        app_server_reply(&request, &json!({"decision":"accept"})).unwrap(),
        json!({"permissions":permissions,"scope":"turn"})
    );
    assert_eq!(
        app_server_reply(&request, &json!({"decision":"decline"})).unwrap(),
        json!({"permissions":{},"scope":"turn"})
    );
}

#[test]
fn dynamic_onboarding_response_uses_the_official_tool_output_shape() {
    let request = request(
        "item/tool/call",
        json!({"tool":"request_onboarding_input","arguments":{"questions":[{
            "id":"name","header":"名称","question":"叫什么？","options":[]
        }]}}),
    );
    let result = app_server_reply(&request, &json!({"answers":{"name":["示例"]}})).unwrap();
    assert_eq!(result["success"], true);
    assert_eq!(result["contentItems"][0]["type"], "inputText");
    let body: Value =
        serde_json::from_str(result["contentItems"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body, json!({"answers":{"name":{"answers":["示例"]}}}));
}
