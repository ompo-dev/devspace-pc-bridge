//! JSON-RPC side of the desktop MCP endpoint. Every tool goes through the same
//! gate: control must be on, the window's app must be allowed, the action must be
//! permitted and the rate limit respected, and the outcome is written to the audit
//! log. Nothing here talks to Windows directly; that is the `Desktop` backend.

use crate::desktop::{
    audit::{AuditEntry, AuditSink},
    is_supported_key,
    policy::{Action, Policy},
    ClickKind, ClickTarget, Desktop, DesktopResult, WindowInfo, SUPPORTED_KEYS,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard};

pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_TEXT_CHARS: usize = 4000;
const GENERIC_DENIAL: &str = "Janela não encontrada ou não liberada para controle remoto.";

pub struct McpContext {
    pub desktop: Arc<dyn Desktop>,
    pub policy: Arc<Mutex<Policy>>,
    pub audit: Arc<dyn AuditSink>,
}

impl McpContext {
    fn policy(&self) -> MutexGuard<'_, Policy> {
        self.policy
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn text_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}
fn error_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": true })
}
fn image_result(data: &[u8], mime: &str, caption: String) -> Value {
    json!({ "content": [
        { "type": "image", "data": STANDARD.encode(data), "mimeType": mime },
        { "type": "text", "text": caption },
    ] })
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

pub fn tool_definitions() -> Value {
    let window_id = json!({ "type": "integer", "description": "Valor window_id devolvido por desktop_list_windows." });
    let element_id =
        json!({ "type": "string", "description": "Valor id devolvido por desktop_inspect." });
    json!([
        {
            "name": "desktop_status",
            "title": "Estado do controle remoto",
            "description": "Diz se o controle remoto do PC está ligado, por quanto tempo, e o que é permitido. Chame antes de usar as outras ferramentas.",
            "inputSchema": schema(json!({}), &[]),
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "desktop_list_windows",
            "title": "Listar janelas liberadas",
            "description": "Lista as janelas abertas dos apps que o usuário liberou. Janelas de outros apps não aparecem.",
            "inputSchema": schema(json!({}), &[]),
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "desktop_screenshot",
            "title": "Captura de tela de uma janela",
            "description": "Captura uma única janela liberada, mesmo que esteja parcialmente coberta. Não captura a tela inteira.",
            "inputSchema": schema(json!({
                "window_id": window_id,
                "max_edge": { "type": "integer", "description": "Maior lado da imagem em pixels (320 a 2000, padrão 1280)." },
                "format": { "type": "string", "enum": ["jpeg", "png"], "description": "Padrão jpeg (menor)." }
            }), &["window_id"]),
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "desktop_inspect",
            "title": "Estrutura dos controles de uma janela",
            "description": "Lista botões, campos e textos da janela via UI Automation, com ids para usar em click, type e scroll. Mais confiável que adivinhar pixels.",
            "inputSchema": schema(json!({
                "window_id": window_id,
                "max_depth": { "type": "integer", "description": "Profundidade (1 a 8, padrão 4)." },
                "max_nodes": { "type": "integer", "description": "Máximo de controles (10 a 400, padrão 150)." }
            }), &["window_id"]),
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "desktop_click",
            "title": "Clicar em uma janela",
            "description": "Clica sem mover o mouse do usuário nem roubar o foco. Prefira element_id; x e y são pixels a partir do canto da janela.",
            "inputSchema": schema(json!({
                "window_id": window_id,
                "element_id": element_id,
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "button": { "type": "string", "enum": ["left", "right", "double"] }
            }), &["window_id"]),
            "annotations": { "readOnlyHint": false, "destructiveHint": true }
        },
        {
            "name": "desktop_type",
            "title": "Digitar texto em uma janela",
            "description": "Digita sem usar o teclado do usuário. Sem element_id, usa o campo que já tem foco na janela.",
            "inputSchema": schema(json!({
                "window_id": window_id,
                "text": { "type": "string", "description": "Até 4000 caracteres." },
                "element_id": element_id,
                "mode": { "type": "string", "enum": ["append", "replace"], "description": "Padrão append." }
            }), &["window_id", "text"]),
            "annotations": { "readOnlyHint": false, "destructiveHint": true }
        },
        {
            "name": "desktop_key",
            "title": "Pressionar uma tecla em uma janela",
            "description": format!("Envia uma tecla simples à janela. Combinações com Ctrl ou Alt não são suportadas. Teclas: {}.", SUPPORTED_KEYS.join(", ")),
            "inputSchema": schema(json!({
                "window_id": window_id,
                "key": { "type": "string", "enum": SUPPORTED_KEYS }
            }), &["window_id", "key"]),
            "annotations": { "readOnlyHint": false, "destructiveHint": true }
        },
        {
            "name": "desktop_scroll",
            "title": "Rolar o conteúdo de uma janela",
            "description": "Rola sem mover o mouse do usuário.",
            "inputSchema": schema(json!({
                "window_id": window_id,
                "element_id": element_id,
                "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                "amount": { "type": "integer", "description": "Passos da roda (1 a 20, padrão 3)." }
            }), &["window_id", "direction"]),
            "annotations": { "readOnlyHint": false, "destructiveHint": false }
        }
    ])
}

fn negotiate_version(request: &Value) -> &'static str {
    let asked = request
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|version| *version == asked)
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

