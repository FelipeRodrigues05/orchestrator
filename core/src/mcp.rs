//! A minimal MCP server for the agents' CLIs. JSON-RPC over HTTP on localhost; each
//! agent gets its own URL token, so the handler knows who calls (ADR 0005). Tools:
//! `permission` (`claude --permission-prompt-tool`) and the team tools `list_agents`
//! and `ask_agent` (ADR 0018). Only the slice of MCP this needs is implemented (ADR 0016).

use std::{collections::HashMap, io, path::PathBuf};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};

use crate::runner::{AgentInfo, RunnerContext, ask_permission};

pub const TOOL: &str = "permission";
/// What the CLI must be told: `mcp__<server>__<tool>`.
pub const SERVER: &str = "orch";

#[derive(Clone)]
struct Registration {
    context: RunnerContext,
    agent: AgentInfo,
}

enum Msg {
    Register { token: String, registration: Registration },
    Unregister(String),
    Lookup { token: String, reply: oneshot::Sender<Option<Registration>> },
}

#[derive(Clone)]
pub struct McpHandle {
    tx: mpsc::UnboundedSender<Msg>,
    /// `http://127.0.0.1:<port>/mcp`; agents use `<url>/<token>`.
    pub url: String,
}

impl McpHandle {
    /// Gives an agent its endpoint; returns the URL to hand to its CLI.
    pub(crate) fn register(&self, token: &str, context: RunnerContext, agent: AgentInfo) -> String {
        let registration = Registration { context, agent };
        let _ = self.tx.send(Msg::Register { token: token.to_owned(), registration });
        format!("{}/{token}", self.url)
    }

    pub fn unregister(&self, token: &str) {
        let _ = self.tx.send(Msg::Unregister(token.to_owned()));
    }
}

/// Binds a free localhost port and serves until the runtime stops.
pub async fn spawn() -> io::Result<McpHandle> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/mcp", listener.local_addr()?);
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();

    tokio::spawn(async move {
        let mut agents: HashMap<String, Registration> = HashMap::new();
        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::Register { token, registration } => {
                    agents.insert(token, registration);
                }
                Msg::Unregister(token) => {
                    agents.remove(&token);
                }
                Msg::Lookup { token, reply } => {
                    let _ = reply.send(agents.get(&token).cloned());
                }
            }
        }
    });

    let registry = tx.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let registry = registry.clone();
            tokio::spawn(async move {
                if let Err(error) = serve(stream, registry).await {
                    tracing::debug!(%error, "mcp connection");
                }
            });
        }
    });
    Ok(McpHandle { tx, url })
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

async fn read_request(stream: &mut BufReader<TcpStream>) -> io::Result<Option<Request>> {
    let mut line = String::new();
    if stream.read_line(&mut line).await? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or_default().to_owned(), parts.next().unwrap_or_default().to_owned());
    let mut length = 0;
    loop {
        line.clear();
        stream.read_line(&mut line).await?;
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok(Some(Request { method, path, body }))
}

async fn write_response(stream: &mut BufReader<TcpStream>, status: &str, body: Option<&Value>) -> io::Result<()> {
    let body = body.map(Value::to_string).unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    let stream = stream.get_mut();
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await
}

