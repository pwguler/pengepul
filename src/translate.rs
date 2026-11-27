use serde_json::{Map, Value, json};

const ANTHROPIC_WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";
const ANTHROPIC_WEB_SEARCH_TOOL_TYPES: [&str; 1] = [ANTHROPIC_WEB_SEARCH_TOOL_TYPE];

#[must_use]
fn resolve_model(model: Option<&str>) -> String {
    model.unwrap_or("claude-sonnet-4-6").to_string()
}

#[must_use]
pub fn openai_to_anthropic(body: &Value) -> Value {
    let messages = value_array(body.get("messages"));
    let mut out = Map::new();
    out.insert(
        "model".to_string(),
        Value::String(resolve_model(body.get("model").and_then(Value::as_str))),
    );
    out.insert(
        "max_tokens".to_string(),
        body.get("max_completion_tokens")
            .or_else(|| body.get("max_tokens"))
            .cloned()
            .unwrap_or_else(|| json!(8192)),
    );
    copy_if_present(body, &mut out, "stream");
    copy_if_present(body, &mut out, "temperature");
    copy_if_present(body, &mut out, "top_p");
    if let Some(stop) = body.get("stop") {
        out.insert(
            "stop_sequences".to_string(),
            if stop.is_array() {
                stop.clone()
            } else {
                Value::Array(vec![stop.clone()])
            },
        );
    }

    let system = anthropic_system_from_messages(&messages);
    if !system.is_empty() {
        out.insert("system".to_string(), Value::Array(system));
    }

    let mut output_messages = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str);
        match role {
            Some("tool") => output_messages.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": message.get("tool_call_id").cloned().unwrap_or(Value::Null),
                    "content": text_from_content(message.get("content").unwrap_or(&Value::Null))
                }]
            })),
            Some("assistant") if message.get("tool_calls").is_some() => {
                let mut content = Vec::new();
                let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
                if !text.is_empty() {
                    content.push(json!({"type": "text", "text": text}));
                }
                for call in value_array(message.get("tool_calls")) {
                    let function = call.get("function").unwrap_or(&Value::Null);
                    let args = function
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    let parsed_args =
                        serde_json::from_str::<Value>(args).unwrap_or_else(|_| Value::String(args.to_string()));
                    content.push(json!({
                        "type": "tool_use",
                        "id": call.get("id").cloned().unwrap_or(Value::Null),
                        "name": function.get("name").cloned().unwrap_or(Value::Null),
                        "input": parsed_args
                    }));
                }
                output_messages.push(json!({"role": "assistant", "content": content}));
            }
            Some(message_role @ ("user" | "assistant")) => output_messages.push(json!({
                "role": message_role,
                "content": openai_content_to_anthropic(message.get("content").unwrap_or(&Value::Null))
            })),
            _ => {}
        }
    }
    out.insert("messages".to_string(), Value::Array(output_messages));

    if let Some(thinking) =
        thinking_from_effort(body.get("reasoning_effort").and_then(Value::as_str), None)
    {
        out.insert("thinking".to_string(), thinking);
    }

    let mut tools = Vec::new();
    for tool in value_array(body.get("tools"))
        .into_iter()
        .chain(value_array(body.get("responses_tools")))
    {
        if let Some(tool) = openai_tool_to_anthropic(tool) {
            tools.push(tool);
        }
    }
    if !tools.is_empty() {
        out.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(choice) = anthropic_tool_choice(body, out.get("tools").is_some()) {
        out.insert("tool_choice".to_string(), choice);
    }

    Value::Object(out)
}

#[must_use]
pub fn responses_to_anthropic(body: &Value) -> Value {
    let input = body
        .get("input")
        .or_else(|| body.get("messages"))
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let messages = responses_input_to_openai_messages(&input);
    let mut pseudo = Map::new();
    insert_if_some(&mut pseudo, "model", body.get("model").cloned());
    pseudo.insert("messages".to_string(), Value::Array(messages));
    pseudo.insert(
        "stream".to_string(),
        body.get("stream").cloned().unwrap_or(Value::Bool(false)),
    );
    copy_if_present(body, &mut pseudo, "temperature");
    copy_if_present(body, &mut pseudo, "top_p");
    if let Some(value) = body.get("max_output_tokens") {
        pseudo.insert("max_completion_tokens".to_string(), value.clone());
    }
    copy_if_present(body, &mut pseudo, "tools");
    copy_if_present(body, &mut pseudo, "tool_choice");
    copy_if_present(body, &mut pseudo, "parallel_tool_calls");

    let mut out = openai_to_anthropic(&Value::Object(pseudo));
    if let Some(instructions) = body.get("instructions").filter(|value| !is_empty(value)) {
        out["system"] = json!([{"type": "text", "text": instructions}]);
    }
    let reasoning = body.get("reasoning").unwrap_or(&Value::Null);
    if let Some(thinking) = thinking_from_effort(
        reasoning.get("effort").and_then(Value::as_str),
        reasoning.get("summary").and_then(Value::as_str),
    ) {
        out["thinking"] = thinking;
    }
    out
}

#[must_use]
pub fn anthropic_to_responses(payload: &Value, model: &str) -> Value {
    let mut output = Vec::new();
    let mut text = String::new();
    let mut output_text_parts = Vec::new();
    let mut annotations = Vec::new();

    for block in value_array(payload.get("content")) {
        match block.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                flush_response_text(
                    &mut output,
                    &mut output_text_parts,
                    &mut text,
                    &mut annotations,
                );
                output.push(json!({
                    "type": "reasoning",
                    "summary": [{"type": "summary_text", "text": block.get("thinking").and_then(Value::as_str).unwrap_or("")}]
                }));
            }
            Some("text") => {
                text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
                annotations.extend(anthropic_citations_to_openai(
                    block.get("citations").and_then(Value::as_array),
                ));
            }
            Some("tool_use") => {
                flush_response_text(
                    &mut output,
                    &mut output_text_parts,
                    &mut text,
                    &mut annotations,
                );
                output.push(json!({
                    "type": "function_call",
                    "call_id": block.get("id").cloned().unwrap_or(Value::Null),
                    "name": block.get("name").cloned().unwrap_or(Value::Null),
                    "arguments": serde_json::to_string(block.get("input").unwrap_or(&json!({}))).unwrap_or_else(|_| "{}".to_string())
                }));
            }
            Some("server_tool_use")
                if block.get("name").and_then(Value::as_str) == Some("web_search") =>
            {
                flush_response_text(
                    &mut output,
                    &mut output_text_parts,
                    &mut text,
                    &mut annotations,
                );
                output.push(json!({
                    "type": "web_search_call",
                    "id": block.get("id").cloned().unwrap_or(Value::Null),
                    "status": "completed",
                    "action": {
                        "type": "search",
                        "query": block.get("input").and_then(|input| input.get("query")).and_then(Value::as_str).unwrap_or("")
                    }
                }));
            }
            _ => {}
        }
    }
    flush_response_text(
        &mut output,
        &mut output_text_parts,
        &mut text,
        &mut annotations,
    );

    let usage = payload.get("usage").unwrap_or(&Value::Null);
    let input_tokens = int_field(usage, "input_tokens");
    let output_tokens = int_field(usage, "output_tokens");
    json!({
        "id": payload.get("id").cloned().unwrap_or_else(|| Value::String(format!("resp_{}", uuid::Uuid::new_v4().simple()))),
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": if payload.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") { "incomplete" } else { "completed" },
        "model": model,
        "output": output,
        "output_text": output_text_parts.join(""),
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
            "input_tokens_details": {"cached_tokens": int_field(usage, "cache_read_input_tokens")},
            "output_tokens_details": {"reasoning_tokens": 0}
        }
    })
}

#[must_use]
pub fn anthropic_to_openai(payload: &Value, model: &str) -> Value {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();
    for block in value_array(payload.get("content")) {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                text_parts.push(
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
            }
            Some("tool_use") => {
                tool_calls.push(json!({
                    "id": block.get("id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": block.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": serde_json::to_string(block.get("input").unwrap_or(&json!({}))).unwrap_or_else(|_| "{}".to_string())
                    }
                }));
            }
            _ => {}
        }
    }

    let mut message = json!({"role": "assistant", "content": text_parts.join("")});
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    json!({
        "id": payload.get("id").cloned().unwrap_or_else(|| Value::String(format!("chatcmpl_{}", uuid::Uuid::new_v4().simple()))),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason(payload.get("stop_reason").and_then(Value::as_str), has_tool_calls)
        }],
        "usage": usage_to_openai(payload.get("usage").unwrap_or(&Value::Null))
    })
}

#[must_use]
pub fn chat_to_responses_request(body: &Value) -> Value {
    let mut instructions = Vec::new();
    let mut input = Vec::new();
    for message in value_array(body.get("messages")) {
        let role = message.get("role").and_then(Value::as_str);
        match role {
            Some("system" | "developer") => {
                let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
                if !text.is_empty() {
                    instructions.push(text);
                }
            }
            Some("tool") => input.push(json!({
                "type": "function_call_output",
                "call_id": message.get("tool_call_id").cloned().unwrap_or(Value::Null),
                "output": text_from_content(message.get("content").unwrap_or(&Value::Null))
            })),
            Some("assistant") if message.get("tool_calls").is_some() => {
                let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
                if !text.is_empty() {
                    input.push(json!({"role": "assistant", "content": text}));
                }
                for call in value_array(message.get("tool_calls")) {
                    let function = call.get("function").unwrap_or(&Value::Null);
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.get("id").cloned().unwrap_or(Value::Null),
                        "name": function.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": function.get("arguments").cloned().unwrap_or_else(|| json!("{}"))
                    }));
                }
            }
            Some(message_role @ ("user" | "assistant")) => input.push(json!({
                "role": message_role,
                "content": chat_content_to_responses(message.get("content"))
            })),
            _ => {}
        }
    }

    let mut out = Map::new();
    insert_if_some(&mut out, "model", body.get("model").cloned());
    out.insert("input".to_string(), Value::Array(input));
    if !instructions.is_empty() {
        out.insert(
            "instructions".to_string(),
            Value::String(instructions.join("\n\n")),
        );
    }
    copy_if_present(body, &mut out, "stream");
    copy_if_present(body, &mut out, "temperature");
    copy_if_present(body, &mut out, "top_p");
    if let Some(tokens) = body
        .get("max_completion_tokens")
        .or_else(|| body.get("max_tokens"))
    {
        out.insert("max_output_tokens".to_string(), tokens.clone());
    }
    if let Some(effort) = body.get("reasoning_effort") {
        out.insert("reasoning".to_string(), json!({"effort": effort}));
    }
    let mut tools = Vec::new();
    for tool in value_array(body.get("tools")) {
        if let Some(tool) = responses_tool_from_chat_tool(tool) {
            tools.push(tool);
        }
    }
    for tool in value_array(body.get("responses_tools")) {
        tools.push(tool.clone());
    }
    if !tools.is_empty() {
        out.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(choice) = body.get("responses_tool_choice") {
        out.insert("tool_choice".to_string(), choice.clone());
    } else if let Some(choice) = body.get("tool_choice") {
        out.insert(
            "tool_choice".to_string(),
            responses_tool_choice_from_chat(choice),
        );
    }
    copy_if_present(body, &mut out, "parallel_tool_calls");
    Value::Object(out)
}