fn initialize(request: &Value) -> Value {
    json!({
        "protocolVersion": negotiate_version(request),
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "devspace-pc-bridge-desktop", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Controle de janelas do PC do usuário, sempre limitado a apps que ele liberou. Comece por desktop_status. O usuário pode desligar o acesso a qualquer momento."
    })
}

/// Returns `None` for notifications, which get no JSON-RPC response.
pub fn handle(ctx: &McpContext, client: &str, request: &Value, now_ms: u64) -> Option<Value> {
    let id = request.get("id").cloned()?;
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = match method {
        "initialize" => Ok(initialize(request)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => Ok(call_tool(ctx, client, request.get("params"), now_ms)),
        _ => Err((-32601, "Method not found")),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

fn record(
    ctx: &McpContext,
    now_ms: u64,
    client: &str,
    tool: &str,
    window: Option<&WindowInfo>,
    outcome: &str,
    detail: &str,
) {
    ctx.audit.record(&AuditEntry::new(
        now_ms / 1000,
        client,
        tool,
        window.map_or("", |w| w.title.as_str()),
        window.map_or("", |w| w.process.as_str()),
        outcome,
        detail,
    ));
}

fn call_tool(ctx: &McpContext, client: &str, params: Option<&Value>, now_ms: u64) -> Value {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let empty = json!({});
    let args = params.and_then(|p| p.get("arguments")).unwrap_or(&empty);
    match name {
        "desktop_status" => status(ctx, now_ms),
        "desktop_list_windows" => list_windows(ctx, client, now_ms),
        "desktop_screenshot" => screenshot(ctx, client, args, now_ms),
        "desktop_inspect" => inspect(ctx, client, args, now_ms),
        "desktop_click" => click(ctx, client, args, now_ms),
        "desktop_type" => type_text(ctx, client, args, now_ms),
        "desktop_key" => press_key(ctx, client, args, now_ms),
        "desktop_scroll" => scroll(ctx, client, args, now_ms),
        _ => error_result("Ferramenta desconhecida."),
    }
}

fn status(ctx: &McpContext, now_ms: u64) -> Value {
    let policy = ctx.policy();
    let config = policy.config();
    let body = json!({
        "controle_ligado": policy.is_enabled(now_ms / 1000),
        "segundos_restantes": policy.remaining_secs(now_ms / 1000),
        "capturas_e_inspecao": config.screenshots,
        "teclado_e_mouse": config.input,
        "apps_liberados": config.allowed_apps,
        "limite_por_minuto": config.max_actions_per_minute,
        "observacao": "Cliques e teclas vão direto à janela, sem mover o mouse do usuário. Apps que leem entrada bruta (jogos, alguns Electron/UWP) podem ignorá-los.",
    });
    text_result(serde_json::to_string_pretty(&body).unwrap_or_default())
}

fn list_windows(ctx: &McpContext, client: &str, now_ms: u64) -> Value {
    if let Err(reason) = ctx.policy().check_session(now_ms) {
        record(
            ctx,
            now_ms,
            client,
            "desktop_list_windows",
            None,
            "denied",
            &reason,
        );
        return error_result(reason);
    }
    let windows = match ctx.desktop.list_windows() {
        Ok(windows) => windows,
        Err(err) => {
            record(
                ctx,
                now_ms,
                client,
                "desktop_list_windows",
                None,
                "error",
                &err,
            );
            return error_result(err);
        }
    };
    let visible: Vec<Value> = {
        let policy = ctx.policy();
        windows
            .iter()
            .filter(|window| policy.visible_to_remote(window))
            .map(|window| {
                json!({
                    "window_id": window.id,
                    "title": window.title,
                    "process": window.process,
                    "minimized": window.minimized,
                    "foreground": window.foreground,
                    "x": window.rect.left,
                    "y": window.rect.top,
                    "width": window.rect.width(),
                    "height": window.rect.height(),
                })
            })
            .collect()
    };
    record(
        ctx,
        now_ms,
        client,
        "desktop_list_windows",
        None,
        "ok",
        &format!("{} janelas", visible.len()),
    );
    text_result(serde_json::to_string_pretty(&visible).unwrap_or_default())
}

/// The shared gate for every tool that targets one window.
#[allow(clippy::too_many_arguments)] // Keep the shared policy check beside each window action.
fn guarded(
    ctx: &McpContext,
    client: &str,
    tool: &str,
    args: &Value,
    action: Action,
    now_ms: u64,
    detail: &str,
    run: impl FnOnce(&dyn Desktop, &WindowInfo) -> DesktopResult<Value>,
) -> Value {
    let Some(id) = args.get("window_id").and_then(Value::as_u64) else {
        return error_result("O argumento window_id é obrigatório.");
    };
    let Ok(window) = ctx.desktop.window(id) else {
        record(
            ctx,
            now_ms,
            client,
            tool,
            None,
            "denied",
            "janela inexistente",
        );
        return error_result(GENERIC_DENIAL);
    };
    let (verdict, visible) = {
        let mut policy = ctx.policy();
        (
            policy.check(now_ms, &window, action),
            policy.visible_to_remote(&window),
        )
    };
    if let Err(reason) = verdict {
        record(ctx, now_ms, client, tool, Some(&window), "denied", &reason);
        // A window the chat was never allowed to see must not be confirmed or described.
        return error_result(if visible {
            reason
        } else {
            GENERIC_DENIAL.into()
        });
    }
    match run(&*ctx.desktop, &window) {
        Ok(result) => {
            record(ctx, now_ms, client, tool, Some(&window), "ok", detail);
            result
        }
        Err(err) => {
            record(ctx, now_ms, client, tool, Some(&window), "error", &err);
            error_result(err)
        }
    }
}

fn clamped(args: &Value, key: &str, default: u64, min: u64, max: u64) -> u64 {
    args.get(key)
        .and_then(Value::as_u64)
        .unwrap_or(default)
        .clamp(min, max)
}
fn text_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn screenshot(ctx: &McpContext, client: &str, args: &Value, now_ms: u64) -> Value {
    let max_edge = clamped(args, "max_edge", 1280, 320, 2000) as u32;
    let jpeg = args.get("format").and_then(Value::as_str) != Some("png");
    guarded(
        ctx,
        client,
        "desktop_screenshot",
        args,
        Action::View,
        now_ms,
        &format!("max_edge={max_edge}"),
        |desktop, window| {
            let shot = desktop.screenshot(window.id, max_edge, jpeg)?;
            Ok(image_result(
                &shot.data,
                shot.mime,
                format!(
                    "Janela \"{}\" ({}), imagem {}x{} px.",
                    window.title, window.process, shot.width, shot.height
                ),
            ))
        },
    )
}

fn inspect(ctx: &McpContext, client: &str, args: &Value, now_ms: u64) -> Value {
    let depth = clamped(args, "max_depth", 4, 1, 8) as u8;
    let nodes = clamped(args, "max_nodes", 150, 10, 400) as usize;
    guarded(
        ctx,
        client,
        "desktop_inspect",
        args,
        Action::View,
        now_ms,
        &format!("depth={depth} nodes={nodes}"),
        |desktop, window| {
            let elements = desktop.inspect(window.id, depth, nodes)?;
            Ok(text_result(
                serde_json::to_string_pretty(&elements).unwrap_or_default(),
            ))
        },
    )
}

fn click(ctx: &McpContext, client: &str, args: &Value, now_ms: u64) -> Value {
    let target = if let Some(element) = text_arg(args, "element_id") {
        ClickTarget::Element(element.to_owned())
    } else if let (Some(x), Some(y)) = (
        args.get("x").and_then(Value::as_i64),
        args.get("y").and_then(Value::as_i64),
    ) {
        let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
            return error_result("Coordenadas fora do intervalo.");
        };
        ClickTarget::Point { x, y }
    } else {
        return error_result("Informe element_id, ou x e y.");
    };
    let kind = match args.get("button").and_then(Value::as_str) {
        Some("right") => ClickKind::Right,
        Some("double") => ClickKind::Double,
        _ => ClickKind::Left,
    };
    let detail = match &target {
        ClickTarget::Element(id) => format!("element={id} {kind:?}"),
        ClickTarget::Point { x, y } => format!("ponto=({x},{y}) {kind:?}"),
    };
    guarded(
        ctx,
        client,
        "desktop_click",
        args,
        Action::Input,
        now_ms,
        &detail,
        |desktop, window| desktop.click(window.id, &target, kind).map(text_result),
    )
}

fn type_text(ctx: &McpContext, client: &str, args: &Value, now_ms: u64) -> Value {
    let Some(text) = args.get("text").and_then(Value::as_str) else {
        return error_result("O argumento text é obrigatório.");
    };
    if text.chars().count() > MAX_TEXT_CHARS {
        return error_result(format!("O texto passa de {MAX_TEXT_CHARS} caracteres."));
    }
    let append = args.get("mode").and_then(Value::as_str) != Some("replace");
    let element = text_arg(args, "element_id");
    // Only the length is logged, never what was typed.
    let detail = format!(
        "{} caracteres, {}",
        text.chars().count(),
        if append { "append" } else { "replace" }
    );
    guarded(
        ctx,
        client,
        "desktop_type",
        args,
        Action::Input,
        now_ms,
        &detail,
        |desktop, window| {
            desktop
                .type_text(window.id, element, text, append)
                .map(text_result)
        },
    )
}

fn press_key(ctx: &McpContext, client: &str, args: &Value, now_ms: u64) -> Value {
    let Some(key) = text_arg(args, "key") else {
        return error_result("O argumento key é obrigatório.");
    };
    if !is_supported_key(key) {
        return error_result(format!(
            "Tecla não suportada. Use uma destas: {}.",
            SUPPORTED_KEYS.join(", ")
        ));
    }
    guarded(
        ctx,
        client,
        "desktop_key",
        args,
        Action::Input,
        now_ms,
        key,
        |desktop, window| {
            desktop
                .press_key(window.id, &key.to_ascii_lowercase())
                .map(text_result)
        },
    )
}

fn scroll(ctx: &McpContext, client: &str, args: &Value, now_ms: u64) -> Value {
    let amount = clamped(args, "amount", 3, 1, 20) as i32;
    let (dx, dy) = match args.get("direction").and_then(Value::as_str) {
        Some("up") => (0, amount),
        Some("down") => (0, -amount),
        Some("left") => (-amount, 0),
        Some("right") => (amount, 0),
        _ => return error_result("direction deve ser up, down, left ou right."),
    };
    let element = text_arg(args, "element_id");
    guarded(
        ctx,
        client,
        "desktop_scroll",
        args,
        Action::Input,
        now_ms,
        &format!("dx={dx} dy={dy}"),
        |desktop, window| desktop.scroll(window.id, element, dx, dy).map(text_result),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::{audit::MemoryAudit, policy::PolicyConfig, Element, Rect, Screenshot};

    #[derive(Default)]
    struct FakeDesktop {
        calls: Mutex<Vec<String>>,
    }
    impl FakeDesktop {
        fn windows() -> Vec<WindowInfo> {
            let make = |id, title: &str, process: &str, pid| WindowInfo {
                id,
                title: title.into(),
                process: process.into(),
                pid,
                rect: Rect {
                    left: 10,
                    top: 20,
                    right: 410,
                    bottom: 320,
                },
                minimized: false,
                foreground: id == 1,
            };
            vec![
                make(1, "Notas - Bloco", "notepad.exe", 100),
                make(2, "Caixa de entrada", "chrome.exe", 200),
                make(3, "Cofre de senhas", "notepad.exe", 100),
                make(4, "Terminal", "windowsterminal.exe", 300),
            ]
        }
        fn log(&self, call: impl Into<String>) {
            self.calls.lock().unwrap().push(call.into());
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl Desktop for FakeDesktop {
        fn list_windows(&self) -> DesktopResult<Vec<WindowInfo>> {
            Ok(Self::windows())
        }
        fn screenshot(&self, id: u64, max_edge: u32, jpeg: bool) -> DesktopResult<Screenshot> {
            self.log(format!("screenshot {id} {max_edge} {jpeg}"));
            Ok(Screenshot {
                data: vec![1, 2, 3],
                mime: "image/jpeg",
                width: 400,
                height: 300,
            })
        }
        fn inspect(&self, id: u64, depth: u8, nodes: usize) -> DesktopResult<Vec<Element>> {
            self.log(format!("inspect {id} {depth} {nodes}"));
            Ok(vec![Element {
                id: "0.1".into(),
                role: "Button".into(),
                name: "Salvar".into(),
                automation_id: "save".into(),
                rect: Rect::default(),
                actions: vec!["invoke".into()],
                depth: 1,
                value: None,
            }])
        }
        fn click(&self, id: u64, target: &ClickTarget, kind: ClickKind) -> DesktopResult<String> {
            self.log(format!("click {id} {target:?} {kind:?}"));
            Ok("clicou".into())
        }
        fn type_text(
            &self,
            id: u64,
            element: Option<&str>,
            text: &str,
            append: bool,
        ) -> DesktopResult<String> {
            self.log(format!("type {id} {element:?} {text} {append}"));
            Ok("digitou".into())
        }
        fn press_key(&self, id: u64, key: &str) -> DesktopResult<String> {
            self.log(format!("key {id} {key}"));
            Ok("tecla".into())
        }
        fn scroll(
            &self,
            id: u64,
            element: Option<&str>,
            dx: i32,
            dy: i32,
        ) -> DesktopResult<String> {
            self.log(format!("scroll {id} {element:?} {dx} {dy}"));
            Ok("rolou".into())
        }
    }

    const NOW_MS: u64 = 1_000_000;

    fn context(enabled: bool, input: bool) -> (McpContext, Arc<FakeDesktop>, Arc<MemoryAudit>) {
        let config = PolicyConfig {
            allowed_apps: vec!["notepad.exe".into(), "windowsterminal.exe".into()],
            input,
            ..PolicyConfig::default()
        };
        let mut policy = Policy::new(config, vec![]);
        if enabled {
            policy.enable(NOW_MS / 1000, 30);
        }
        let desktop = Arc::new(FakeDesktop::default());
        let audit = Arc::new(MemoryAudit::default());
        let ctx = McpContext {
            desktop: desktop.clone(),
            policy: Arc::new(Mutex::new(policy)),
            audit: audit.clone(),
        };
        (ctx, desktop, audit)
    }
    fn call(ctx: &McpContext, name: &str, arguments: Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}});
        handle(ctx, "ChatGPT", &request, NOW_MS).unwrap()["result"].clone()
    }
    fn text_of(result: &Value) -> String {
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }
    fn is_error(result: &Value) -> bool {
        result["isError"] == true
    }

    #[test]
    fn initialize_negotiates_the_protocol_and_notifications_get_no_reply() {
        let (ctx, ..) = context(false, false);
        let request = json!({"jsonrpc": "2.0", "id": 7, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}});
        let reply = handle(&ctx, "c", &request, NOW_MS).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["protocolVersion"], "2025-03-26");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        let unknown = json!({"jsonrpc": "2.0", "id": 8, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}});
        assert_eq!(
            handle(&ctx, "c", &unknown, NOW_MS).unwrap()["result"]["protocolVersion"],
            PROTOCOL_VERSIONS[0]
        );
        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(handle(&ctx, "c", &note, NOW_MS).is_none());
    }
    #[test]
    fn unknown_methods_are_json_rpc_errors() {
        let (ctx, ..) = context(false, false);
        let reply = handle(
            &ctx,
            "c",
            &json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}),
            NOW_MS,
        )
        .unwrap();
        assert_eq!(reply["error"]["code"], -32601);
        assert_eq!(
            handle(&ctx, "c", &json!({"id": 2, "method": "ping"}), NOW_MS).unwrap()["result"],
            json!({})
        );
    }
    #[test]
    fn every_tool_is_declared_with_a_schema() {
        let tools = tool_definitions();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "desktop_status",
                "desktop_list_windows",
                "desktop_screenshot",
                "desktop_inspect",
                "desktop_click",
                "desktop_type",
                "desktop_key",
                "desktop_scroll"
            ]
        );
        for tool in tools.as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"].as_str().unwrap().len() > 10);
        }
    }
    #[test]
    fn status_works_even_when_control_is_off() {
        let (ctx, ..) = context(false, false);
        let result = call(&ctx, "desktop_status", json!({}));
        assert!(!is_error(&result));
        assert!(text_of(&result).contains("\"controle_ligado\": false"));
    }
    #[test]
    fn everything_else_is_refused_while_control_is_off() {
        let (ctx, desktop, audit) = context(false, true);
        for (tool, args) in [
            ("desktop_list_windows", json!({})),
            ("desktop_screenshot", json!({"window_id": 1})),
            ("desktop_click", json!({"window_id": 1, "x": 5, "y": 5})),
        ] {
            let result = call(&ctx, tool, args);
            assert!(is_error(&result), "{tool}");
            assert!(text_of(&result).contains("desligado"), "{tool}");
        }
        assert!(
            desktop.calls().is_empty(),
            "no desktop call may happen while off"
        );
        assert!(audit
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|entry| entry.outcome == "denied"));
    }
    #[test]
    fn listing_shows_only_allowed_visible_windows() {
        let (ctx, ..) = context(true, false);
        let listing = text_of(&call(&ctx, "desktop_list_windows", json!({})));
        assert!(listing.contains("Notas - Bloco"));
        assert!(
            !listing.contains("Caixa de entrada"),
            "chrome was never allowed"
        );
        assert!(!listing.contains("Cofre"), "sensitive titles stay hidden");
        assert!(
            !listing.contains("Terminal"),
            "terminals can never be allowed"
        );
        assert!(listing.contains("\"width\": 400"));
    }
    #[test]
    fn screenshot_returns_an_image_block() {
        let (ctx, desktop, _) = context(true, false);
        let result = call(
            &ctx,
            "desktop_screenshot",
            json!({"window_id": 1, "max_edge": 99999, "format": "png"}),
        );
        assert_eq!(result["content"][0]["type"], "image");
        assert_eq!(result["content"][0]["data"], "AQID");
        assert_eq!(result["content"][0]["mimeType"], "image/jpeg");
        assert!(text_of(&json!({"content": [result["content"][1].clone()]})).contains("400x300"));
        assert_eq!(desktop.calls(), ["screenshot 1 2000 false"]);
    }
    #[test]
    fn windows_outside_the_allowlist_are_not_confirmed_or_described() {
        let (ctx, desktop, audit) = context(true, true);
        for id in [2u64, 3, 4, 999] {
            let result = call(&ctx, "desktop_screenshot", json!({"window_id": id}));
            assert!(is_error(&result), "window {id}");
            assert_eq!(text_of(&result), GENERIC_DENIAL, "window {id}");
        }
        assert!(desktop.calls().is_empty());
        let log = audit.0.lock().unwrap();
        assert_eq!(log.len(), 4);
        // The audit log keeps the real reason even though the chat is told nothing.
        assert!(log.iter().any(|entry| entry.detail.contains("chrome.exe")));
        assert!(log.iter().any(|entry| entry.detail.contains("inexistente")));
    }
    #[test]
    fn input_needs_the_input_switch() {
        let (view_only, desktop, _) = context(true, false);
        let denied = call(
            &view_only,
            "desktop_click",
            json!({"window_id": 1, "x": 5, "y": 6}),
        );
        assert!(is_error(&denied));
        assert!(text_of(&denied).contains("Teclado e mouse"));
        assert!(desktop.calls().is_empty());
        let (full, desktop, _) = context(true, true);
        assert!(!is_error(&call(
            &full,
            "desktop_click",
            json!({"window_id": 1, "x": 5, "y": 6, "button": "double"})
        )));
        assert!(!is_error(&call(
            &full,
            "desktop_click",
            json!({"window_id": 1, "element_id": "0.1"})
        )));
        assert_eq!(
            desktop.calls(),
            [
                "click 1 Point { x: 5, y: 6 } Double",
                "click 1 Element(\"0.1\") Left"
            ]
        );
    }
    #[test]
    fn typed_text_reaches_the_window_but_never_the_audit_log() {
        let (ctx, desktop, audit) = context(true, true);
        let secret = "minha-senha-123";
        let result = call(
            &ctx,
            "desktop_type",
            json!({"window_id": 1, "text": secret, "mode": "replace", "element_id": "0.2"}),
        );
        assert!(!is_error(&result));
        assert_eq!(
            desktop.calls(),
            [format!("type 1 Some(\"0.2\") {secret} false")]
        );
        let log = audit.0.lock().unwrap();
        let line = serde_json::to_string(&log[0]).unwrap();
        assert!(
            !line.contains(secret),
            "typed text leaked into the audit log: {line}"
        );
        assert!(log[0].detail.contains("15 caracteres"));
    }
    #[test]
    fn arguments_are_validated() {
        let (ctx, desktop, _) = context(true, true);
        assert!(is_error(&call(
            &ctx,
            "desktop_click",
            json!({"window_id": 1})
        )));
        assert!(is_error(&call(
            &ctx,
            "desktop_click",
            json!({"window_id": 1, "x": 5})
        )));
        assert!(is_error(&call(
            &ctx,
            "desktop_click",
            json!({"x": 5, "y": 5})
        )));
        assert!(is_error(&call(
            &ctx,
            "desktop_type",
            json!({"window_id": 1})
        )));
        assert!(is_error(&call(
            &ctx,
            "desktop_type",
            json!({"window_id": 1, "text": "x".repeat(MAX_TEXT_CHARS + 1)})
        )));
        assert!(is_error(&call(
            &ctx,
            "desktop_key",
            json!({"window_id": 1, "key": "ctrl+s"})
        )));
        assert!(is_error(&call(
            &ctx,
            "desktop_scroll",
            json!({"window_id": 1, "direction": "sideways"})
        )));
        assert!(is_error(&call(&ctx, "desktop_nope", json!({}))));
        assert!(desktop.calls().is_empty());
    }
    #[test]
    fn keys_scroll_and_inspect_are_forwarded() {
        let (ctx, desktop, _) = context(true, true);
        assert!(!is_error(&call(
            &ctx,
            "desktop_key",
            json!({"window_id": 1, "key": "Enter"})
        )));
        assert!(!is_error(&call(
            &ctx,
            "desktop_scroll",
            json!({"window_id": 1, "direction": "down", "amount": 500})
        )));
        let inspected = call(
            &ctx,
            "desktop_inspect",
            json!({"window_id": 1, "max_depth": 99, "max_nodes": 1}),
        );
        assert!(text_of(&inspected).contains("Salvar"));
        assert_eq!(
            desktop.calls(),
            ["key 1 enter", "scroll 1 None 0 -20", "inspect 1 8 10"]
        );
    }
    #[test]
    fn the_rate_limit_stops_a_runaway_client() {
        let (ctx, desktop, _) = context(true, false);
        ctx.policy().config_mut().max_actions_per_minute = 3;
        let results: Vec<bool> = (0..5)
            .map(|_| is_error(&call(&ctx, "desktop_screenshot", json!({"window_id": 1}))))
            .collect();
        assert_eq!(results, [false, false, false, true, true]);
        assert_eq!(desktop.calls().len(), 3);
    }
}
