use crate::storage::{Message, Provider, Settings};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::Duration,
};

#[derive(Debug)]
pub enum WorkerEvent {
    ProcessStarted(u32),
    Chunk(String),
    Info(String),
    Session(Provider, String),
    Finished(Result<(), String>),
}

pub fn spawn_chat(settings: Settings, sender: Sender<WorkerEvent>, cancelled: Arc<AtomicBool>) {
    thread::spawn(move || {
        let result = match settings.provider {
            Provider::Codex => cli_codex(&settings, &sender),
            Provider::ClaudeCode => cli_claude(&settings, &sender),
            _ => http_chat(&settings, &sender, &cancelled),
        };
        let _ = sender.send(WorkerEvent::Finished(result));
    });
}

fn command_hidden(command: &str) -> Command {
    let mut cmd = Command::new(command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: child CLI tools run behind the Ratatui terminal.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

pub fn is_available(name: &str) -> bool {
    command_hidden(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn prompt(settings: &Settings) -> Result<&str, String> {
    settings
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| message.content.as_str())
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| "Escreva uma mensagem antes de enviar.".into())
}

fn cli_codex(settings: &Settings, tx: &Sender<WorkerEvent>) -> Result<(), String> {
    let question = prompt(settings)?;
    if !is_available("codex") {
        return Err("Codex CLI nao instalado. Use a opcao 'Instalar Codex' em Setup.".into());
    }
    let mut command = command_hidden("codex");
    if let Some(session) = &settings.codex_session_id {
        command.args(["exec", "resume", "--json"]);
        command.arg("-c").arg(format!(
            "sandbox_mode=\"{}\"",
            if settings.allow_edits { "workspace-write" } else { "read-only" }
        ));
        command.args([session, "-"]);
    } else {
        command.args(["exec", "--json", "--skip-git-repo-check", "--sandbox"]);
        command.arg(if settings.allow_edits { "workspace-write" } else { "read-only" });
        command.arg("-C").arg(&settings.root);
        if !settings.model().is_empty() {
            command.arg("--model").arg(settings.model());
        }
        command.arg("-");
    }
    command.current_dir(&settings.root);
    let mut last_message = String::new();
    run_cli(command, question, tx, |payload, sender| {
        if payload["type"] == "thread.started" {
            if let Some(id) = payload["thread_id"].as_str() {
                let _ = sender.send(WorkerEvent::Session(Provider::Codex, id.to_owned()));
            }
        }
        let kind = payload["type"].as_str().unwrap_or("");
        let item = &payload["item"];
        let item_type = item["type"].as_str().unwrap_or("");
        if kind == "item.completed" && item_type == "agent_message" {
            if let Some(text) = item["text"].as_str() {
                if !text.is_empty() && text != last_message {
                    last_message = text.to_owned();
                    let _ = sender.send(WorkerEvent::Chunk(format!("{text}\n")));
                }
            }
        } else if kind == "item.started" && item_type == "command_execution" {
            let _ = sender.send(WorkerEvent::Info("Codex: executando ferramenta".into()));
        } else if kind == "turn.failed" {
            let msg = payload["error"]["message"].as_str().unwrap_or("Erro no Codex");
            let _ = sender.send(WorkerEvent::Info(format!("Codex: {msg}")));
        }
    })
}

fn cli_claude(settings: &Settings, tx: &Sender<WorkerEvent>) -> Result<(), String> {
    let question = prompt(settings)?;
    if !is_available("claude") {
        return Err("Claude Code CLI nao instalado. Use 'Instalar Claude' em Setup.".into());
    }
    let mut command = command_hidden("claude");
    command.args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-mode",
        if settings.allow_edits { "acceptEdits" } else { "plan" },
    ]);
    if let Some(session) = &settings.claude_session_id {
        command.arg("--resume").arg(session);
    }
    if !settings.model().is_empty() {
        command.arg("--model").arg(settings.model());
    }
    command.current_dir(&settings.root);
    let mut saw_stream = false;
    run_cli(command, question, tx, |payload, sender| {
        if let Some(id) = payload["session_id"].as_str() {
            let _ = sender.send(WorkerEvent::Session(Provider::ClaudeCode, id.to_owned()));
        }
        match payload["type"].as_str().unwrap_or("") {
            "stream_event" => {
                let event = &payload["event"];
                if event["type"] == "content_block_delta" {
                    if let Some(text) = event["delta"]["text"].as_str() {
                        saw_stream = true;
                        let _ = sender.send(WorkerEvent::Chunk(text.to_owned()));
                    }
                }
            }
            "result" => {
                if !saw_stream {
                    if let Some(text) = payload["result"].as_str() {
                        let _ = sender.send(WorkerEvent::Chunk(text.to_owned()));
                    }
                }
                if payload["is_error"] == true {
                    let _ = sender.send(WorkerEvent::Info("Claude Code reportou erro".into()));
                }
            }
            "assistant" => {
                if payload["message"]["content"]
                    .as_array()
                    .is_some_and(|items| items.iter().any(|item| item["type"] == "tool_use"))
                {
                    let _ = sender.send(WorkerEvent::Info("Claude: ferramenta em execucao".into()));
                }
            }
            _ => {}
        }
    })
}