#[must_use]
pub fn anthropic_to_responses_request(body: &Value) -> Value {
    let mut out = Map::new();
    insert_if_some(&mut out, "model", body.get("model").cloned());
    copy_if_present(body, &mut out, "stream");
    if let Some(max_tokens) = body.get("max_tokens") {
        out.insert("max_output_tokens".to_string(), max_tokens.clone());
    }
    copy_if_present(body, &mut out, "temperature");
    copy_if_present(body, &mut out, "top_p");
    if let Some(system) = body.get("system") {
        let instructions = if let Some(system) = system.as_str() {
            system.to_string()
        } else {
            value_array(Some(system))
                .into_iter()
                .map(text_from_content)
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        if !instructions.is_empty() {
            out.insert("instructions".to_string(), Value::String(instructions));
        }
    }
    let thinking = body.get("thinking").unwrap_or(&Value::Null);
    if thinking.get("type").and_then(Value::as_str) == Some("enabled") {
        out.insert(
            "reasoning".to_string(),
            json!({"effort": effort_from_budget(thinking.get("budget_tokens").and_then(Value::as_i64))}),
        );
    }

    let mut input = Vec::new();
    for message in value_array(body.get("messages")) {
        let role = message.get("role").cloned().unwrap_or(Value::Null);
        let content = message
            .get("content")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        if let Some(blocks) = content.as_array() {
            let mut parts = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => parts.push(json!({"type": "input_text", "text": block.get("text").cloned().unwrap_or_else(|| json!(""))})),
                    Some("image") => {
                        if let Some(image) = anthropic_image_to_responses(block) {
                            parts.push(image);
                        }
                    }
                    Some("tool_use") => {
                        flush_input_parts(&mut input, &role, &mut parts);
                        input.push(json!({
                            "type": "function_call",
                            "call_id": block.get("id").cloned().unwrap_or(Value::Null),
                            "name": block.get("name").cloned().unwrap_or(Value::Null),
                            "arguments": serde_json::to_string(block.get("input").unwrap_or(&json!({}))).unwrap_or_else(|_| "{}".to_string())
                        }));
                    }
                    Some("tool_result") => {
                        flush_input_parts(&mut input, &role, &mut parts);
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
                            "output": text_from_content(block.get("content").unwrap_or(&Value::Null))
                        }));
                    }
                    _ => {}
                }
            }
            flush_input_parts(&mut input, &role, &mut parts);
        } else {
            input.push(json!({"role": role, "content": content}));
        }
    }
    out.insert("input".to_string(), Value::Array(input));

    let tools = value_array(body.get("tools"))
        .into_iter()
        .map(anthropic_tool_to_responses)
        .collect::<Vec<_>>();
    if !tools.is_empty() {
        out.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(choice) = body.get("tool_choice") {
        out.insert(
            "tool_choice".to_string(),
            responses_tool_choice_from_anthropic(choice),
        );
    }
    Value::Object(out)
}

#[must_use]
pub fn responses_to_chat_completion(payload: &Value, model: &str) -> Value {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();

    for item in value_array(payload.get("output")) {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                for summary in value_array(item.get("summary")) {
                    reasoning.push_str(summary.get("text").and_then(Value::as_str).unwrap_or(""));
                }
            }
            Some("message") => {
                for content in value_array(item.get("content")) {
                    if matches!(
                        content.get("type").and_then(Value::as_str),
                        Some("output_text" | "text")
                    ) {
                        text.push_str(content.get("text").and_then(Value::as_str).unwrap_or(""));
                    }
                }
            }
            Some("function_call") => {
                tool_calls.push(json!({
                    "id": item.get("call_id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": item.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": item.get("arguments").cloned().unwrap_or_else(|| json!("{}"))
                    }
                }));
            }
            _ => {}
        }
    }

    let mut message = json!({"role": "assistant", "content": text});
    if !reasoning.is_empty() {
        message["reasoning_content"] = Value::String(reasoning);
    }
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    let usage = payload.get("usage").unwrap_or(&Value::Null);
    let input_tokens = int_field(usage, "input_tokens");
    let output_tokens = int_field(usage, "output_tokens");
    json!({
        "id": format!("chatcmpl_{}", uuid::Uuid::new_v4().simple()),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": if has_tool_calls {
                "tool_calls"
            } else if payload.get("status").and_then(Value::as_str) == Some("incomplete") {
                "length"
            } else {
                "stop"
            }
        }],
        "usage": {
            "prompt_tokens": input_tokens,
            "completion_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
            "prompt_tokens_details": usage.get("input_tokens_details").cloned().unwrap_or_else(|| json!({"cached_tokens": 0})),
            "completion_tokens_details": usage.get("output_tokens_details").cloned().unwrap_or_else(|| json!({"reasoning_tokens": 0}))
        }
    })
}

/// An `OpenAI` Responses request → `OpenAI` Chat Completions request: what
/// grok build's relay has to be handed for a harness that speaks Responses
/// to reach it. The inverse of the response side, `chat_to_responses_message`
/// — input items become messages, `instructions` becomes a system message,
/// `reasoning.effort` becomes `reasoning_effort`, and the function tools
/// unwrap into the chat dialect's nested shape.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn responses_to_chat_request(body: &Value) -> Value {
    let mut messages = Vec::new();
    if let Some(instructions) =
        text_or_empty(body.get("instructions")).filter(|text| !text.is_empty())
    {
        messages.push(json!({"role": "system", "content": instructions}));
    }
    let input = body.get("input").unwrap_or(&Value::Null);
    let items: Vec<&Value> = match input {
        Value::Array(items) => items.iter().collect(),
        // A bare string is the shorthand for one user message.
        Value::String(text) => {
            messages.push(json!({"role": "user", "content": text}));
            Vec::new()
        }
        _ => Vec::new(),
    };
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                messages.push(json!({
                    "role": "assistant",
                    "content": Value::Null,
                    "tool_calls": [{
                        "id": item.get("call_id").cloned().unwrap_or(Value::Null),
                        "type": "function",
                        "function": {
                            "name": item.get("name").cloned().unwrap_or(Value::Null),
                            "arguments": item.get("arguments").cloned().unwrap_or_else(|| json!("{}"))
                        }
                    }]
                }));
            }
            Some("function_call_output") => {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": item.get("call_id").cloned().unwrap_or(Value::Null),
                    "content": item.get("output").cloned().unwrap_or_else(|| json!(""))
                }));
            }
            // Reasoning items are the model's own history; a chat request has
            // no place for them and nothing is lost by dropping them.
            Some("reasoning") => {}
            Some("message") => {
                let role = item.get("role").and_then(Value::as_str).unwrap_or("user");
                let text = item
                    .get("content")
                    .map(text_from_content)
                    .unwrap_or_default();
                if !text.is_empty() || role != "assistant" {
                    messages.push(json!({"role": role, "content": text}));
                }
            }
            _ => {
                // A plain `{role, content}` item, or nothing recognizable.
                if let Some(role) = item.get("role").and_then(Value::as_str) {
                    let text = item
                        .get("content")
                        .map(text_from_content)
                        .unwrap_or_default();
                    messages.push(json!({"role": role, "content": text}));
                }
            }
        }
    }

    let mut out = Map::new();
    insert_if_some(&mut out, "model", body.get("model").cloned());
    copy_if_present(body, &mut out, "stream");
    copy_if_present(body, &mut out, "temperature");
    copy_if_present(body, &mut out, "top_p");
    if let Some(tokens) = body.get("max_output_tokens") {
        out.insert("max_tokens".to_string(), tokens.clone());
    }
    if let Some(effort) = body
        .get("reasoning")
        .and_then(|reasoning| reasoning.get("effort"))
    {
        out.insert("reasoning_effort".to_string(), effort.clone());
    }
    if !messages.is_empty() {
        out.insert("messages".to_string(), Value::Array(messages));
    }
    let mut tools = Vec::new();
    for tool in value_array(body.get("tools")) {
        if tool.get("type").and_then(Value::as_str) != Some("function") {
            // Server-side tools (web_search, ...) have no chat-dialect form;
            // the request stays honest about what it can run.
            continue;
        }
        tools.push(json!({
            "type": "function",
            "function": {
                "name": tool.get("name").cloned().unwrap_or(Value::Null),
                "description": tool.get("description").cloned().unwrap_or_else(|| json!("")),
                "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"}))
            }
        }));
    }
    if !tools.is_empty() {
        out.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(choice) = body.get("tool_choice") {
        out.insert(
            "tool_choice".to_string(),
            chat_tool_choice_from_responses(choice),
        );
    }
    copy_if_present(body, &mut out, "parallel_tool_calls");
    Value::Object(out)
}

fn text_or_empty(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        other => Some(text_from_content(other)),
    }
}

fn chat_tool_choice_from_responses(choice: &Value) -> Value {
    if choice.get("type").and_then(Value::as_str) == Some("function") {
        return json!({
            "type": "function",
            "function": {"name": choice.get("name").cloned().unwrap_or(Value::Null)}
        });
    }
    choice.clone()
}

/// A chat completion reply as the Responses reply the client asked in: the
/// inverse of [`responses_to_chat_completion`]. Reasoning content becomes a
/// `reasoning` item, tool calls become `function_call` items, and the chat
/// usage map re-keys into the Responses one.
#[must_use]
pub fn chat_to_responses_message(payload: &Value, model: &str) -> Value {
    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let message = choice.get("message").unwrap_or(&Value::Null);
    let mut output = Vec::new();
    if let Some(reasoning) = message
        .get("reasoning_content")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        output.push(json!({
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": reasoning}]
        }));
    }
    for call in value_array(message.get("tool_calls")) {
        let function = call.get("function").unwrap_or(&Value::Null);
        output.push(json!({
            "type": "function_call",
            "call_id": call.get("id").cloned().unwrap_or(Value::Null),
            "name": function.get("name").cloned().unwrap_or(Value::Null),
            "arguments": function.get("arguments").cloned().unwrap_or_else(|| json!("{}"))
        }));
    }
    let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
    if !text.is_empty() {
        output.push(json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text, "annotations": []}]
        }));
    }

    let usage = payload.get("usage").unwrap_or(&Value::Null);
    let input_tokens = int_field(usage, "prompt_tokens");
    let output_tokens = int_field(usage, "completion_tokens");
    let status = if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        "incomplete"
    } else {
        "completed"
    };
    json!({
        "id": format!("resp_{}", uuid::Uuid::new_v4().simple()),
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": status,
        "model": model,
        "output": output,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
            "input_tokens_details": usage.get("prompt_tokens_details").cloned().unwrap_or_else(|| json!({"cached_tokens": 0})),
            "output_tokens_details": usage.get("completion_tokens_details").cloned().unwrap_or_else(|| json!({"reasoning_tokens": 0}))
        }
    })
}

#[must_use]
pub fn responses_to_anthropic_message(payload: &Value, model: &str) -> Value {
    let mut content = Vec::new();
    let mut has_tool_use = false;
    for item in value_array(payload.get("output")) {
        match item.get("type").and_then(Value::as_str) {
            Some("reasoning") => {
                let text = value_array(item.get("summary"))
                    .iter()
                    .filter_map(|summary| summary.get("text").and_then(Value::as_str))
                    .collect::<String>();
                if !text.is_empty() {
                    content.push(json!({"type": "thinking", "thinking": text}));
                }
            }
            Some("message") => {
                let mut text = String::new();
                let mut citations = Vec::new();
                for block in value_array(item.get("content")) {
                    if matches!(
                        block.get("type").and_then(Value::as_str),
                        Some("output_text" | "text")
                    ) {
                        text.push_str(block.get("text").and_then(Value::as_str).unwrap_or(""));
                        citations.extend(openai_annotations_to_anthropic(
                            block.get("annotations").and_then(Value::as_array),
                        ));
                    }
                }
                if !text.is_empty() {
                    let mut text_block = json!({"type": "text", "text": text});
                    if !citations.is_empty() {
                        text_block["citations"] = Value::Array(citations);
                    }
                    content.push(text_block);
                }
            }
            Some("function_call") => {
                has_tool_use = true;
                let args = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                let parsed = serde_json::from_str::<Value>(args)
                    .unwrap_or_else(|_| Value::String(args.to_string()));
                content.push(json!({
                    "type": "tool_use",
                    "id": item.get("call_id").cloned().unwrap_or(Value::Null),
                    "name": item.get("name").cloned().unwrap_or(Value::Null),
                    "input": parsed
                }));
            }
            Some("web_search_call") => {
                content.push(json!({
                    "type": "server_tool_use",
                    "id": item.get("id").cloned().unwrap_or(Value::Null),
                    "name": "web_search",
                    "input": {"query": web_search_query_from_action(item.get("action").unwrap_or(&Value::Null))}
                }));
            }
            _ => {}
        }
    }
    let usage = payload.get("usage").unwrap_or(&Value::Null);
    json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": if payload.get("status").and_then(Value::as_str) == Some("incomplete") {
            "max_tokens"
        } else if has_tool_use {
            "tool_use"
        } else {
            "end_turn"
        },
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": int_field(usage, "input_tokens"),
            "output_tokens": int_field(usage, "output_tokens")
        }
    })
}

