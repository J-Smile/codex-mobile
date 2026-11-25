use crate::{state::select, BridgeError, Result};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
pub fn request_id(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}
fn questions(value: &Value) -> Vec<Value> {
    value.as_array().into_iter().flatten().map(|q|json!({"id":text(&q["id"]),"header":text(&q["header"]),"question":text(&q["question"]),"secret":q["isSecret"]==true,
        "options":q["options"].as_array().into_iter().flatten().map(|o|json!({"label":text(&o["label"]),"description":text(&o["description"])})).collect::<Vec<_>>()})).collect()
}
fn fields(schema: &Value) -> Option<Vec<Value>> {
    if schema["type"] != "object" {
        return None;
    }
    schema["properties"]
        .as_object()?
        .iter()
        .map(|(name, p)| {
            let kind = p["type"].as_str()?;
            if !["string", "number", "integer", "boolean", "array"].contains(&kind)
                || ["oneOf", "anyOf", "allOf", "$ref"]
                    .iter()
                    .any(|k| p.get(k).is_some())
            {
                return None;
            }
            if kind == "array" && (p["items"]["type"] != "string" || !p["items"]["enum"].is_array())
            {
                return None;
            }
            let mut f = select(
                p,
                &["minLength", "maxLength", "minimum", "maximum", "format"],
            );
            f["name"] = json!(name);
            f["type"] = json!(kind);
            f["label"] = json!(p["title"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(name));
            f["description"] = json!(text(&p["description"]));
            f["required"] = json!(schema["required"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(name))));
            if kind == "array" {
                f["options"] = p["items"]["enum"].clone();
            } else if let Some(options) = p.get("enum") {
                f["options"] = options.clone();
            }
            if let Some(names) = p.get("enumNames") {
                f["optionNames"] = names.clone();
            }
            Some(f)
        })
        .collect()
}