fn run_cli(
    mut command: Command,
    input: &str,
    tx: &Sender<WorkerEvent>,
    mut on_event: impl FnMut(&Value, &Sender<WorkerEvent>),
) -> Result<(), String> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| format!("Nao foi possivel iniciar CLI: {error}"))?;
    let _ = tx.send(WorkerEvent::ProcessStarted(child.id()));
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| format!("Erro ao enviar prompt: {error}"))?;
        // stdin is dropped here to signal EOF to Codex/Claude.
    }
    let stderr = child.stderr.take().ok_or("stderr indisponivel")?;
    let log_thread = thread::spawn(move || {
        let mut text = String::new();
        let _ = BufReader::new(stderr).take(8192).read_to_string(&mut text);
        text
    });
    let stdout = child.stdout.take().ok_or("stdout indisponivel")?;
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|error| format!("Falha lendo agente: {error}"))?;
        if let Ok(json) = serde_json::from_str::<Value>(&line) {
            on_event(&json, tx);
        }
    }
    let status = child.wait().map_err(|error| format!("Falha aguardando CLI: {error}"))?;
    let stderr = log_thread.join().unwrap_or_default();
    if !status.success() {
        let safe = stderr.chars().take(700).collect::<String>();
        return Err(if safe.trim().is_empty() {
            format!("CLI encerrou com codigo {status}. Verifique login e permissao.")
        } else {
            format!("CLI encerrou com codigo {status}: {safe}")
        });
    }
    Ok(())
}

fn http_chat(settings: &Settings, tx: &Sender<WorkerEvent>, cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        return Err("Cancelado antes de enviar a solicitacao".into());
    }
    let provider = settings.provider;
    let key = if provider.requires_key() {
        crate::storage::load_key(provider)
            .ok_or_else(|| format!("Cadastre a chave {} em Configuracao.", provider.label()))?
    } else {
        String::new()
    };
    let endpoint = settings.endpoint().trim_end_matches('/');
    let (url, body): (String, Value) = match provider {
        Provider::Anthropic => {
            let mut system = String::new();
            let messages: Vec<Value> = settings
                .messages
                .iter()
                .rev()
                .take(40)
                .rev()
                .filter(|m| {
                    if m.role == "system" {
                        system.push_str(&m.content);
                        false
                    } else {
                        !m.content.is_empty()
                    }
                })
                .map(|m| json!({"role":m.role,"content":m.content}))
                .collect();
            let mut body = json!({"model":settings.model(),"max_tokens":4096,"messages":messages,"stream":true});
            if !system.is_empty() {
                body["system"] = json!(system);
            }
            (format!("{endpoint}/messages"), body)
        }
        Provider::Ollama => (
            format!("{endpoint}/api/chat"),
            json!({"model":settings.model(),"messages":settings.messages.iter().rev().take(40).rev().filter(|m| !m.content.is_empty()).collect::<Vec<_>>(),"stream":true}),
        ),
        _ => (
            format!("{endpoint}/chat/completions"),
            json!({"model":settings.model(),"messages":settings.messages.iter().rev().take(40).rev().filter(|m| !m.content.is_empty()).collect::<Vec<_>>(),"stream":true}),
        ),
    };

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| format!("Cliente HTTP: {error}"))?;
    let mut request = client.post(url).json(&body);
    match provider {
        Provider::Anthropic => {
            request = request
                .header("x-api-key", &key)
                .header("anthropic-version", "2023-06-01");
        }
        Provider::OpenAi | Provider::OpenRouter => {
            request = request.bearer_auth(&key);
        }
        _ => {}
    }
    let response = request.send().map_err(|error| format!("Conexao falhou: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().unwrap_or_default();
        return Err(format!("Provedor respondeu HTTP {status}: {}", detail.chars().take(500).collect::<String>()));
    }
    let reader = BufReader::new(response);
    let mut saw_chunk = false;
    for line in reader.lines() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Resposta cancelada".into());
        }
        let line = line.map_err(|error| format!("Streaming interrompido: {error}"))?;
        let raw = if provider == Provider::Ollama {
            line.trim()
        } else if let Some(data) = line.strip_prefix("data:") {
            data.trim()
        } else {
            continue;
        };
        if raw == "[DONE]" {
            break;
        }
        if raw.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(raw) else { continue };
        if value.get("error").is_some() {
            return Err(format!("Provedor retornou erro: {}", value["error"]));
        }
        if let Some(chunk) = extract_stream_text(provider, &value) {
            if !chunk.is_empty() {
                saw_chunk = true;
                let _ = tx.send(WorkerEvent::Chunk(chunk.into()));
            }
        }
    }
    if !saw_chunk {
        return Err("O provedor encerrou a resposta sem enviar texto.".into());
    }
    Ok(())
}