/// Anthropic Messages request → `OpenAI` Chat Completions request: what a
/// configured endpoint has to be handed for a harness that speaks Messages
/// to reach it. The inverse of `openai_to_anthropic`, so the two read
/// together — a system block becomes a system message, a `tool_use` block
/// becomes a `tool_calls` entry, and a `tool_result` block leaves its user
/// turn to become a `tool` message of its own, which is where this dialect
/// keeps them.
#[must_use]
pub fn anthropic_to_chat_request(body: &Value) -> Value {
    let mut out = Map::new();
    insert_if_some(&mut out, "model", body.get("model").cloned());
    copy_if_present(body, &mut out, "stream");
    copy_if_present(body, &mut out, "temperature");
    copy_if_present(body, &mut out, "top_p");
    copy_if_present(body, &mut out, "max_tokens");
    if let Some(stop) = body.get("stop_sequences") {
        out.insert("stop".to_string(), stop.clone());
    }
    let thinking = body.get("thinking").unwrap_or(&Value::Null);
    if thinking.get("type").and_then(Value::as_str) == Some("enabled") {
        out.insert(
            "reasoning_effort".to_string(),
            Value::String(
                effort_from_budget(thinking.get("budget_tokens").and_then(Value::as_i64))
                    .to_string(),
            ),
        );
    }

    let mut messages = Vec::new();
    if let Some(system) = body.get("system") {
        let text = system.as_str().map_or_else(
            || {
                value_array(Some(system))
                    .into_iter()
                    .map(text_from_content)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            },
            ToString::to_string,
        );
        if !text.is_empty() {
            messages.push(json!({"role": "system", "content": text}));
        }
    }
    for message in value_array(body.get("messages")) {
        push_chat_messages(&mut messages, message);
    }
    out.insert("messages".to_string(), Value::Array(messages));

    let tools = value_array(body.get("tools"))
        .into_iter()
        .filter_map(anthropic_tool_to_chat)
        .collect::<Vec<_>>();
    if !tools.is_empty() {
        out.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(choice) = body.get("tool_choice") {
        out.insert(
            "tool_choice".to_string(),
            chat_tool_choice_from_anthropic(choice),
        );
        // Anthropic hangs this off tool_choice; this dialect keeps it at the
        // top level. Dropping it gave a client that asked for one call at a
        // time several.
        if choice
            .get("disable_parallel_tool_use")
            .and_then(Value::as_bool)
            == Some(true)
        {
            out.insert("parallel_tool_calls".to_string(), Value::Bool(false));
        }
    }
    Value::Object(out)
}

/// `OpenAI` Chat Completions reply → Anthropic Messages reply, the inverse of
/// `anthropic_to_openai`.
#[must_use]
pub fn chat_to_anthropic_message(payload: &Value, model: &str) -> Value {
    let choices = value_array(payload.get("choices"));
    let null = Value::Null;
    let choice = choices.first().copied().unwrap_or(&null);
    let message = choice.get("message").unwrap_or(&null);

    let mut content = Vec::new();
    // Reasoning arrives under one name or the other depending on the
    // gateway; both become one thinking block, and neither is invented.
    let reasoning = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !reasoning.is_empty() {
        content.push(json!({"type": "thinking", "thinking": reasoning}));
    }
    let text = text_from_content(message.get("content").unwrap_or(&null));
    if !text.is_empty() {
        content.push(json!({"type": "text", "text": text}));
    }
    let mut has_tool_use = false;
    for call in value_array(message.get("tool_calls")) {
        has_tool_use = true;
        let function = call.get("function").unwrap_or(&null);
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}");
        content.push(json!({
            "type": "tool_use",
            "id": call.get("id").cloned().unwrap_or(Value::Null),
            "name": function.get("name").cloned().unwrap_or(Value::Null),
            "input": serde_json::from_str::<Value>(arguments).unwrap_or_else(|_| json!({}))
        }));
    }

    let usage = payload.get("usage").unwrap_or(&null);
    json!({
        "id": payload.get("id").cloned().unwrap_or_else(
            || Value::String(format!("msg_{}", uuid::Uuid::new_v4().simple()))
        ),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": anthropic_stop_reason(
            choice.get("finish_reason").and_then(Value::as_str),
            has_tool_use
        ),
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": int_field(usage, "prompt_tokens"),
            "output_tokens": int_field(usage, "completion_tokens")
        }
    })
}

/// A Chat Completions finish reason under its Messages name. A stream and a
/// whole document both settle it, so both call this.
#[must_use]
pub(crate) fn anthropic_stop_reason(
    finish_reason: Option<&str>,
    has_tool_use: bool,
) -> &'static str {
    // The upstream's own reason wins where it names one. A turn cut off at
    // the token limit mid tool-call is still a cutoff, and reporting
    // `tool_use` there hid it behind arguments that never finished
    // arriving.
    match finish_reason {
        Some("length") => "max_tokens",
        Some("content_filter") => "refusal",
        Some("tool_calls" | "function_call") => "tool_use",
        _ if has_tool_use => "tool_use",
        _ => "end_turn",
    }
}

/// One Messages turn as the one or more Chat Completions messages it
/// becomes. A turn holding a `tool_result` splits: this dialect keeps tool
/// results in messages of their own, after the turn that asked for them.
fn push_chat_messages(messages: &mut Vec<Value>, message: &Value) {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let content = message.get("content").unwrap_or(&Value::Null);
    let Some(blocks) = content.as_array() else {
        messages.push(json!({"role": role, "content": content.clone()}));
        return;
    };
    let mut parts = Vec::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => parts.push(json!({
                "type": "text",
                "text": block.get("text").cloned().unwrap_or_else(|| json!(""))
            })),
            Some("image") => {
                if let Some(image) = anthropic_image_to_chat(block) {
                    parts.push(image);
                }
            }
            Some("tool_use") => tool_calls.push(json!({
                "id": block.get("id").cloned().unwrap_or(Value::Null),
                "type": "function",
                "function": {
                    "name": block.get("name").cloned().unwrap_or(Value::Null),
                    "arguments": serde_json::to_string(block.get("input").unwrap_or(&json!({})))
                        .unwrap_or_else(|_| "{}".to_string())
                }
            })),
            // Held back, not written where it sits. A `tool` message must
            // follow the assistant turn that called for it with nothing in
            // between, and a Messages turn is free to put text before the
            // result. Flushing that text first wedged a `user` message
            // between the call and its answer, which strict gateways reject
            // with "'tool' must be a response to a preceding message with
            // 'tool_calls'".
            Some("tool_result") => {
                let content = block.get("content").unwrap_or(&Value::Null);
                tool_results.push(json!({
                    "role": "tool",
                    "tool_call_id": block.get("tool_use_id").cloned().unwrap_or(Value::Null),
                    "content": text_from_content(content)
                }));
                // A `tool` message in this dialect carries text and nothing
                // else, so an image result would vanish. It follows as a
                // user turn instead, which is where this dialect puts an
                // image the model must see.
                let images = value_array(Some(content))
                    .into_iter()
                    .filter(|part| part.get("type").and_then(Value::as_str) == Some("image"))
                    .filter_map(anthropic_image_to_chat)
                    .collect::<Vec<_>>();
                if !images.is_empty() {
                    tool_results.push(json!({"role": "user", "content": images}));
                }
            }
            // Thinking blocks are the model's own prior reasoning. This
            // dialect has nowhere to put them on the way up, and echoing
            // them back as text would change the transcript.
            _ => {}
        }
    }
    // Results first, then whatever else the turn carried.
    messages.append(&mut tool_results);
    if tool_calls.is_empty() {
        flush_chat_parts(messages, role, &mut parts);
    } else {
        let text = chat_parts_text(&parts);
        let mut assistant = json!({"role": "assistant"});
        // Null, not an empty string: several gateways reject `content: ""`
        // alongside `tool_calls`.
        assistant["content"] = if text.is_empty() {
            Value::Null
        } else {
            Value::String(text)
        };
        assistant["tool_calls"] = Value::Array(tool_calls);
        messages.push(assistant);
    }
}

/// One turn's parts as this dialect prefers them: a bare string when it is
/// only text, an array when an image is among it. A gateway that accepts
/// only the older string form therefore still sees a string for the text
/// turns, which is most of them.
fn flush_chat_parts(messages: &mut Vec<Value>, role: &str, parts: &mut Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    let only_text = parts
        .iter()
        .all(|part| part.get("type").and_then(Value::as_str) == Some("text"));
    let content = if only_text {
        Value::String(chat_parts_text(parts))
    } else {
        Value::Array(parts.clone())
    };
    messages.push(json!({"role": role, "content": content}));
    parts.clear();
}

fn chat_parts_text(parts: &[Value]) -> String {
    parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

/// An anthropic image block as a Chat Completions image part. A base64
/// source becomes the data URL this dialect carries images in.
fn anthropic_image_to_chat(block: &Value) -> Option<Value> {
    let source = block.get("source")?;
    let url = if source.get("type").and_then(Value::as_str) == Some("url") {
        source.get("url")?.as_str()?.to_string()
    } else {
        format!(
            "data:{};base64,{}",
            source
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png"),
            source.get("data").and_then(Value::as_str)?
        )
    };
    Some(json!({"type": "image_url", "image_url": {"url": url}}))
}

/// One anthropic tool as a Chat Completions function.
///
/// Only the web-search tool is executed by the vendor (`masquerade.rs`
/// leaves it alone for exactly that reason), and a configured endpoint
/// cannot run it, so that one is dropped rather than offered as a function
/// calling into nothing. `bash`, `computer` and `text_editor` are the
/// client's own tools whatever their `type` says — dropping those silently
/// took away tools the harness had offered.
fn anthropic_tool_to_chat(tool: &Value) -> Option<Value> {
    if tool
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.starts_with("web_search"))
    {
        return None;
    }
    let mut function = Map::new();
    function.insert("name".to_string(), tool.get("name")?.clone());
    insert_if_some(
        &mut function,
        "description",
        tool.get("description").cloned(),
    );
    function.insert(
        "parameters".to_string(),
        tool.get("input_schema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
    );
    Some(json!({"type": "function", "function": Value::Object(function)}))
}

fn chat_tool_choice_from_anthropic(choice: &Value) -> Value {
    match choice.get("type").and_then(Value::as_str) {
        Some("any") => Value::String("required".to_string()),
        Some("none") => Value::String("none".to_string()),
        Some("tool") => json!({
            "type": "function",
            "function": {"name": choice.get("name").cloned().unwrap_or(Value::Null)}
        }),
        _ => Value::String("auto".to_string()),
    }
}

fn thinking_from_effort(effort: Option<&str>, summary: Option<&str>) -> Option<Value> {
    let effort = effort?;
    let budget = match effort {
        "low" => 4_096,
        "high" => 24_576,
        _ => 8_192,
    };
    let mut thinking = json!({"type": "enabled", "budget_tokens": budget});
    if summary.is_some() {
        thinking["display"] = json!("summarized");
    }
    Some(thinking)
}

fn effort_from_budget(budget: Option<i64>) -> &'static str {
    let budget = budget.unwrap_or_default();
    if budget > 8192 {
        "high"
    } else if budget > 0 && budget <= 4096 {
        "low"
    } else {
        "medium"
    }
}

fn text_from_content(content: &Value) -> String {
    match content {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Array(items) => items.iter().map(text_from_content).collect(),
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                text.to_string()
            } else if matches!(
                object.get("type").and_then(Value::as_str),
                Some("input_text" | "output_text")
            ) {
                object
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            } else if object.get("type").and_then(Value::as_str) == Some("tool_result") {
                object
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            } else {
                String::new()
            }
        }
        other => other.to_string(),
    }
}

fn openai_content_to_anthropic(content: &Value) -> Value {
    match content {
        Value::String(_) | Value::Null => Value::String(text_from_content(content)),
        Value::Array(items) => {
            let mut blocks = Vec::new();
            for item in items {
                if let Some(text) = item.as_str() {
                    blocks.push(json!({"type": "text", "text": text}));
                    continue;
                }
                let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                    blocks.push(json!({"type": "text", "text": item.to_string()}));
                    continue;
                };
                match item_type {
                    "text" | "input_text" | "output_text" => {
                        blocks.push(json!({"type": "text", "text": item.get("text").cloned().unwrap_or_else(|| json!(""))}));
                    }
                    "image_url" | "input_image" => {
                        if let Some(image) =
                            image_url_to_anthropic(&image_url_value(item.get("image_url")))
                        {
                            blocks.push(image);
                        }
                    }
                    _ => {}
                }
            }
            if blocks.is_empty() {
                Value::String(String::new())
            } else {
                Value::Array(blocks)
            }
        }
        other => Value::String(other.to_string()),
    }
}

