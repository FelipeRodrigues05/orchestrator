//! Model calls shared by the `ApiRunner` and the host pet: Anthropic Messages and
//! OpenAI-compatible chat completions (OpenAI, OpenRouter, Gemini, Ollama), with
//! tool calls and token usage.

use std::time::Duration;

use serde_json::{Value, json};

use super::tools::{ToolCall, ToolDef};
use crate::model::{Provider, Usage};

#[derive(Clone)]
pub enum Wire {
    Anthropic { key: String, model: String },
    OpenAi { base: String, key: Option<String>, model: String },
}

/// Provider-neutral conversation.
#[derive(Debug, Clone)]
pub enum Turn {
    User(String),
    Assistant { text: String, calls: Vec<ToolCall> },
    Results(Vec<(String, String, bool)>),
}

pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Usage,
}

pub fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().timeout(Duration::from_secs(300)).build().map_err(|e| e.to_string())
}

/// Resolves the endpoint and key for an `Api` or `Local` provider.
pub async fn wire_for(provider: &Provider) -> Result<Wire, String> {
    Ok(match provider {
        Provider::Api { service, model, key_ref } => {
            let key = crate::secrets::load(key_ref)
                .await
                .map_err(|e| format!("Secret Service: {e}"))?
                .ok_or_else(|| format!("no API key for {service}: type :key {service}"))?;
            let openai = |base: &str| Wire::OpenAi { base: base.into(), key: Some(key.clone()), model: model.clone() };
            match service.as_str() {
                "anthropic" => Wire::Anthropic { key: key.clone(), model: model.clone() },
                "openai" => openai("https://api.openai.com/v1"),
                "openrouter" => openai("https://openrouter.ai/api/v1"),
                "gemini" => openai("https://generativelanguage.googleapis.com/v1beta/openai"),
                other => return Err(format!("unknown API service {other}")),
            }
        }
        Provider::Local { url, model } => {
            Wire::OpenAi { base: format!("{}/v1", url.trim_end_matches('/')), key: None, model: model.clone() }
        }
        _ => return Err("needs an Api or Local provider".into()),
    })
}

pub async fn complete(
    client: &reqwest::Client,
    wire: &Wire,
    system: &str,
    definitions: &[ToolDef],
    history: &[Turn],
) -> Result<Reply, String> {
    let request = match wire {
        Wire::Anthropic { key, model } => client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&anthropic_body(model, system, definitions, history)),
        Wire::OpenAi { base, key, model } => {
            let request =
                client.post(format!("{base}/chat/completions")).json(&openai_body(model, system, definitions, history));
            match key {
                Some(key) => request.bearer_auth(key),
                None => request,
            }
        }
    };
    let response = request.send().await.map_err(|e| e.without_url().to_string())?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|e| e.without_url().to_string())?;
    if !status.is_success() {
        let message = body["error"]["message"].as_str().map_or_else(|| body.to_string(), str::to_owned);
        return Err(format!("{status}: {}", message.chars().take(300).collect::<String>()));
    }
    Ok(match wire {
        Wire::Anthropic { .. } => parse_anthropic(&body),
        Wire::OpenAi { .. } => parse_openai(&body),
    })
}

fn anthropic_body(model: &str, system: &str, definitions: &[ToolDef], history: &[Turn]) -> Value {
    let messages: Vec<Value> = history
        .iter()
        .map(|turn| match turn {
            Turn::User(text) => json!({ "role": "user", "content": text }),
            Turn::Assistant { text, calls } => {
                let mut content = Vec::new();
                if !text.is_empty() {
                    content.push(json!({ "type": "text", "text": text }));
                }
                for call in calls {
                    content.push(json!({ "type": "tool_use", "id": call.id, "name": call.name, "input": call.input }));
                }
                json!({ "role": "assistant", "content": content })
            }
            Turn::Results(results) => {
                json!({ "role": "user", "content": results.iter().map(|(id, output, error)| json!({
                "type": "tool_result", "tool_use_id": id, "content": output, "is_error": error
            })).collect::<Vec<_>>() })
            }
        })
        .collect();
    json!({
        "model": model,
        "max_tokens": 8192,
        "system": system,
        "tools": definitions.iter().map(|d| json!({ "name": d.name, "description": d.description, "input_schema": d.schema })).collect::<Vec<_>>(),
        "messages": messages,
    })
}