async fn serve(stream: TcpStream, registry: mpsc::UnboundedSender<Msg>) -> io::Result<()> {
    let mut stream = BufReader::new(stream);
    while let Some(request) = read_request(&mut stream).await? {
        let token = request.path.strip_prefix("/mcp/").unwrap_or_default().trim_end_matches('/').to_owned();
        let (reply, registration) = oneshot::channel();
        let _ = registry.send(Msg::Lookup { token, reply });
        let Some(registration) = registration.await.ok().flatten() else {
            write_response(&mut stream, "404 Not Found", None).await?;
            continue;
        };
        match request.method.as_str() {
            "POST" => {
                let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
                    let error = rpc_error(&Value::Null, -32700, "parse error");
                    write_response(&mut stream, "400 Bad Request", Some(&error)).await?;
                    continue;
                };
                let answer = match body {
                    Value::Array(batch) => {
                        let mut answers = Vec::new();
                        for message in batch {
                            answers.extend(handle(message, &registration).await);
                        }
                        (!answers.is_empty()).then(|| Value::Array(answers))
                    }
                    message => handle(message, &registration).await,
                };
                match answer {
                    Some(answer) => write_response(&mut stream, "200 OK", Some(&answer)).await?,
                    // Notifications only.
                    None => write_response(&mut stream, "202 Accepted", None).await?,
                }
            }
            "DELETE" => write_response(&mut stream, "200 OK", None).await?,
            // No server-initiated stream.
            _ => write_response(&mut stream, "405 Method Not Allowed", None).await?,
        }
    }
    Ok(())
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// One JSON-RPC message; `None` for notifications.
async fn handle(message: Value, registration: &Registration) -> Option<Value> {
    let id = message.get("id").cloned()?;
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER, "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => {
            let mut tools = vec![json!({
                "name": TOOL,
                "description": "Asks the operator (through the orchestrator's broker) whether a tool may run.",
                "inputSchema": { "type": "object", "properties": {
                    "tool_name": { "type": "string" },
                    "input": { "type": "object" },
                    "tool_use_id": { "type": "string" }
                }, "required": ["tool_name", "input"] },
            })];
            if registration.context.team.is_some() {
                tools.extend(crate::team::tools().into_iter().map(|(name, description, schema)| {
                    json!({ "name": name, "description": description, "inputSchema": schema })
                }));
            }
            json!({ "tools": tools })
        }
        "tools/call" if registration.context.team.is_some() && params["name"] != TOOL => {
            let team = registration.context.team.as_ref()?;
            let name = params["name"].as_str().unwrap_or_default();
            let Some(result) = team.call(registration.agent.id, name, &params["arguments"]).await else {
                return Some(rpc_error(&id, -32602, "unknown tool"));
            };
            let (text, failed) = match result {
                Ok(text) => (text, false),
                Err(error) => (error, true),
            };
            json!({ "content": [{ "type": "text", "text": text }], "isError": failed })
        }
        "tools/call" if params["name"] == TOOL => {
            let arguments = &params["arguments"];
            let tool = arguments["tool_name"].as_str().unwrap_or("Tool").to_owned();
            let input = arguments.get("input").cloned().unwrap_or_else(|| json!({}));
            let paths = ["file_path", "path", "notebook_path"]
                .iter()
                .filter_map(|key| input.get(*key).and_then(Value::as_str))
                .map(PathBuf::from)
                .collect();
            let decision =
                ask_permission(&registration.context, &registration.agent, &tool, input.clone(), paths).await;
            let verdict = match decision {
                crate::model::Decision::Deny { reason } => {
                    json!({ "behavior": "deny", "message": reason })
                }
                _ => json!({ "behavior": "allow", "updatedInput": input }),
            };
            json!({ "content": [{ "type": "text", "text": verdict.to_string() }] })
        }
        "tools/call" => return Some(rpc_error(&id, -32602, "unknown tool")),
        _ => return Some(rpc_error(&id, -32601, "method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use uuid::Uuid;

    use super::*;
    use crate::{
        broker::{self, RuleScope},
        model::{Effect, PermissionRule},
    };

    async fn call(url: &str, body: Value) -> Value {
        let response = reqwest::Client::new().post(url).json(&body).send().await.unwrap();
        response.json().await.unwrap()
    }

    #[tokio::test]
    async fn permission_tool_answers_through_the_broker() {
        let mcp = spawn().await.unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let broker = broker::spawn(Duration::from_secs(5), tx, None);
        let agent = AgentInfo { id: Uuid::new_v4(), name: "Tests".into(), worktree: "/tmp/w".into(), model: None };
        broker.set_rules(
            RuleScope::Agent(agent.id),
            vec![
                PermissionRule::parse("Bash(cargo test:*)", Effect::Allow).unwrap(),
                PermissionRule::parse("Bash(rm:*)", Effect::Deny).unwrap(),
            ],
        );
        let context = RunnerContext {
            broker,
            group_id: Uuid::new_v4(),
            group_name: "API".into(),
            sandbox: false,
            mcp: None,
            team: None,
            extra_dirs: Vec::new(),
        };
        let url = mcp.register("t0ken", context, agent);

        let init = call(
            &url,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }),
        )
        .await;
        assert_eq!(init["result"]["serverInfo"]["name"], SERVER);
        let tools = call(&url, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await;
        assert_eq!(tools["result"]["tools"][0]["name"], TOOL);

        let ask = |command: &str| {
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": TOOL, "arguments": { "tool_name": "Bash", "input": { "command": command } }
        } })
        };
        let allowed = call(&url, ask("cargo test")).await;
        let verdict: Value = serde_json::from_str(allowed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(verdict["behavior"], "allow");
        assert_eq!(verdict["updatedInput"]["command"], "cargo test");
        let denied = call(&url, ask("rm -rf target")).await;
        let verdict: Value = serde_json::from_str(denied["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(verdict["behavior"], "deny");

        // Another agent's token is not valid.
        let status =
            reqwest::Client::new().post(format!("{}/other", mcp.url)).body("{}").send().await.unwrap().status();
        assert_eq!(status, 404);
    }

    #[tokio::test]
    async fn team_tools_reach_the_orchestrator() {
        let mcp = spawn().await.unwrap();
        let (tx, _rx) = mpsc::unbounded_channel();
        let broker = broker::spawn(Duration::from_secs(5), tx, None);
        let (team_tx, mut team_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(request) = team_rx.recv().await {
                match request {
                    crate::team::TeamRequest::Roster { reply, .. } => {
                        let _ = reply.send("- Front (group Web, Running): front end only".into());
                    }
                    crate::team::TeamRequest::Remember { reply, .. } => {
                        let _ = reply.send(Ok("noted".into()));
                    }
                    crate::team::TeamRequest::Ask { to, message, reply, .. } => {
                        let _ = reply.send(Ok(format!("{to} got {message}")));
                    }
                }
            }
        });
        let agent = AgentInfo { id: Uuid::new_v4(), name: "Back".into(), worktree: "/tmp/w".into(), model: None };
        let context = RunnerContext {
            broker,
            group_id: Uuid::new_v4(),
            group_name: "API".into(),
            sandbox: false,
            mcp: None,
            team: Some(crate::team::TeamHandle::new(team_tx)),
            extra_dirs: Vec::new(),
        };
        let url = mcp.register("team", context, agent);

        let tools = call(&url, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await;
        let names: Vec<&str> =
            tools["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, [TOOL, crate::team::REMEMBER, crate::team::LIST, crate::team::ASK]);

        let ask = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": crate::team::ASK, "arguments": { "agent": "Front", "message": "is the form wired?" }
        } });
        let answer = call(&url, ask).await;
        assert_eq!(answer["result"]["content"][0]["text"], "Front got is the form wired?");
        assert_eq!(answer["result"]["isError"], false);
        let list = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": crate::team::LIST, "arguments": {} } });
        assert!(call(&url, list).await["result"]["content"][0]["text"].as_str().unwrap().contains("front end only"));
    }
}