/// Convert `OpenAI` Chat content parts to their Responses equivalents.
///
/// Chat says `text`/`image_url` with a nested `{url}`; Responses wants
/// `input_text`/`input_image` with a flat url string. A plain string passes
/// through, since both APIs accept one.
fn chat_content_to_responses(content: Option<&Value>) -> Value {
    let Some(parts) = content.and_then(Value::as_array) else {
        return content.cloned().unwrap_or_else(|| json!(""));
    };
    let converted: Vec<Value> = parts
        .iter()
        .map(|part| match part.get("type").and_then(Value::as_str) {
            Some("image_url" | "input_image") => json!({
                "type": "input_image",
                "image_url": image_url_value(part.get("image_url"))
            }),
            Some("text" | "input_text" | "output_text") => json!({
                "type": "input_text",
                "text": part.get("text").cloned().unwrap_or_else(|| json!(""))
            }),
            _ => part.clone(),
        })
        .collect();
    Value::Array(converted)
}

fn image_url_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::Object(object)) => object
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some(Value::String(value)) => value.clone(),
        _ => String::new(),
    }
}

fn image_url_to_anthropic(url: &str) -> Option<Value> {
    if url.is_empty() {
        return None;
    }
    if let Some((media_type, data)) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(";base64,"))
    {
        return Some(json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": data}
        }));
    }
    Some(json!({"type": "image", "source": {"type": "url", "url": url}}))
}

fn anthropic_image_to_responses(block: &Value) -> Option<Value> {
    let source = block.get("source")?;
    match source.get("type").and_then(Value::as_str)? {
        "url" => Some(json!({"type": "input_image", "image_url": source.get("url")?.as_str()?})),
        "base64" => Some(json!({
            "type": "input_image",
            "image_url": format!(
                "data:{};base64,{}",
                source.get("media_type").and_then(Value::as_str).unwrap_or("image/png"),
                source.get("data")?.as_str()?
            )
        })),
        _ => None,
    }
}

fn anthropic_system_from_messages(messages: &[&Value]) -> Vec<Value> {
    messages
        .iter()
        .filter(|message| {
            matches!(
                message.get("role").and_then(Value::as_str),
                Some("system" | "developer")
            )
        })
        .filter_map(|message| {
            let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
            (!text.is_empty()).then(|| json!({"type": "text", "text": text}))
        })
        .collect()
}

fn openai_tool_to_anthropic(tool: &Value) -> Option<Value> {
    let tool_type = tool.get("type").and_then(Value::as_str);
    if matches!(tool_type, Some("web_search" | "web_search_preview"))
        || tool_type.is_some_and(|value| ANTHROPIC_WEB_SEARCH_TOOL_TYPES.contains(&value))
    {
        return Some(anthropic_web_search_tool(tool));
    }
    if tool_type == Some("function") && tool.get("name").is_some() {
        return Some(json!({
            "name": tool.get("name").cloned().unwrap_or(Value::Null),
            "description": tool.get("description").cloned().unwrap_or_else(|| json!("")),
            "input_schema": tool.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"}))
        }));
    }
    let function = tool.get("function")?;
    Some(json!({
        "name": function.get("name").cloned().unwrap_or(Value::Null),
        "description": function.get("description").cloned().unwrap_or_else(|| json!("")),
        "input_schema": function.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"}))
    }))
}

fn anthropic_web_search_tool(tool: &Value) -> Value {
    let tool_type = tool
        .get("type")
        .and_then(Value::as_str)
        .filter(|value| ANTHROPIC_WEB_SEARCH_TOOL_TYPES.contains(value))
        .unwrap_or(ANTHROPIC_WEB_SEARCH_TOOL_TYPE);
    let mut out = json!({
        "type": tool_type,
        "name": tool.get("name").and_then(Value::as_str).unwrap_or("web_search")
    });
    copy_value_field(tool, &mut out, "max_uses");
    let filters = tool.get("filters").unwrap_or(&Value::Null);
    if let Some(value) = tool
        .get("allowed_domains")
        .or_else(|| filters.get("allowed_domains"))
    {
        out["allowed_domains"] = value.clone();
    }
    if let Some(value) = tool
        .get("blocked_domains")
        .or_else(|| filters.get("blocked_domains"))
    {
        out["blocked_domains"] = value.clone();
    }
    copy_value_field(tool, &mut out, "user_location");
    out
}

fn responses_tool_from_chat_tool(tool: &Value) -> Option<Value> {
    let tool_type = tool.get("type").and_then(Value::as_str);
    if tool_type.is_some_and(|tool_type| tool_type != "function") {
        return Some(tool.clone());
    }
    let function = tool.get("function")?;
    function.get("name")?;
    let mut out = json!({
        "type": "function",
        "name": function.get("name").cloned().unwrap_or(Value::Null),
        "description": function.get("description").cloned().unwrap_or_else(|| json!("")),
        "parameters": function.get("parameters").cloned().unwrap_or_else(|| json!({"type": "object"}))
    });
    copy_value_field(function, &mut out, "strict");
    Some(out)
}

fn anthropic_tool_to_responses(tool: &Value) -> Value {
    if tool
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| ANTHROPIC_WEB_SEARCH_TOOL_TYPES.contains(&value))
    {
        let mut out = json!({"type": "web_search"});
        let mut filters = Map::new();
        if let Some(value) = tool.get("allowed_domains") {
            filters.insert("allowed_domains".to_string(), value.clone());
        }
        if let Some(value) = tool.get("blocked_domains") {
            filters.insert("blocked_domains".to_string(), value.clone());
        }
        if !filters.is_empty() {
            out["filters"] = Value::Object(filters);
        }
        copy_value_field(tool, &mut out, "user_location");
        return out;
    }
    json!({
        "type": "function",
        "name": tool.get("name").cloned().unwrap_or(Value::Null),
        "description": tool.get("description").cloned().unwrap_or_else(|| json!("")),
        "parameters": tool.get("input_schema").cloned().unwrap_or_else(|| json!({"type": "object"}))
    })
}

fn anthropic_tool_choice(body: &Value, has_tools: bool) -> Option<Value> {
    if !has_tools {
        return None;
    }
    if body.get("tool_choice").is_none()
        && body.get("parallel_tool_calls").and_then(Value::as_bool) != Some(false)
    {
        return None;
    }
    let default_choice = json!("auto");
    let choice = body.get("tool_choice").unwrap_or(&default_choice);
    let mut out = match choice {
        Value::String(value) if value == "auto" => json!({"type": "auto"}),
        Value::String(value) if value == "required" => json!({"type": "any"}),
        Value::String(value) if value == "none" => json!({"type": "none"}),
        Value::Object(_) => match choice.get("type").and_then(Value::as_str) {
            Some("web_search" | "web_search_preview") => {
                json!({"type": "tool", "name": "web_search"})
            }
            Some("function") => json!({
                "type": "tool",
                "name": choice.get("name")
                    .or_else(|| choice.get("function").and_then(|function| function.get("name")))
                    .cloned()
                    .unwrap_or(Value::Null)
            }),
            Some("auto" | "any" | "none" | "tool") => choice.clone(),
            _ => json!({"type": "auto"}),
        },
        _ => json!({"type": "auto"}),
    };
    if body.get("parallel_tool_calls").and_then(Value::as_bool) == Some(false) {
        out["disable_parallel_tool_use"] = Value::Bool(true);
    }
    Some(out)
}

fn responses_tool_choice_from_chat(choice: &Value) -> Value {
    if choice.get("type").and_then(Value::as_str) == Some("function") {
        return json!({
            "type": "function",
            "name": choice.get("name")
                .or_else(|| choice.get("function").and_then(|function| function.get("name")))
                .cloned()
                .unwrap_or(Value::Null)
        });
    }
    choice.clone()
}

fn responses_tool_choice_from_anthropic(choice: &Value) -> Value {
    match choice.get("type").and_then(Value::as_str) {
        Some("auto") => json!("auto"),
        Some("any") => json!("required"),
        Some("none") => json!("none"),
        Some("tool") if choice.get("name").and_then(Value::as_str) == Some("web_search") => {
            json!({"type": "web_search"})
        }
        Some("tool") => {
            json!({"type": "function", "name": choice.get("name").cloned().unwrap_or(Value::Null)})
        }
        _ => choice.clone(),
    }
}

fn responses_input_to_openai_messages(input_items: &Value) -> Vec<Value> {
    if let Some(input) = input_items.as_str() {
        return vec![json!({"role": "user", "content": input})];
    }
    let mut messages = Vec::new();
    for item in value_array(Some(input_items)) {
        if item.get("role").is_some() {
            messages.push(item.clone());
            continue;
        }
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => messages.push(json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": item.get("call_id").cloned().unwrap_or(Value::Null),
                    "type": "function",
                    "function": {
                        "name": item.get("name").cloned().unwrap_or(Value::Null),
                        "arguments": item.get("arguments").cloned().unwrap_or_else(|| json!("{}"))
                    }
                }]
            })),
            Some("function_call_output") => messages.push(json!({
                "role": "tool",
                "tool_call_id": item.get("call_id").cloned().unwrap_or(Value::Null),
                "content": item.get("output").cloned().unwrap_or_else(|| json!(""))
            })),
            // reasoning, web_search_call and friends have no chat equivalent.
            // Serializing them into a user turn attributes machine JSON to the
            // user and inflates the prompt, so they are dropped.
            _ => {}
        }
    }
    messages
}

fn anthropic_citations_to_openai(citations: Option<&Vec<Value>>) -> Vec<Value> {
    citations
        .into_iter()
        .flatten()
        .filter(|citation| {
            citation.get("type").and_then(Value::as_str) == Some("web_search_result_location")
        })
        .map(|citation| {
            let mut out = json!({
                "type": "url_citation",
                "url": citation.get("url").cloned().unwrap_or(Value::Null),
                "title": citation.get("title").cloned().unwrap_or(Value::Null)
            });
            copy_value_field(citation, &mut out, "start_index");
            copy_value_field(citation, &mut out, "end_index");
            out
        })
        .collect()
}

fn openai_annotations_to_anthropic(annotations: Option<&Vec<Value>>) -> Vec<Value> {
    annotations
        .into_iter()
        .flatten()
        .filter(|annotation| annotation.get("type").and_then(Value::as_str) == Some("url_citation"))
        .map(|annotation| {
            let mut out = json!({
                "type": "web_search_result_location",
                "url": annotation.get("url").cloned().unwrap_or(Value::Null),
                "title": annotation.get("title").cloned().unwrap_or(Value::Null)
            });
            copy_value_field(annotation, &mut out, "start_index");
            copy_value_field(annotation, &mut out, "end_index");
            out
        })
        .collect()
}

fn web_search_query_from_action(action: &Value) -> String {
    if let Some(query) = action.get("query").and_then(Value::as_str) {
        return query.to_string();
    }
    value_array(action.get("queries"))
        .into_iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

fn finish_reason(stop_reason: Option<&str>, has_tool_calls: bool) -> &'static str {
    if has_tool_calls || stop_reason == Some("tool_use") {
        "tool_calls"
    } else if stop_reason == Some("max_tokens") {
        "length"
    } else {
        "stop"
    }
}

fn usage_to_openai(usage: &Value) -> Value {
    let prompt = int_field(usage, "input_tokens");
    let completion = int_field(usage, "output_tokens");
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
        "prompt_tokens_details": {"cached_tokens": int_field(usage, "cache_read_input_tokens")},
        "completion_tokens_details": {
            "reasoning_tokens": usage
                .get("output_tokens_details")
                .map_or(0, |details| int_field(details, "reasoning_tokens"))
        }
    })
}