pub fn normalize(request: &Value) -> Option<Value> {
    let params = &request["params"];
    let mut entry = json!({"id":request_id(&request["id"]),"kind":"desktop","title":"Codex 需要你确认",
        "message":params["reason"].as_str().filter(|s|!s.is_empty()).unwrap_or(text(&params["message"])),"replySupported":true});
    if let Some(turn) = params.get("turnId") {
        entry["turnId"] = turn.clone();
    }
    match text(&request["method"]) {
        "item/tool/requestUserInput" => {
            entry["kind"] = json!("questions");
            entry["title"] = json!("Codex 需要你回复");
            entry["skipSupported"] = json!(true);
            entry["questions"] = json!(questions(&params["questions"]));
        }
        "item/tool/call" => match text(&params["tool"]) {
            "request_onboarding_input" => {
                entry["kind"] = json!("questions");
                entry["title"] = json!("Codex 需要你回复");
                entry["questions"] = json!(questions(&params["arguments"]["questions"]));
            }
            "request_option_picker" | "request_environment_input" | "setup_codex_step" => {
                entry["replySupported"] = json!(false);
                entry["message"] = json!(params["arguments"]["question"]
                    .as_str()
                    .unwrap_or(text(&params["arguments"]["reason"])));
                entry["detail"] = json!("请在电脑上的 Codex 完成此设置。");
            }
            _ => return None,
        },
        "item/commandExecution/requestApproval" => {
            entry["kind"] = json!("approval");
            entry["title"] = json!("允许执行此命令？");
            entry["approveLabel"] = json!("允许本次");
            entry["detail"] = json!([
                text(&params["command"]).to_string(),
                params["cwd"]
                    .as_str()
                    .map(|s| format!("目录：{s}"))
                    .unwrap_or_default()
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"));
        }
        "item/fileChange/requestApproval" => {
            entry["kind"] = json!("approval");
            entry["title"] = json!("允许修改文件？");
            entry["approveLabel"] = json!("允许本次");
            entry["detail"] = json!(params["grantRoot"]
                .as_str()
                .map(|s| format!("目录：{s}"))
                .unwrap_or_default());
        }
        "item/permissions/requestApproval" => {
            entry["kind"] = json!("approval");
            entry["title"] = json!("允许本轮任务访问？");
            entry["approveLabel"] = json!("允许本轮");
            entry["detail"] = json!(serde_json::to_string_pretty(
                params.get("permissions").unwrap_or(&json!({}))
            )
            .unwrap());
        }
        "mcpServer/elicitation/request" => {
            let fields = fields(&params["requestedSchema"]);
            let special = text(&params["_meta"]["codex_approval_kind"]);
            let supported = params["mode"] == "form"
                && fields.is_some()
                && (special.is_empty() || special == "mcp_tool_call");
            entry["kind"] = json!("form");
            entry["title"] = json!(params["_meta"]["title"]
                .as_str()
                .unwrap_or("Codex 需要你确认"));
            entry["server"] = json!(text(&params["serverName"]));
            entry["replySupported"] = json!(supported);
            entry["fields"] = json!(fields.unwrap_or_default());
            entry["detail"] = json!(if special == "mcp_tool_call" {
                [
                    text(&params["_meta"]["tool_name"]).to_string(),
                    if params["_meta"]["tool_params"].is_object() {
                        serde_json::to_string_pretty(&params["_meta"]["tool_params"]).unwrap()
                    } else {
                        String::new()
                    },
                ]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
            } else {
                String::new()
            });
            if !supported {
                entry["desktopMessage"] = json!("请在电脑上的 Codex 完成此授权或验证。");
            }
        }
        "item/tool/requestOptionPicker" => {
            entry["replySupported"] = json!(false);
            entry["title"] = json!("Codex 需要你选择");
            entry["message"] = params["question"].clone();
            entry["detail"] = json!(params["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| [text(&o["label"]), text(&o["description"])]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join("："))
                .collect::<Vec<_>>()
                .join("\n"));
            entry["desktopMessage"] = json!("请在电脑上的 Codex 完成此选择。");
        }
        "item/plan/requestImplementation" => {
            entry["replySupported"] = json!(false);
            entry["title"] = json!("计划已准备好");
            entry["desktopMessage"] = json!("请在电脑上的 Codex 确认实施计划。");
        }
        _ => return None,
    }
    Some(entry)
}

pub fn async_pending(turns: &[Value]) -> Vec<Value> {
    let mut groups = vec![];
    let mut question_turns = HashMap::new();
    let mut answers = vec![];
    for (turn_index, turn) in turns.iter().enumerate() {
        for item in turn["items"].as_array().into_iter().flatten() {
            if item["type"] == "agentMessage" {
                if let Some(questions) = item["questions"].as_array().filter(|a| !a.is_empty()) {
                    let values:Vec<Value>=questions.iter().enumerate().map(|(index,q)|{
                let id=json!(["request_user_input_async",item["id"],index]).to_string();question_turns.insert(id.clone(),turn_index);
                json!({"id":id,"header":"","question":q["title"],"secret":false,"options":q["options"].as_array().into_iter().flatten().map(|label|json!({"label":label,"description":""})).collect::<Vec<_>>()})
            }).collect();
                    groups.push(json!({"id":json!(["request_user_input_async",item["id"]]).to_string(),"source":"async-message","turnId":turn["id"],"kind":"questions","title":"Codex 需要你回复","message":"","replySupported":true,"skipSupported":true,"questions":values}));
                }
            }
            if item["type"] != "userMessage" && item["type"] != "steeringUserMessage" {
                continue;
            }
            if item["type"] == "steeringUserMessage"
                && item["status"] != "accepted"
                && item["steerAcknowledged"] != true
            {
                continue;
            }
            let content = if item["type"] == "userMessage" {
                &item["content"]
            } else {
                &item["input"]
            };
            let body = content
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["type"] == "text" || p["type"] == "inputText")
                .map(|p| text(&p["text"]))
                .collect::<Vec<_>>()
                .join("\n");
            let body = body
                .rsplit("\n## My request:\n")
                .next()
                .unwrap_or("")
                .trim();
            let Some(body) = body
                .strip_prefix("<send_user_message_question_reply>")
                .and_then(|b| b.strip_suffix("</send_user_message_question_reply>"))
            else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(body.trim()) else {
                continue;
            };
            let entries = if let Some(values) = value.as_array() {
                values.clone()
            } else {
                vec![value]
            };
            if entries.iter().all(|a| {
                a["questionItemId"].is_string()
                    && a["question"].is_string()
                    && a["answer"].is_string()
            }) {
                answers.extend(entries.into_iter().map(|a| {
                    (
                        a["questionItemId"].as_str().unwrap().to_string(),
                        turn_index,
                    )
                }));
            }
        }
    }
    let answered: HashSet<String> = answers
        .into_iter()
        .filter(|(id, turn)| question_turns.get(id).is_some_and(|q| q <= turn))
        .map(|(id, _)| id)
        .collect();
    groups
        .into_iter()
        .filter_map(|mut group| {
            group["questions"]
                .as_array_mut()
                .unwrap()
                .retain(|q| !answered.contains(text(&q["id"])));
            (!group["questions"].as_array().unwrap().is_empty()).then_some(group)
        })
        .collect()
}
fn read_answers(known: &[Value], input: &Value) -> Result<Map<String, Value>> {
    let answers = input
        .as_object()
        .ok_or_else(|| BridgeError::invalid("请选择答案或填写回复。"))?;
    if answers
        .keys()
        .any(|id| !known.iter().any(|q| q["id"] == *id))
    {
        return Err(BridgeError::invalid("问题已变化，请刷新后重新填写。"));
    }
    let mut result = Map::new();
    for question in known {
        let id = text(&question["id"]);
        let value = &input[id];
        if !value.as_array().is_some_and(|a| {
            !a.is_empty()
                && a.iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
        }) {
            return Err(BridgeError::invalid("请完成每个问题后再发送回复。"));
        }
        result.insert(id.into(), value.clone());
    }
    Ok(result)
}

/// Validate an interaction and produce the result for the original official
/// server request. Transport replies retain its original numeric/string ID.
pub fn app_server_reply(request: &Value, input: &Value) -> Result<Value> {
    if !input.is_object() {
        return Err(BridgeError::invalid("回复格式无效。"));
    }
    let interaction = normalize(request)
        .filter(|entry| entry["replySupported"] == true)
        .ok_or_else(|| BridgeError::new(422, "此请求需要在电脑上的 Codex 完成。"))?;
    if input["skip"] == true && interaction["kind"] != "questions" {
        return Err(BridgeError::invalid("此请求不能跳过，请明确允许或拒绝。"));
    }
    match text(&request["method"]) {
        "item/tool/requestUserInput" => {
            user_input_response(&request["params"]["questions"], input, true)
        }
        "item/tool/call" => {
            let response =
                user_input_response(&request["params"]["arguments"]["questions"], input, false)?;
            Ok(
                json!({"success":true,"contentItems":[{"type":"inputText","text":response.to_string()}]}),
            )
        }
        "mcpServer/elicitation/request" => form_response(&request["params"], input),
        method => {
            if input["decision"] != "accept" && input["decision"] != "decline" {
                return Err(BridgeError::invalid("请选择允许或拒绝。"));
            }
            if method == "item/permissions/requestApproval" {
                Ok(json!({"permissions":if input["decision"]=="accept" {
                    request["params"]["permissions"].clone()
                } else {json!({})},"scope":"turn"}))
            } else {
                Ok(json!({"decision":input["decision"]}))
            }
        }
    }
}

fn user_input_response(known: &Value, input: &Value, skip_supported: bool) -> Result<Value> {
    let answers = if input["skip"] == true {
        if !skip_supported {
            return Err(BridgeError::invalid("此问题不支持在手机上跳过。"));
        }
        if input
            .get("answers")
            .is_some_and(|answers| !answers.as_object().is_some_and(Map::is_empty))
        {
            return Err(BridgeError::invalid("跳过时不能同时提交答案。"));
        }
        Map::new()
    } else {
        read_answers(&questions(known), &input["answers"])?
            .into_iter()
            .map(|(id, answers)| (id, json!({"answers":answers})))
            .collect()
    };
    Ok(json!({"answers":answers}))
}
pub fn async_reply(turns: &[Value], id: &str, input: &Value) -> Result<String> {
    if !input.is_object() {
        return Err(BridgeError::invalid("回复格式无效。"));
    }
    let interaction = async_pending(turns)
        .into_iter()
        .find(|i| i["id"] == id)
        .ok_or_else(|| BridgeError::new(409, "这项请求已在其他设备处理或已结束。"))?;
    if input["skip"] == true {
        return Err(BridgeError::invalid("异步问题请逐题跳过。"));
    }
    let skipped = input.get("skipped").cloned().unwrap_or(json!([]));
    let known = interaction["questions"].as_array().unwrap();
    if !skipped
        .as_array()
        .is_some_and(|a| a.iter().all(|id| known.iter().any(|q| q["id"] == *id)))
    {
        return Err(BridgeError::invalid("问题已变化，请刷新后重新填写。"));
    }
    let answered: Vec<Value> = known
        .iter()
        .filter(|q| !skipped.as_array().unwrap().contains(&q["id"]))
        .cloned()
        .collect();
    if answered.is_empty() {
        return Err(BridgeError::invalid("全部跳过无需发送回复。"));
    }
    let answers = read_answers(&answered, &input["answers"])?;
    if answers.values().any(|a| a.as_array().unwrap().len() != 1) {
        return Err(BridgeError::invalid("每个问题请提交一条回复。"));
    }
    let values: Vec<Value> = answered
        .iter()
        .map(|q| {
            json!({
                "questionItemId": q["id"],
                "question": q["question"],
                "answer": answers[text(&q["id"])][0].as_str().unwrap().trim()
            })
        })
        .collect();
    Ok(format!(
        "<send_user_message_question_reply>\n{}\n</send_user_message_question_reply>",
        json!(values)
    ))
}

fn form_response(params: &Value, input: &Value) -> Result<Value> {
    let action = text(&input["action"]);
    if !["accept", "decline", "cancel"].contains(&action) {
        return Err(BridgeError::invalid("请选择确认或拒绝。"));
    }
    if action != "accept" {
        return Ok(json!({"action":action,"content":null,"_meta":null}));
    }
    let content = input["content"]
        .as_object()
        .ok_or_else(|| BridgeError::invalid("请填写需要的信息。"))?;
    let schema = &params["requestedSchema"];
    let properties = schema["properties"].as_object().unwrap();
    if content.keys().any(|key| !properties.contains_key(key)) {
        return Err(BridgeError::invalid("表单字段已变化，请刷新后重试。"));
    }
    for (name, p) in properties {
        let value = &input["content"][name];
        let label = p["title"].as_str().unwrap_or(name);
        if !content.contains_key(name) || value == "" {
            if schema["required"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(name)))
            {
                return Err(BridgeError::invalid(format!("请填写“{label}”。")));
            }
            continue;
        }
        let valid = match text(&p["type"]) {
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_f64().is_some_and(|n| n.fract() == 0.0),
            "boolean" => value.is_boolean(),
            "array" => value.as_array().is_some_and(|a| {
                a.iter().all(|v| {
                    v.is_string()
                        && p["items"]["enum"]
                            .as_array()
                            .is_some_and(|opts| opts.contains(v))
                })
            }),
            _ => false,
        };
        if !valid
            || p["enum"]
                .as_array()
                .is_some_and(|opts| !opts.contains(value))
        {
            return Err(BridgeError::invalid(format!("“{label}”的内容无效。")));
        }
        if let Some(n) = value.as_f64() {
            if p["minimum"].as_f64().is_some_and(|min| n < min)
                || p["maximum"].as_f64().is_some_and(|max| n > max)
            {
                return Err(BridgeError::invalid(format!("“{label}”超出了允许的范围。")));
            }
        }
        if let Some(s) = value.as_str() {
            let len = s.encode_utf16().count() as u64;
            if p["minLength"].as_u64().is_some_and(|min| len < min)
                || p["maxLength"].as_u64().is_some_and(|max| len > max)
                || p["pattern"].as_str().is_some_and(|pattern| {
                    regex::Regex::new(pattern).map_or(true, |re| !re.is_match(s))
                })
            {
                return Err(BridgeError::invalid(format!("请检查“{label}”的格式。")));
            }
        }
        if let Some(a) = value.as_array() {
            if p["minItems"]
                .as_u64()
                .is_some_and(|min| a.len() < (min as usize))
                || p["maxItems"]
                    .as_u64()
                    .is_some_and(|max| a.len() > (max as usize))
            {
                return Err(BridgeError::invalid(format!("请检查“{label}”的选项数量。")));
            }
        }
    }
    Ok(json!({"action":"accept","content":content,"_meta":null}))
}