fn openai_body(model: &str, system: &str, definitions: &[ToolDef], history: &[Turn]) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": system })];
    for turn in history {
        match turn {
            Turn::User(text) => messages.push(json!({ "role": "user", "content": text })),
            Turn::Assistant { text, calls } => {
                let mut message = json!({ "role": "assistant", "content": text });
                if !calls.is_empty() {
                    message["tool_calls"] = calls
                        .iter()
                        .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.input.to_string() } }))
                        .collect();
                }
                messages.push(message);
            }
            Turn::Results(results) => {
                for (id, output, _) in results {
                    messages.push(json!({ "role": "tool", "tool_call_id": id, "content": output }));
                }
            }
        }
    }
    json!({
        "model": model,
        "messages": messages,
        "tools": definitions.iter().map(|d| json!({ "type": "function", "function": {
            "name": d.name, "description": d.description, "parameters": d.schema
        } })).collect::<Vec<_>>(),
    })
}

fn parse_anthropic(body: &Value) -> Reply {
    let mut reply = Reply { text: String::new(), calls: Vec::new(), usage: Usage::default() };
    for block in body["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => reply.text.push_str(block["text"].as_str().unwrap_or_default()),
            Some("tool_use") => reply.calls.push(ToolCall {
                id: block["id"].as_str().unwrap_or_default().into(),
                name: block["name"].as_str().unwrap_or_default().into(),
                input: block["input"].clone(),
            }),
            _ => {}
        }
    }
    reply.usage = Usage {
        input_tokens: body["usage"]["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: body["usage"]["output_tokens"].as_u64().unwrap_or(0),
    };
    reply
}

fn parse_openai(body: &Value) -> Reply {
    let message = &body["choices"][0]["message"];
    let calls = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, call)| {
            let arguments = &call["function"]["arguments"];
            // Ollama sends an object, OpenAI a JSON string.
            let input = match arguments.as_str() {
                Some(text) => serde_json::from_str(text).unwrap_or_else(|_| json!({})),
                None => arguments.clone(),
            };
            ToolCall {
                id: call["id"].as_str().map_or_else(|| format!("call_{index}"), str::to_owned),
                name: call["function"]["name"].as_str().unwrap_or_default().into(),
                input,
            }
        })
        .collect();
    let usage = Usage {
        input_tokens: body["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: body["usage"]["completion_tokens"].as_u64().unwrap_or(0),
    };
    Reply { text: strip_thinking(message["content"].as_str().unwrap_or_default()), calls, usage }
}

/// Reasoning models served by Ollama (qwen3…) inline `<think>…</think>`.
fn strip_thinking(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::tools;

    #[test]
    fn openai_tool_calls_parse_from_strings_and_objects() {
        let body = json!({ "choices": [{ "message": { "content": "<think>hm</think>Done.", "tool_calls": [
            { "id": "a", "function": { "name": "read_file", "arguments": "{\"path\":\"x\"}" } },
            { "function": { "name": "run_command", "arguments": { "command": "ls" } } }
        ] } }] });
        let reply = parse_openai(&body);
        assert_eq!(reply.text, "Done.");
        assert_eq!(reply.calls[0].input, json!({ "path": "x" }));
        assert_eq!(reply.calls[1].id, "call_1");
        assert_eq!(reply.calls[1].input, json!({ "command": "ls" }));
    }

    #[test]
    fn histories_convert_to_both_wires() {
        let call = ToolCall { id: "t1".into(), name: "read_file".into(), input: json!({ "path": "a" }) };
        let history = vec![
            Turn::User("hi".into()),
            Turn::Assistant { text: String::new(), calls: vec![call] },
            Turn::Results(vec![("t1".into(), "content".into(), false)]),
        ];
        let anthropic = anthropic_body("m", "sys", &tools::definitions(), &history);
        assert_eq!(anthropic["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(anthropic["messages"][2]["content"][0]["tool_use_id"], "t1");
        let openai = openai_body("m", "sys", &tools::definitions(), &history);
        assert_eq!(openai["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"path\":\"a\"}");
        assert_eq!(openai["messages"][3]["role"], "tool");
    }
}