fn flush_response_text(
    output: &mut Vec<Value>,
    output_text_parts: &mut Vec<String>,
    text: &mut String,
    annotations: &mut Vec<Value>,
) {
    if text.is_empty() {
        return;
    }
    output_text_parts.push(text.clone());
    let mut content = json!({"type": "output_text", "text": text});
    if !annotations.is_empty() {
        content["annotations"] = Value::Array(std::mem::take(annotations));
    }
    output.push(json!({"type": "message", "role": "assistant", "content": [content]}));
    text.clear();
}

fn flush_input_parts(input: &mut Vec<Value>, role: &Value, parts: &mut Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    let content = if parts
        .iter()
        .all(|part| part.get("type").and_then(Value::as_str) == Some("input_text"))
    {
        Value::String(
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect(),
        )
    } else {
        Value::Array(std::mem::take(parts))
    };
    input.push(json!({"role": role, "content": content}));
    parts.clear();
}

fn copy_if_present(source: &Value, target: &mut Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key) {
        target.insert(key.to_string(), value.clone());
    }
}

fn insert_if_some(target: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        target.insert(key.to_string(), value);
    }
}

fn copy_value_field(source: &Value, target: &mut Value, key: &str) {
    if let Some(value) = source.get(key) {
        target[key] = value.clone();
    }
}

// -- Antigravity: Chat Completions <-> Gemini ----------------------------

/// The Gemini `GenerateContentRequest` a Chat Completions body becomes — the
/// inner `request` of the Antigravity envelope. Chat Completions is the
/// relay's pivot, so a Messages or Responses client reaches this through the
/// existing translators rather than through a second matrix.
#[must_use]
pub fn antigravity_request(chat: &Value) -> Value {
    let mut system = Vec::new();
    let mut contents: Vec<Value> = Vec::new();
    let mut tool_names: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();

    for message in value_array(chat.get("messages")) {
        match message.get("role").and_then(Value::as_str) {
            Some("system" | "developer") => {
                let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
                if !text.is_empty() {
                    system.push(text);
                }
            }
            Some("assistant") => {
                let parts = gemini_model_parts(message, &mut tool_names);
                push_gemini_content(&mut contents, "model", parts);
            }
            Some("tool") => {
                let part = gemini_function_response(message, &tool_names);
                push_gemini_content(&mut contents, "user", vec![part]);
            }
            _ => {
                let parts =
                    gemini_parts_from_content(message.get("content").unwrap_or(&Value::Null));
                push_gemini_content(&mut contents, "user", parts);
            }
        }
    }

    let mut out = Map::new();
    out.insert("contents".to_string(), Value::Array(contents));
    if !system.is_empty() {
        // Must be an object with parts; a bare string is a 400 upstream.
        out.insert(
            "systemInstruction".to_string(),
            json!({"parts": [{"text": system.join("\n\n")}]}),
        );
    }
    let generation = gemini_generation_config(chat);
    if !generation.is_empty() {
        out.insert("generationConfig".to_string(), Value::Object(generation));
    }
    let tools = value_array(chat.get("tools"))
        .into_iter()
        .filter_map(gemini_function_declaration)
        .collect::<Vec<_>>();
    if !tools.is_empty() {
        out.insert(
            "tools".to_string(),
            json!([{"functionDeclarations": tools}]),
        );
        if let Some(config) = gemini_tool_config(chat.get("tool_choice"), &tools) {
            out.insert("toolConfig".to_string(), config);
        }
    }
    Value::Object(out)
}

/// One assistant message as the parts of a Gemini model turn: returned
/// reasoning as a thought, the text, and each call with the vendor's id and its
/// signature. Each call's name is remembered under its id for the results.
fn gemini_model_parts(
    message: &Value,
    tool_names: &mut std::collections::BTreeMap<String, String>,
) -> Vec<Value> {
    let mut parts = Vec::new();
    // Reasoning the client sends back rides in front of the answer, flagged as
    // a thought so the vendor does not read it as text.
    let reasoning = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !reasoning.is_empty() {
        parts.push(json!({"text": reasoning, "thought": true}));
    }
    let text = text_from_content(message.get("content").unwrap_or(&Value::Null));
    if !text.is_empty() {
        parts.push(json!({"text": text}));
    }
    for (position, call) in value_array(message.get("tool_calls"))
        .into_iter()
        .enumerate()
    {
        let function = call.get("function").unwrap_or(&Value::Null);
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (id, signature) = call
            .get("id")
            .and_then(Value::as_str)
            .map(decode_tool_call_id)
            .unwrap_or_default();
        tool_names.insert(id.clone(), name.to_string());
        let mut function_call = json!({
            "name": name,
            "args": gemini_tool_arguments(function.get("arguments")),
        });
        if !id.is_empty() {
            function_call["id"] = json!(id);
        }
        let mut part = json!({"functionCall": function_call});
        // Gemini refuses a model turn whose first call carries no signature. A
        // history with none to replay gets the validator-skip sentinel Google
        // documents for exactly that case, which Cloud Code accepts for Gemini
        // and Claude alike.
        match signature {
            Some(signature) => part["thoughtSignature"] = json!(signature),
            None if position == 0 => {
                part["thoughtSignature"] = json!(SKIP_THOUGHT_SIGNATURE);
            }
            None => {}
        }
        parts.push(part);
    }
    parts
}

/// One tool result as a Gemini `functionResponse`, named and identified by the
/// call it answers.
fn gemini_function_response(
    message: &Value,
    tool_names: &std::collections::BTreeMap<String, String>,
) -> Value {
    let id = message
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map(|id| decode_tool_call_id(id).0)
        .unwrap_or_default();
    let name = tool_names
        .get(&id)
        .cloned()
        .or_else(|| {
            message
                .get("name")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default();
    let content = text_from_content(message.get("content").unwrap_or(&Value::Null));
    let mut response = json!({"name": name, "response": {"content": content}});
    // Claude models behind Cloud Code refuse a result without the id of the
    // call it answers.
    if !id.is_empty() {
        response["id"] = json!(id);
    }
    json!({"functionResponse": response})
}

/// Append parts to the conversation, merging into the previous turn when the
/// role repeats: Gemini reads one `contents` entry per turn, and a Messages
/// client splits a tool result into its own message.
fn push_gemini_content(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(existing) = contents
        .last_mut()
        .filter(|last| last.get("role").and_then(Value::as_str) == Some(role))
        .and_then(|last| last.get_mut("parts"))
        .and_then(Value::as_array_mut)
    {
        existing.extend(parts);
        return;
    }
    contents.push(json!({"role": role, "parts": parts}));
}

/// Chat message content as Gemini parts: text, and an image URL as either
/// `inlineData` (a data URL) or `fileData` (a hosted one).
fn gemini_parts_from_content(content: &Value) -> Vec<Value> {
    match content {
        Value::String(text) => {
            if text.is_empty() {
                Vec::new()
            } else {
                vec![json!({"text": text})]
            }
        }
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({"text": text})),
                Some("image_url") => {
                    let url = item
                        .get("image_url")
                        .and_then(|image| image.get("url"))
                        .or_else(|| item.get("image_url"))
                        .and_then(Value::as_str)?;
                    Some(gemini_image_part(url))
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn gemini_image_part(url: &str) -> Value {
    use base64::Engine as _;
    // `data:<mime>[;base64],<payload>` travels inline; a payload that is not
    // base64 is percent-decoded and encoded, since `inlineData` is base64 only.
    if let Some((meta, data)) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
    {
        return match meta.strip_suffix(";base64") {
            Some(mime) => json!({"inlineData": {"mimeType": mime, "data": data}}),
            None => json!({"inlineData": {
                "mimeType": meta,
                "data": base64::engine::general_purpose::STANDARD.encode(percent_decode(data))
            }}),
        };
    }
    // A hosted image is handed over as a URI; `fileData` needs its MIME type,
    // which the extension names for the formats the upstream reads.
    json!({"fileData": {"fileUri": url, "mimeType": image_mime_from_url(url)}})
}

fn image_mime_from_url(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        // JPEG is the common case and the upstream's own default for photos.
        _ => "image/jpeg",
    }
}

/// `%XX` escapes decoded to bytes; anything malformed stays as written.
fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn gemini_tool_arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(raw)) => serde_json::from_str(raw).unwrap_or_else(|_| json!({})),
        Some(value) if value.is_object() => value.clone(),
        _ => json!({}),
    }
}

fn gemini_generation_config(chat: &Value) -> Map<String, Value> {
    let mut config = Map::new();
    let budget = chat
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .and_then(gemini_thinking_budget);
    let max_tokens = chat
        .get("max_completion_tokens")
        .or_else(|| chat.get("max_tokens"))
        .and_then(Value::as_i64);
    if let Some(budget) = budget {
        config.insert(
            "thinkingConfig".to_string(),
            json!({"thinkingBudget": budget, "includeThoughts": true}),
        );
    }
    // The upstream refuses `maxOutputTokens <= thinkingBudget`, so a client cap
    // below the budget is raised past it; no cap is invented where the client
    // set none.
    if let Some(max_tokens) = max_tokens {
        let limit = budget.map_or(max_tokens, |budget| max_tokens.max(budget + 1024));
        config.insert("maxOutputTokens".to_string(), json!(limit));
    }
    for (from, to) in [
        ("temperature", "temperature"),
        ("top_p", "topP"),
        ("top_k", "topK"),
    ] {
        if let Some(value) = chat.get(from).filter(|value| value.is_number()) {
            config.insert(to.to_string(), value.clone());
        }
    }
    if let Some(stop) = chat.get("stop") {
        let sequences = match stop {
            Value::String(one) => vec![Value::String(one.clone())],
            Value::Array(items) => items.clone(),
            _ => Vec::new(),
        };
        if !sequences.is_empty() {
            config.insert("stopSequences".to_string(), Value::Array(sequences));
        }
    }
    config
}

/// The thinking budget an effort names; `none` asks for no thinking at all.
fn gemini_thinking_budget(effort: &str) -> Option<i64> {
    match effort {
        "none" => None,
        "minimal" => Some(1_024),
        "low" => Some(4_096),
        "high" => Some(24_576),
        "xhigh" => Some(32_768),
        _ => Some(8_192),
    }
}

fn gemini_function_declaration(tool: &Value) -> Option<Value> {
    let function = tool.get("function").unwrap_or(tool);
    let name = function.get("name").and_then(Value::as_str)?;
    let mut out = Map::new();
    out.insert("name".to_string(), json!(name));
    if let Some(description) = function.get("description").and_then(Value::as_str) {
        out.insert("description".to_string(), json!(description));
    }
    if let Some(parameters) = function.get("parameters").filter(|value| value.is_object()) {
        out.insert("parameters".to_string(), gemini_schema(parameters));
    }
    Some(Value::Object(out))
}

/// The fields of the Gemini `Schema` proto, the only ones Cloud Code accepts in
/// a function declaration: any other key is `400 Cannot find field`.
const GEMINI_SCHEMA_FIELDS: [&str; 21] = [
    "type",
    "format",
    "title",
    "description",
    "nullable",
    "enum",
    "maxItems",
    "minItems",
    "properties",
    "required",
    "minProperties",
    "maxProperties",
    "minLength",
    "maxLength",
    "pattern",
    "example",
    "anyOf",
    "propertyOrdering",
    "items",
    "minimum",
    "maximum",
];

/// How deep `$ref`s are inlined; a recursive definition ends here as an open
/// object rather than expanding forever.
const GEMINI_REF_DEPTH: usize = 8;

/// A JSON Schema as the Gemini `Schema` proto accepts it. Local `$ref`s are
/// inlined from `$defs`/`definitions`; a `type` array becomes one type plus
/// `nullable`; an `anyOf`/`oneOf` branch of `null` becomes `nullable`; `const`
/// and non-string enums become string enums; exclusive bounds become inclusive
/// ones; and every key the proto lacks is dropped.
fn gemini_schema(schema: &Value) -> Value {
    gemini_schema_node(schema, schema, 0)
}