fn extract_stream_text<'a>(provider: Provider, value: &'a Value) -> Option<&'a str> {
    match provider {
        Provider::Anthropic => {
            if value["type"] == "content_block_delta" {
                value["delta"]["text"].as_str()
            } else {
                None
            }
        }
        Provider::Ollama => value["message"]["content"].as_str(),
        _ => value["choices"][0]["delta"]["content"].as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn openai_and_ollama_chunks() {
        assert_eq!(
            extract_stream_text(Provider::OpenAi, &json!({"choices":[{"delta":{"content":"oi"}}]})),
            Some("oi")
        );
        assert_eq!(
            extract_stream_text(Provider::Ollama, &json!({"message":{"content":"ok"}})),
            Some("ok")
        );
    }

    #[test]
    fn anthropic_stream_event() {
        assert_eq!(
            extract_stream_text(Provider::Anthropic, &json!({"type":"content_block_delta","delta":{"text":"ola"}})),
            Some("ola")
        );
    }

    #[test]
    fn default_agent_sandbox_is_read_only() {
        assert!(!Settings::default().allow_edits);
    }

    #[test]
    fn assistant_placeholder_does_not_hide_user_prompt() {
        let mut s = Settings::default();
        s.messages.push(Message { role: "user".into(), content: "teste".into() });
        s.messages.push(Message { role: "assistant".into(), content: "".into() });
        assert_eq!(prompt(&s).unwrap(), "teste");
    }

    #[test]
    fn no_bypass_flags() {
        let source = include_str!("backend.rs");
        assert!(!source.contains(concat!("dangerously-", "bypass-approvals-and-sandbox")));
        assert!(!source.contains(concat!("dangerously-", "skip-permissions")));
    }

    #[test]
    fn local_ollama_stream_is_read_incrementally() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 8192];
            let _ = stream.read(&mut request).unwrap();
            let body = "{\"message\":{\"content\":\"ola\"}}\n{\"message\":{\"content\":\" mundo\"},\"done\":true}\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let mut settings = Settings::default();
        settings.switch_provider(Provider::Ollama);
        settings.endpoint = format!("http://127.0.0.1:{port}");
        settings.messages.push(Message { role: "user".into(), content: "oi".into() });
        let (sender, receiver) = std::sync::mpsc::channel();
        assert!(http_chat(&settings, &sender, &AtomicBool::new(false)).is_ok());
        let chunks = receiver.try_iter().filter_map(|event| match event {
            WorkerEvent::Chunk(text) => Some(text),
            _ => None,
        }).collect::<String>();
        assert_eq!(chunks, "ola mundo");
        server.join().unwrap();
    }
}