fn gemini_schema_node(node: &Value, root: &Value, depth: usize) -> Value {
    let Some(map) = node.as_object() else {
        return json!({});
    };
    // A local reference stands for its target; sibling keys (a description, a
    // default) add to it.
    if let Some(reference) = map.get("$ref").and_then(Value::as_str) {
        let target = (depth < GEMINI_REF_DEPTH)
            .then(|| resolve_local_ref(root, reference))
            .flatten();
        let mut merged = target
            .and_then(Value::as_object)
            .cloned()
            // Unresolvable or too deep: an open object, never a dangling ref.
            .unwrap_or_else(|| Map::from_iter([("type".to_string(), json!("object"))]));
        for (key, value) in map {
            if key != "$ref" {
                merged.insert(key.clone(), value.clone());
            }
        }
        return gemini_schema_node(&Value::Object(merged), root, depth + 1);
    }

    let mut out = Map::new();
    let mut nullable = map.get("nullable").and_then(Value::as_bool) == Some(true);
    let mut types = schema_types(map, &mut nullable);
    fold_schema_branches(map, root, depth, &mut out, &mut nullable);
    if schema_string_enum(map, &mut out, &mut nullable) {
        types = vec!["string".to_string()];
    }
    match types.as_slice() {
        [] => {}
        [one] => {
            out.insert("type".to_string(), json!(one));
        }
        // Several real types: one branch each.
        many => {
            let branches = many.iter().map(|kind| json!({"type": kind})).collect();
            out.insert("anyOf".to_string(), Value::Array(branches));
        }
    }
    // An exclusive bound becomes the inclusive one; for the integers coding
    // tools bound, that is off by one in the permissive direction only.
    for (exclusive, inclusive) in [
        ("exclusiveMinimum", "minimum"),
        ("exclusiveMaximum", "maximum"),
    ] {
        if let Some(bound) = map.get(exclusive).filter(|bound| bound.is_number()) {
            out.entry(inclusive.to_string())
                .or_insert_with(|| bound.clone());
        }
    }
    copy_schema_fields(map, root, depth, &mut out);
    if nullable {
        out.insert("nullable".to_string(), json!(true));
    }
    Value::Object(out)
}

/// The node's real types; a `null` among them (`["boolean", "null"]`) marks it
/// nullable instead.
fn schema_types(map: &Map<String, Value>, nullable: &mut bool) -> Vec<String> {
    let mut types: Vec<String> = match map.get("type") {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    };
    if types.iter().any(|kind| kind == "null") {
        *nullable = true;
        types.retain(|kind| kind != "null");
    }
    types
}

/// `anyOf`/`oneOf` keep their non-null branches, a lone branch folded into the
/// node (pydantic's `Optional[X]`); `allOf` branches merge into it.
fn fold_schema_branches(
    map: &Map<String, Value>,
    root: &Value,
    depth: usize,
    out: &mut Map<String, Value>,
    nullable: &mut bool,
) {
    let mut branches = Vec::new();
    for key in ["anyOf", "oneOf"] {
        for branch in value_array(map.get(key)) {
            if branch.get("type").and_then(Value::as_str) == Some("null") {
                *nullable = true;
            } else {
                branches.push(gemini_schema_node(branch, root, depth + 1));
            }
        }
    }
    if branches.len() == 1 {
        if let Some(Value::Object(only)) = branches.pop() {
            for (key, value) in only {
                out.entry(key).or_insert(value);
            }
        }
    } else if !branches.is_empty() {
        out.insert("anyOf".to_string(), Value::Array(branches));
    }
    for branch in value_array(map.get("allOf")) {
        if let Value::Object(part) = gemini_schema_node(branch, root, depth + 1) {
            for (key, value) in part {
                merge_schema_key(out, key, value);
            }
        }
    }
}

/// `const` and `enum` as the proto's string-only enum. Returns whether the
/// node now holds one, which makes its type `string`.
fn schema_string_enum(
    map: &Map<String, Value>,
    out: &mut Map<String, Value>,
    nullable: &mut bool,
) -> bool {
    let mut values: Vec<Value> = value_array(map.get("enum")).into_iter().cloned().collect();
    if let Some(constant) = map.get("const") {
        values.push(constant.clone());
    }
    if values.is_empty() {
        return false;
    }
    if values.iter().any(Value::is_null) {
        *nullable = true;
    }
    let strings = values
        .iter()
        .filter(|value| !value.is_null())
        .map(|value| match value {
            Value::String(text) => Value::String(text.clone()),
            other => Value::String(other.to_string()),
        })
        .collect();
    out.insert("enum".to_string(), Value::Array(strings));
    true
}

/// The proto fields the node carries as they are, its subschemas cleaned, and
/// every other key dropped.
fn copy_schema_fields(
    map: &Map<String, Value>,
    root: &Value,
    depth: usize,
    out: &mut Map<String, Value>,
) {
    for (key, value) in map {
        if out.contains_key(key) || !GEMINI_SCHEMA_FIELDS.contains(&key.as_str()) {
            continue;
        }
        match key.as_str() {
            "type" | "enum" | "anyOf" | "nullable" => {}
            "properties" => {
                let properties = value
                    .as_object()
                    .map(|entries| {
                        entries
                            .iter()
                            .map(|(name, sub)| {
                                (name.clone(), gemini_schema_node(sub, root, depth + 1))
                            })
                            .collect::<Map<String, Value>>()
                    })
                    .unwrap_or_default();
                out.insert(key.clone(), Value::Object(properties));
            }
            "items" => {
                // A tuple (`items: [..]`) keeps its first shape.
                let item = match value {
                    Value::Array(tuple) => tuple.first().cloned().unwrap_or_else(|| json!({})),
                    other => other.clone(),
                };
                out.insert(key.clone(), gemini_schema_node(&item, root, depth + 1));
            }
            // The proto names a handful of formats; `uri`, `email` and the
            // rest are dropped rather than refused.
            "format" => {
                if matches!(
                    value.as_str(),
                    Some("enum" | "date-time" | "int32" | "int64" | "float" | "double")
                ) {
                    out.insert(key.clone(), value.clone());
                }
            }
            "required" | "propertyOrdering" => {
                if value
                    .as_array()
                    .is_some_and(|names| names.iter().all(Value::is_string))
                {
                    out.insert(key.clone(), value.clone());
                }
            }
            "title" | "description" | "pattern" => {
                if value.is_string() {
                    out.insert(key.clone(), value.clone());
                }
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
}

/// `#/$defs/Name` or `#/definitions/Name`, read from the schema's root.
fn resolve_local_ref<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let path = reference.strip_prefix("#/")?;
    path.split('/').try_fold(root, |node, segment| {
        node.get(segment.replace("~1", "/").replace("~0", "~"))
    })
}

/// One key of an `allOf` branch folded into its parent: properties and
/// `required` union, anything else is kept where the parent has none.
fn merge_schema_key(out: &mut Map<String, Value>, key: String, value: Value) {
    match (out.get_mut(&key), value) {
        (Some(Value::Object(existing)), Value::Object(more)) if key == "properties" => {
            existing.extend(more);
        }
        (Some(Value::Array(existing)), Value::Array(more)) if key == "required" => {
            for name in more {
                if !existing.contains(&name) {
                    existing.push(name);
                }
            }
        }
        (Some(_), _) => {}
        (None, value) => {
            out.insert(key, value);
        }
    }
}

fn gemini_tool_config(choice: Option<&Value>, declarations: &[Value]) -> Option<Value> {
    let mode = match choice {
        None => return None,
        Some(Value::String(mode)) => mode.clone(),
        Some(object) => {
            let function = object.get("function").unwrap_or(object);
            let name = function.get("name").and_then(Value::as_str)?;
            return Some(json!({"functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": [name]
            }}));
        }
    };
    let mode = match mode.as_str() {
        "auto" => "AUTO",
        "none" => "NONE",
        "required" | "any" => "ANY",
        _ => return None,
    };
    // `ANY` without a name is the upstream's "must call something"; a client
    // that named nothing is served by the declared names.
    let names = declarations
        .iter()
        .filter_map(|declaration| declaration.get("name").cloned())
        .collect::<Vec<_>>();
    let config = if mode == "ANY" && !names.is_empty() {
        json!({"mode": mode, "allowedFunctionNames": names})
    } else {
        json!({"mode": mode})
    };
    Some(json!({"functionCallingConfig": config}))
}

/// A Gemini `GenerateContentResponse` as an `OpenAI` Chat Completions reply.
#[must_use]
pub fn antigravity_response(gemini: &Value, model: &str) -> Value {
    let (text, reasoning, tool_calls) = gemini_parts(gemini);
    let mut message = Map::new();
    message.insert("role".to_string(), json!("assistant"));
    message.insert("content".to_string(), json!(text));
    if !reasoning.is_empty() {
        message.insert("reasoning_content".to_string(), json!(reasoning));
    }
    let has_tool_calls = !tool_calls.is_empty();
    if has_tool_calls {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    let mut out = json!({
        "id": gemini_id(gemini),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": gemini_finish_reason(gemini, has_tool_calls)
        }]
    });
    if let Some(usage) = gemini_usage(gemini.get("usageMetadata")) {
        out["usage"] = usage;
    }
    out
}

/// One candidate's parts split three ways: answer text, thinking text, and
/// function calls in the Chat shape. Shared by the whole reply and the stream.
#[must_use]
pub fn gemini_parts(gemini: &Value) -> (String, String, Vec<Value>) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut tool_calls = Vec::new();
    for part in value_array(
        gemini
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
            .and_then(|candidate| candidate.get("content"))
            .and_then(|content| content.get("parts")),
    ) {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            if let Some(thought) = part.get("text").and_then(Value::as_str) {
                reasoning.push_str(thought);
            }
            continue;
        }
        if let Some(chunk) = part.get("text").and_then(Value::as_str) {
            text.push_str(chunk);
        }
        if let Some(call) = part.get("functionCall") {
            let signature = part.get("thoughtSignature").and_then(Value::as_str);
            tool_calls.push(gemini_tool_call(call, signature));
        }
    }
    (text, reasoning, tool_calls)
}

/// What Google documents for a model turn whose calls have no signature to
/// replay: the validator is skipped for that turn.
const SKIP_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

/// Separates the vendor's call id from the signature carried beside it. Ids
/// Cloud Code mints (`call_<digits>`, `toolu_vrtx_<alnum>`) never contain it.
const SIGNATURE_MARKER: &str = "__ags__";

/// The tool-call id handed to the client: the vendor's id, and the call's
/// thought signature when it has one, so the next turn can send the signature
/// back without the relay holding any state. A harness echoes the id verbatim
/// on the call and on its result; the signature is base64url so the id stays
/// inside `[A-Za-z0-9_-]`, which every dialect's tool ids accept.
fn encode_tool_call_id(id: &str, signature: Option<&str>) -> String {
    use base64::Engine as _;
    match signature.filter(|signature| !signature.is_empty()) {
        Some(signature) => format!(
            "{id}{SIGNATURE_MARKER}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature)
        ),
        None => id.to_string(),
    }
}

/// The inverse of [`encode_tool_call_id`]: the vendor's id and the signature,
/// if the id carried one that decodes. Any other id is returned whole.
fn decode_tool_call_id(id: &str) -> (String, Option<String>) {
    use base64::Engine as _;
    let Some((vendor, encoded)) = id.split_once(SIGNATURE_MARKER) else {
        return (id.to_string(), None);
    };
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    match signature {
        Some(signature) => (vendor.to_string(), Some(signature)),
        None => (id.to_string(), None),
    }
}

fn gemini_tool_call(call: &Value, signature: Option<&str>) -> Value {
    let name = call.get("name").and_then(Value::as_str).unwrap_or_default();
    let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
    let id = call.get("id").and_then(Value::as_str).map_or_else(
        || format!("call_{}", uuid::Uuid::new_v4().simple()),
        ToString::to_string,
    );
    let id = encode_tool_call_id(&id, signature);
    json!({
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string())
        }
    })
}

/// `usageMetadata` under the Chat names. Gemini reports thinking apart from
/// `candidatesTokenCount`; the Chat dialect counts it inside
/// `completion_tokens`, with `reasoning_tokens` as its breakdown, so a client
/// never sees reasoning larger than the output it belongs to.
#[must_use]
pub fn gemini_usage(usage: Option<&Value>) -> Option<Value> {
    let usage = usage?;
    if !usage.is_object() {
        return None;
    }
    let prompt = int_field(usage, "promptTokenCount");
    let completion = int_field(usage, "candidatesTokenCount");
    let reasoning = int_field(usage, "thoughtsTokenCount");
    let cached = int_field(usage, "cachedContentTokenCount");
    let total = usage
        .get("totalTokenCount")
        .and_then(Value::as_i64)
        .unwrap_or(prompt + completion + reasoning);
    let mut out = json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion + reasoning,
        "total_tokens": total
    });
    if reasoning > 0 {
        out["completion_tokens_details"] = json!({"reasoning_tokens": reasoning});
    }
    if cached > 0 {
        out["prompt_tokens_details"] = json!({"cached_tokens": cached});
    }
    Some(out)
}

#[must_use]
pub fn gemini_finish_reason(gemini: &Value, has_tool_calls: bool) -> &'static str {
    if gemini_prompt_blocked(gemini) {
        return "content_filter";
    }
    let reason = gemini
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .and_then(|candidate| candidate.get("finishReason"))
        .and_then(Value::as_str);
    match reason {
        Some("MAX_TOKENS") => "length",
        Some("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII") => {
            "content_filter"
        }
        // A call the model could not form is not a cutoff: reporting `length`
        // told a Messages client `max_tokens`, which invites a retry with more.
        Some("MALFORMED_FUNCTION_CALL") => "stop",
        _ if has_tool_calls => "tool_calls",
        _ => "stop",
    }
}

/// A prompt the upstream refused outright: `promptFeedback.blockReason` and
/// no candidate at all.
#[must_use]
pub fn gemini_prompt_blocked(gemini: &Value) -> bool {
    gemini
        .get("promptFeedback")
        .and_then(|feedback| feedback.get("blockReason"))
        .is_some_and(|reason| !reason.is_null())
}

fn gemini_id(gemini: &Value) -> String {
    gemini
        .get("responseId")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("chatcmpl_{}", uuid::Uuid::new_v4().simple()),
            ToString::to_string,
        )
}

fn value_array(value: Option<&Value>) -> Vec<&Value> {
    value
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |items| items.iter().collect())
}

fn int_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn is_empty(value: &Value) -> bool {
    matches!(value, Value::Null) || value.as_str().is_some_and(str::is_empty)
}

#[cfg(test)]
mod grok_responses_tests {
    use super::{chat_to_responses_message, responses_to_chat_request};
    use serde_json::json;

    #[test]
    fn responses_request_maps_instructions_input_and_reasoning_onto_chat() {
        let request = responses_to_chat_request(&json!({
            "model": "grok-4.6",
            "instructions": "You are terse.",
            "max_output_tokens": 64,
            "reasoning": {"effort": "low"},
            "stream": true,
            "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "hi"}]},
                {"type": "function_call", "call_id": "call-1", "name": "read", "arguments": "{\"path\":\"a\"}"},
                {"type": "function_call_output", "call_id": "call-1", "output": "contents"},
                {"type": "reasoning", "summary": []}
            ],
            "tools": [{"type": "function", "name": "read", "description": "read a file", "parameters": {"type": "object"}}]
        }));
        assert_eq!(request["model"], "grok-4.6");
        assert_eq!(request["stream"], true);
        assert_eq!(request["max_tokens"], 64);
        assert_eq!(request["reasoning_effort"], "low");
        let messages = request["messages"].as_array().expect("messages");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are terse.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "hi");
        // the tool call replays as an assistant tool_calls entry, the output
        // as a tool message keyed by the same call id
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["tool_calls"][0]["id"], "call-1");
        assert_eq!(messages[2]["tool_calls"][0]["function"]["name"], "read");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call-1");
        assert_eq!(messages[3]["content"], "contents");
        assert_eq!(messages.len(), 4, "reasoning items are dropped");
        // the responses function tool unwraps into the chat dialect's shape
        assert_eq!(request["tools"][0]["type"], "function");
        assert_eq!(request["tools"][0]["function"]["name"], "read");
    }

    #[test]
    fn responses_request_accepts_a_bare_string_input() {
        let request = responses_to_chat_request(&json!({
            "model": "grok-4.6",
            "input": "reply exactly: pong"
        }));
        let messages = request["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "reply exactly: pong");
    }

    #[test]
    fn chat_reply_becomes_a_responses_object_with_reasoning_and_tool_items() {
        let reply = chat_to_responses_message(
            &json!({
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "done",
                        "reasoning_content": "figured it out",
                        "tool_calls": [{"id": "call-9", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}}]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5,
                    "total_tokens": 15,
                    "prompt_tokens_details": {"cached_tokens": 2},
                    "completion_tokens_details": {"reasoning_tokens": 3}
                }
            }),
            "grok-4.6-build",
        );
        assert_eq!(reply["object"], "response");
        assert_eq!(reply["status"], "completed");
        assert_eq!(reply["model"], "grok-4.6-build");
        assert_eq!(reply["output"][0]["type"], "reasoning");
        assert_eq!(reply["output"][0]["summary"][0]["text"], "figured it out");
        assert_eq!(reply["output"][1]["type"], "function_call");
        assert_eq!(reply["output"][1]["call_id"], "call-9");
        assert_eq!(reply["output"][1]["name"], "read");
        assert_eq!(reply["output"][2]["type"], "message");
        assert_eq!(reply["output"][2]["content"][0]["text"], "done");
        assert_eq!(reply["usage"]["input_tokens"], 10);
        assert_eq!(reply["usage"]["output_tokens"], 5);
        assert_eq!(reply["usage"]["input_tokens_details"]["cached_tokens"], 2);
        assert_eq!(
            reply["usage"]["output_tokens_details"]["reasoning_tokens"],
            3
        );
    }

    #[test]
    fn a_length_truncated_chat_reply_is_incomplete_on_the_responses_dialect() {
        let reply = chat_to_responses_message(
            &json!({
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "partial"},
                    "finish_reason": "length"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            }),
            "grok-4.6",
        );
        assert_eq!(reply["status"], "incomplete");
    }
}

#[cfg(test)]
mod antigravity_translation_tests {
    use super::{antigravity_request, antigravity_response};
    use serde_json::json;

    /// The tool turn the upstream answered on turn one, as Gemini sends it:
    /// the call carries its id and, for Gemini models, a thought signature.
    fn turn_one() -> serde_json::Value {
        antigravity_response(
            &json!({"candidates": [{"content": {"role": "model", "parts": [{
                "thoughtSignature": "c2lnbmF0dXJlLWJ5dGVz+/=",
                "functionCall": {"name": "get_weather", "args": {"city": "Paris"},
                                 "id": "call_7168"}
            }]}, "finishReason": "STOP"}]}),
            "gemini-4-neon-low",
        )
    }

    /// F1: the second turn of a tool loop sends the call and its result back
    /// with the vendor's id, and the call with the signature the vendor minted.
    #[test]
    fn a_tool_call_round_trips_its_id_and_thought_signature() {
        let reply = turn_one();
        let call = &reply["choices"][0]["message"]["tool_calls"][0];
        let id = call["id"].as_str().expect("id").to_string();
        // The client's id stays inside the alphabet every dialect accepts.
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{id}"
        );
        let request = antigravity_request(&json!({"messages": [
            {"role": "user", "content": "weather in Paris?"},
            {"role": "assistant", "content": null, "tool_calls": [call.clone()]},
            {"role": "tool", "tool_call_id": id, "content": "22C"}
        ]}));
        let model = &request["contents"][1]["parts"][0];
        assert_eq!(model["functionCall"]["id"], "call_7168");
        assert_eq!(model["functionCall"]["name"], "get_weather");
        assert_eq!(model["thoughtSignature"], "c2lnbmF0dXJlLWJ5dGVz+/=");
        let result = &request["contents"][2]["parts"][0]["functionResponse"];
        assert_eq!(result["id"], "call_7168");
        assert_eq!(result["name"], "get_weather");
    }

    /// F1: a call with no signature to replay (another provider's history, or
    /// a Claude model, which mints none) carries the documented sentinel on the
    /// first call of its turn, and only there.
    #[test]
    fn a_call_without_a_signature_carries_the_sentinel_on_the_first_call_only() {
        let request = antigravity_request(&json!({"messages": [
            {"role": "user", "content": "two cities"},
            {"role": "assistant", "content": null, "tool_calls": [
                {"id": "toolu_vrtx_1", "type": "function",
                 "function": {"name": "w", "arguments": "{\"city\":\"Paris\"}"}},
                {"id": "toolu_vrtx_2", "type": "function",
                 "function": {"name": "w", "arguments": "{\"city\":\"Rome\"}"}}
            ]},
            {"role": "tool", "tool_call_id": "toolu_vrtx_1", "content": "22C"},
            {"role": "tool", "tool_call_id": "toolu_vrtx_2", "content": "25C"}
        ]}));
        let parts = &request["contents"][1]["parts"];
        assert_eq!(parts[0]["functionCall"]["id"], "toolu_vrtx_1");
        assert_eq!(
            parts[0]["thoughtSignature"],
            "skip_thought_signature_validator"
        );
        assert!(parts[1].get("thoughtSignature").is_none(), "{parts}");
        let results = &request["contents"][2]["parts"];
        assert_eq!(results[0]["functionResponse"]["id"], "toolu_vrtx_1");
        assert_eq!(results[1]["functionResponse"]["id"], "toolu_vrtx_2");
    }

    /// F1: an id with nothing to carry is handed to the client unchanged.
    #[test]
    fn a_call_without_a_signature_keeps_the_vendor_id() {
        let reply = antigravity_response(
            &json!({"candidates": [{"content": {"parts": [{
                "functionCall": {"name": "w", "args": {}, "id": "toolu_vrtx_01B"}
            }]}, "finishReason": "OTHER"}]}),
            "claude-sonnet-5-5-low",
        );
        assert_eq!(
            reply["choices"][0]["message"]["tool_calls"][0]["id"],
            "toolu_vrtx_01B"
        );
    }

    #[test]
    fn antigravity_request_maps_messages_system_tools_and_thinking() {
        let request = antigravity_request(&json!({
            "model": "gemini-4-neon-high",
            "reasoning_effort": "high",
            "max_tokens": 1000,
            "temperature": 0.4,
            "messages": [
                {"role": "system", "content": "be brief"},
                {"role": "user", "content": [
                    {"type": "text", "text": "look"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAA"}}
                ]},
                {"role": "assistant", "content": "checking", "tool_calls": [
                    {"id": "call_1", "type": "function",
                     "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "22C"}
            ],
            "tools": [{"type": "function", "function": {
                "name": "get_weather",
                "description": "weather",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}},
                               "required": ["city"], "additionalProperties": false}
            }}],
            "tool_choice": "auto"
        }));

        assert_eq!(request["systemInstruction"]["parts"][0]["text"], "be brief");
        let contents = request["contents"].as_array().expect("contents");
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "look");
        assert_eq!(
            contents[0]["parts"][1]["inlineData"]["mimeType"],
            "image/png"
        );
        assert_eq!(contents[0]["parts"][1]["inlineData"]["data"], "AAA");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(
            contents[1]["parts"][1]["functionCall"]["name"],
            "get_weather"
        );
        assert_eq!(
            contents[1]["parts"][1]["functionCall"]["args"]["city"],
            "Paris"
        );
        // A tool result is a user turn, and the name is resolved from the call id.
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["name"],
            "get_weather"
        );
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["content"],
            "22C"
        );
        assert_eq!(
            request["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            24_576
        );
        assert_eq!(
            request["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
        // The upstream refuses maxOutputTokens <= thinkingBudget.
        assert!(
            request["generationConfig"]["maxOutputTokens"]
                .as_i64()
                .expect("max")
                > 24_576
        );
        assert_eq!(request["generationConfig"]["temperature"], 0.4);
        assert_eq!(
            request["tools"][0]["functionDeclarations"][0]["name"],
            "get_weather"
        );
        assert_eq!(
            request["toolConfig"]["functionCallingConfig"]["mode"],
            "AUTO"
        );
    }

    /// F9: an effort alone sends no cap; a client cap below the budget is
    /// raised past it; `max_completion_tokens` wins; `none` sends no thinking.
    #[test]
    fn the_output_cap_is_the_clients_and_none_turns_thinking_off() {
        let effort_only = antigravity_request(&json!({
            "reasoning_effort": "low", "messages": [{"role": "user", "content": "x"}]
        }));
        assert!(
            effort_only["generationConfig"]
                .get("maxOutputTokens")
                .is_none(),
            "{effort_only}"
        );
        let small = antigravity_request(&json!({
            "reasoning_effort": "low", "max_tokens": 100,
            "messages": [{"role": "user", "content": "x"}]
        }));
        assert_eq!(small["generationConfig"]["maxOutputTokens"], 4096 + 1024);
        let completion = antigravity_request(&json!({
            "max_completion_tokens": 300, "max_tokens": 50,
            "messages": [{"role": "user", "content": "x"}]
        }));
        assert_eq!(completion["generationConfig"]["maxOutputTokens"], 300);
        let none = antigravity_request(&json!({
            "reasoning_effort": "none", "messages": [{"role": "user", "content": "x"}]
        }));
        assert!(none.get("generationConfig").is_none(), "{none}");
    }

    /// The fields of the Gemini `Schema` proto; anything else is a 400.
    const SCHEMA_FIELDS: [&str; 21] = [
        "type",
        "format",
        "title",
        "description",
        "nullable",
        "enum",
        "maxItems",
        "minItems",
        "properties",
        "required",
        "minProperties",
        "maxProperties",
        "minLength",
        "maxLength",
        "pattern",
        "example",
        "anyOf",
        "propertyOrdering",
        "items",
        "minimum",
        "maximum",
    ];

    /// Every schema node, recursively, holds only proto fields, and every
    /// `type` is a single string.
    fn assert_proto_schema(schema: &serde_json::Value, path: &str) {
        let object = schema
            .as_object()
            .unwrap_or_else(|| panic!("{path}: not an object"));
        for (key, value) in object {
            assert!(
                SCHEMA_FIELDS.contains(&key.as_str()),
                "{path}.{key} is not a Schema field"
            );
            match key.as_str() {
                "type" => assert!(value.is_string(), "{path}.type is {value}"),
                "enum" => assert!(
                    value
                        .as_array()
                        .expect("enum")
                        .iter()
                        .all(serde_json::Value::is_string),
                    "{path}.enum is {value}"
                ),
                "properties" => {
                    for (name, sub) in value.as_object().expect("properties") {
                        assert_proto_schema(sub, &format!("{path}.{name}"));
                    }
                }
                "items" => assert_proto_schema(value, &format!("{path}[]")),
                "anyOf" => {
                    for (i, sub) in value.as_array().expect("anyOf").iter().enumerate() {
                        assert_proto_schema(sub, &format!("{path}|{i}"));
                    }
                }
                _ => {}
            }
        }
    }

    fn declared_parameters(parameters: &serde_json::Value) -> serde_json::Value {
        let request = antigravity_request(&json!({
            "messages": [{"role": "user", "content": "x"}],
            "tools": [{"type": "function", "function": {"name": "t", "parameters": parameters}}]
        }));
        request["tools"][0]["functionDeclarations"][0]["parameters"].clone()
    }

    /// F2: Claude Code's tool schemas — draft 2020-12, nullable type arrays,
    /// exclusive bounds, closed objects — reach the upstream as proto schemas.
    #[test]
    fn claude_code_shaped_tool_schemas_become_proto_schemas() {
        let parameters = declared_parameters(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "file_path": {"type": "string", "description": "absolute path"},
                "replace_all": {"type": ["boolean", "null"], "default": false},
                "limit": {"type": "integer", "exclusiveMinimum": 0, "maximum": 2000},
                "timeout": {"type": "number", "exclusiveMaximum": 600_000},
                "url": {"type": "string", "format": "uri", "minLength": 1},
                "todos": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"status": {"type": "string",
                                              "enum": ["pending", "in_progress", "completed"]}},
                    "required": ["status"],
                    "additionalProperties": false
                }}
            },
            "required": ["file_path"]
        }));
        assert_proto_schema(&parameters, "parameters");
        let properties = &parameters["properties"];
        assert_eq!(properties["replace_all"]["type"], "boolean");
        assert_eq!(properties["replace_all"]["nullable"], true);
        assert_eq!(properties["limit"]["minimum"], 0);
        assert_eq!(properties["limit"]["maximum"], 2000);
        assert_eq!(properties["timeout"]["maximum"], 600_000);
        assert_eq!(properties["url"]["minLength"], 1);
        assert_eq!(parameters["required"], json!(["file_path"]));
        assert_eq!(properties["todos"]["items"]["required"], json!(["status"]));
    }

    /// F2: a pydantic-built MCP schema — `$defs` and `$ref`, `anyOf` with a
    /// `null` branch, titles, defaults, integer enums — keeps its shape.
    #[test]
    fn pydantic_shaped_tool_schemas_inline_refs_and_fold_nullable_unions() {
        let parameters = declared_parameters(&json!({
            "$defs": {
                "Point": {"title": "Point", "type": "object",
                          "properties": {"x": {"type": "number"}, "y": {"type": "number"}},
                          "required": ["x", "y"]},
                "Level": {"enum": [1, 2, 3], "title": "Level", "type": "integer"}
            },
            "title": "DrawArgs",
            "type": "object",
            "properties": {
                "at": {"$ref": "#/$defs/Point"},
                "level": {"$ref": "#/$defs/Level", "default": 1},
                "label": {"anyOf": [{"type": "string"}, {"type": "null"}],
                          "default": null, "title": "Label"},
                "mode": {"oneOf": [{"const": "fill"}, {"const": "stroke"}]},
                "points": {"type": "array", "items": {"$ref": "#/$defs/Point"}}
            },
            "required": ["at"]
        }));
        assert_proto_schema(&parameters, "parameters");
        let properties = &parameters["properties"];
        assert_eq!(properties["at"]["type"], "object");
        assert_eq!(properties["at"]["required"], json!(["x", "y"]));
        assert_eq!(properties["level"]["enum"], json!(["1", "2", "3"]));
        assert_eq!(properties["level"]["type"], "string");
        assert_eq!(properties["label"]["type"], "string");
        assert_eq!(properties["label"]["nullable"], true);
        assert_eq!(
            properties["points"]["items"]["properties"]["x"]["type"],
            "number"
        );
        let modes = properties["mode"]["anyOf"].as_array().expect("anyOf");
        assert_eq!(modes[0]["enum"], json!(["fill"]));
    }

    /// F2: a schema that refers to itself ends at a depth guard instead of
    /// recursing forever.
    #[test]
    fn a_recursive_ref_stops_at_the_depth_guard() {
        let parameters = declared_parameters(&json!({
            "$defs": {"Node": {"type": "object", "properties": {
                "children": {"type": "array", "items": {"$ref": "#/$defs/Node"}}}}},
            "type": "object",
            "properties": {"root": {"$ref": "#/$defs/Node"}}
        }));
        assert_proto_schema(&parameters, "parameters");
    }

    /// F2: Codex CLI's tools are plain objects with string arrays; they pass
    /// through whole.
    #[test]
    fn codex_cli_shaped_tool_schemas_pass_through() {
        let schema = json!({
            "type": "object",
            "properties": {
                "command": {"type": "array", "items": {"type": "string"},
                            "description": "The command to execute"},
                "workdir": {"type": "string"},
                "timeout_ms": {"type": "number"}
            },
            "required": ["command"],
            "additionalProperties": false
        });
        let parameters = declared_parameters(&schema);
        assert_proto_schema(&parameters, "parameters");
        assert_eq!(
            parameters["properties"]["command"]["items"]["type"],
            "string"
        );
        assert_eq!(parameters["required"], json!(["command"]));
    }

    /// L6: a malformed call is not a length cutoff, and a blocked prompt (no
    /// candidates, only `promptFeedback`) is a content filter, not a quiet stop.
    #[test]
    fn malformed_calls_and_blocked_prompts_get_their_own_finish_reason() {
        let malformed = antigravity_response(
            &json!({"candidates": [{"content": {"parts": []},
                                    "finishReason": "MALFORMED_FUNCTION_CALL"}]}),
            "m",
        );
        assert_eq!(malformed["choices"][0]["finish_reason"], "stop");
        let blocked =
            antigravity_response(&json!({"promptFeedback": {"blockReason": "SAFETY"}}), "m");
        assert_eq!(blocked["choices"][0]["finish_reason"], "content_filter");
    }

    /// L8: a hosted image names its MIME type, and a data URL that is not
    /// base64 is encoded before it goes out as `inlineData`.
    #[test]
    fn image_parts_carry_a_mime_type_and_base64_data() {
        let request = antigravity_request(&json!({"messages": [{"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": "https://example.com/a/cat.PNG?x=1"}},
            {"type": "image_url", "image_url": {"url": "data:image/svg+xml,%3Csvg%2F%3E"}}
        ]}]}));
        let parts = &request["contents"][0]["parts"];
        assert_eq!(
            parts[0]["fileData"]["fileUri"],
            "https://example.com/a/cat.PNG?x=1"
        );
        assert_eq!(parts[0]["fileData"]["mimeType"], "image/png");
        assert_eq!(parts[1]["inlineData"]["mimeType"], "image/svg+xml");
        // `<svg/>` in base64.
        assert_eq!(parts[1]["inlineData"]["data"], "PHN2Zy8+");
    }

    #[test]
    fn antigravity_request_drops_the_schema_keywords_the_upstream_rejects() {
        let request = antigravity_request(&json!({
            "messages": [{"role": "user", "content": "x"}],
            "tools": [{"type": "function", "function": {
                "name": "pick",
                "parameters": {
                    "type": "object",
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "properties": {
                        "kind": {"const": "email"},
                        "other": {"type": "string", "default": "x", "examples": ["y"]}
                    }
                }
            }}]
        }));
        let parameters = &request["tools"][0]["functionDeclarations"][0]["parameters"];
        assert!(parameters.get("$schema").is_none(), "{parameters}");
        assert_eq!(parameters["properties"]["kind"]["enum"], json!(["email"]));
        assert!(parameters["properties"]["other"].get("default").is_none());
        assert!(parameters["properties"]["other"].get("examples").is_none());
    }

    #[test]
    fn antigravity_response_maps_thinking_text_usage_and_tool_calls() {
        let response = antigravity_response(
            &json!({
                "candidates": [{
                    "content": {"role": "model", "parts": [
                        {"text": "weighing", "thought": true},
                        {"text": "the answer"},
                        {"functionCall": {"name": "stop", "args": {"now": true}, "id": "call_9"}}
                    ]},
                    "finishReason": "STOP"
                }],
                "usageMetadata": {
                    "promptTokenCount": 100,
                    "candidatesTokenCount": 20,
                    "thoughtsTokenCount": 30,
                    "totalTokenCount": 150,
                    "cachedContentTokenCount": 40
                }
            }),
            "gemini-4-neon-high",
        );

        assert_eq!(response["object"], "chat.completion");
        assert_eq!(response["model"], "gemini-4-neon-high");
        let message = &response["choices"][0]["message"];
        assert_eq!(message["role"], "assistant");
        assert_eq!(message["reasoning_content"], "weighing");
        assert_eq!(message["content"], "the answer");
        assert_eq!(message["tool_calls"][0]["id"], "call_9");
        assert_eq!(message["tool_calls"][0]["function"]["name"], "stop");
        assert_eq!(
            message["tool_calls"][0]["function"]["arguments"],
            "{\"now\":true}"
        );
        // A function call is a tool_calls finish whatever the vendor said.
        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
        let usage = &response["usage"];
        assert_eq!(usage["prompt_tokens"], 100);
        // Gemini reports thinking apart from candidates; the Chat count holds
        // both, and the total is the upstream's own.
        assert_eq!(usage["completion_tokens"], 50);
        assert_eq!(usage["total_tokens"], 150);
        assert_eq!(usage["completion_tokens_details"]["reasoning_tokens"], 30);
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 40);
    }

    #[test]
    fn antigravity_response_reports_length_when_the_vendor_cut_the_turn() {
        let response = antigravity_response(
            &json!({"candidates": [{"content": {"parts": [{"text": "half"}]},
                                    "finishReason": "MAX_TOKENS"}]}),
            "gemini-4-neon",
        );
        assert_eq!(response["choices"][0]["finish_reason"], "length");
        assert!(response.get("usage").is_none(), "nothing was published");
    }
}
