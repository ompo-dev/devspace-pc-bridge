//! The real `Desktop` for Windows.
//!
//! * Windows are listed with `EnumWindows` (hidden, cloaked and tool windows skipped).
//! * Screenshots use `PrintWindow` with `PW_RENDERFULLCONTENT`, which renders one
//!   window even when other windows cover it. A minimized window cannot be drawn, and
//!   there is deliberately no fallback that copies from the screen: that would
//!   capture whatever is on top of the window, including apps that were never allowed.
//! * Input prefers UI Automation patterns (Invoke, Value, Toggle, Scroll...), and falls
//!   back to window messages posted straight to the target window. Neither moves the
//!   user's cursor, changes the foreground window or touches the real keyboard state.
//!   Nothing here calls `SendInput`.

use super::{
    ClickKind, ClickTarget, Desktop, DesktopResult, Element, Rect, Screenshot, WindowInfo,
};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, ImageFormat, RgbImage};
use std::{
    ffi::c_void,
    io::Cursor,
    mem::size_of,
    sync::{mpsc, Once},
    thread,
    time::Duration,
};
use uiautomation::{
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UIScrollPattern, UISelectionItemPattern,
        UITogglePattern, UIValuePattern,
    },
    types::{Handle, ScrollAmount},
    UIAutomation, UIElement, UITreeWalker,
};
use windows::{
    core::{BOOL, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::{
            Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED},
            Gdi::{
                CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
                GetDIBits, ReleaseDC, ScreenToClient, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
                BI_RGB, DIB_RGB_COLORS,
            },
        },
        Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId, OpenProcess,
                QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                MapVirtualKeyW, MAPVK_VK_TO_VSC, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE,
                VK_F1, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB,
                VK_UP,
            },
            WindowsAndMessaging::{
                ChildWindowFromPointEx, EnumChildWindows, EnumWindows, GetClassNameW,
                GetForegroundWindow, GetGUIThreadInfo, GetWindowLongW, GetWindowRect,
                GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
                IsWindowVisible, PostMessageW, SendMessageTimeoutW, SetForegroundWindow,
                CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, GUITHREADINFO, GWL_EXSTYLE, SMTO_ABORTIFHUNG,
                WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
                WM_SETTEXT, WS_EX_TOOLWINDOW,
            },
        },
    },
};

const UIA_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_PIXELS: i64 = 8000 * 8000;
const MAX_VALUE_CHARS: usize = 200;
const JPEG_QUALITY: u8 = 82;
const PW_RENDERFULLCONTENT: u32 = 2;
const MK_LBUTTON: usize = 1;
const MK_RBUTTON: usize = 2;
const WHEEL_DELTA: i32 = 120;
const EM_SETSEL: u32 = 0x00B1;
const EM_REPLACESEL: u32 = 0x00C2;
const BM_CLICK: u32 = 0x00F5;
const SEND_TIMEOUT_MS: u32 = 2000;
/// How long to wait for an app that grabs the focus asynchronously after an action.
const FOCUS_SETTLE: Duration = Duration::from_millis(200);

static DPI_AWARE: Once = Once::new();

/// Without this, window sizes and coordinates are scaled and screenshots come out blurry.
fn ensure_dpi_awareness() {
    DPI_AWARE.call_once(|| unsafe {
        // Already set by the host or a manifest: nothing to do.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
}

fn hwnd_of(id: u64) -> HWND {
    HWND(id as usize as *mut c_void)
}
fn id_of(hwnd: HWND) -> u64 {
    hwnd.0 as usize as u64
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<HWND>` handed to `EnumWindows` below.
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    BOOL(1)
}

fn top_level_windows() -> Vec<HWND> {
    let mut windows: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect_window),
            LPARAM(&mut windows as *mut _ as isize),
        );
    }
    windows
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..usize::try_from(copied).unwrap_or(0)])
    }
}

fn process_name(pid: u32) -> String {
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;
        let queried = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        let _ = CloseHandle(handle);
        if queried.is_err() {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        path.rsplit('\\').next().unwrap_or_default().to_owned()
    }
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
    }
}

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    Some(rect)
}

fn describe_window(hwnd: HWND, foreground: HWND) -> Option<WindowInfo> {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() || !IsWindowVisible(hwnd).as_bool() || is_cloaked(hwnd) {
            return None;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
            return None;
        }
        let title = window_title(hwnd);
        if title.is_empty() {
            return None;
        }
        let rect = window_rect(hwnd)?;
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        Some(WindowInfo {
            id: id_of(hwnd),
            title,
            process: process_name(pid),
            pid,
            rect: Rect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
            minimized: IsIconic(hwnd).as_bool(),
            foreground: hwnd == foreground,
        })
    }
}

/// This process and every ancestor (the terminal it runs in). Remote clients must never
/// reach them: the terminal is where pairing PINs and the on/off switch live.
pub fn protected_process_ids() -> Vec<u32> {
    let mut parents = std::collections::HashMap::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return vec![GetCurrentProcessId()];
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    let mut chain = vec![unsafe { GetCurrentProcessId() }];
    let mut current = chain[0];
    // The depth cap also guards against pid reuse forming a loop.
    for _ in 0..16 {
        match parents.get(&current) {
            Some(parent) if *parent != 0 && !chain.contains(parent) => {
                chain.push(*parent);
                current = *parent;
            }
            _ => break,
        }
    }
    chain
}

/// Runs a UI Automation job on its own thread. Some apps never return from `Invoke`
/// when it opens a modal dialog, and a stuck call must not freeze the server.
fn with_timeout<T: Send + 'static>(
    job: impl FnOnce() -> DesktopResult<T> + Send + 'static,
) -> DesktopResult<T> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(job());
    });
    receiver.recv_timeout(UIA_TIMEOUT).map_err(|_| {
        "A janela demorou demais para responder (talvez tenha aberto um diálogo).".to_owned()
    })?
}

fn automation() -> DesktopResult<UIAutomation> {
    UIAutomation::new().map_err(|e| format!("UI Automation indisponível: {e}"))
}

fn root_element(automation: &UIAutomation, id: u64) -> DesktopResult<UIElement> {
    let hwnd = hwnd_of(id);
    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return Err("Janela não encontrada".into());
    }
    automation
        .element_from_handle(Handle::from(hwnd))
        .map_err(|e| format!("Não consegui ler a janela: {e}"))
}

fn rect_of(element: &UIElement) -> Rect {
    element
        .get_bounding_rectangle()
        .map_or(Rect::default(), |r| Rect {
            left: r.get_left(),
            top: r.get_top(),
            right: r.get_right(),
            bottom: r.get_bottom(),
        })
}

fn actions_of(element: &UIElement) -> Vec<String> {
    let mut actions = Vec::new();
    if element.get_pattern::<UIInvokePattern>().is_ok() {
        actions.push("invoke".to_owned());
    }
    if element.get_pattern::<UITogglePattern>().is_ok() {
        actions.push("toggle".to_owned());
    }
    if element.get_pattern::<UISelectionItemPattern>().is_ok() {
        actions.push("select".to_owned());
    }
    if element.get_pattern::<UIExpandCollapsePattern>().is_ok() {
        actions.push("expand".to_owned());
    }
    if element.get_pattern::<UIValuePattern>().is_ok() {
        actions.push("value".to_owned());
    }
    if element.get_pattern::<UIScrollPattern>().is_ok() {
        actions.push("scroll".to_owned());
    }
    actions
}

fn value_of(element: &UIElement) -> Option<String> {
    if element.is_password().unwrap_or(false) {
        return Some("(campo de senha, oculto)".into());
    }
    let value = element
        .get_pattern::<UIValuePattern>()
        .ok()?
        .get_value()
        .ok()?;
    Some(value.chars().take(MAX_VALUE_CHARS).collect())
}

fn walk(
    walker: &UITreeWalker,
    parent: &UIElement,
    path: &str,
    depth: u8,
    max_depth: u8,
    max_nodes: usize,
    out: &mut Vec<Element>,
) {
    let Some(children) = walker.get_children(parent) else {
        return;
    };
    for (index, child) in children.iter().enumerate() {
        if out.len() >= max_nodes {
            return;
        }
        let id = if path.is_empty() {
            index.to_string()
        } else {
            format!("{path}.{index}")
        };
        if !child.is_offscreen().unwrap_or(false) {
            out.push(Element {
                id: id.clone(),
                role: child
                    .get_control_type()
                    .map_or_else(|_| "Unknown".into(), |kind| format!("{kind:?}")),
                name: child
                    .get_name()
                    .unwrap_or_default()
                    .chars()
                    .take(MAX_VALUE_CHARS)
                    .collect(),
                automation_id: child.get_automation_id().unwrap_or_default(),
                rect: rect_of(child),
                actions: actions_of(child),
                depth,
                value: value_of(child),
            });
        }
        if depth < max_depth {
            walk(walker, child, &id, depth + 1, max_depth, max_nodes, out);
        }
    }
}

/// Element ids are child-index paths from the window root, e.g. `0.3.1`.
fn resolve(automation: &UIAutomation, root: &UIElement, path: &str) -> DesktopResult<UIElement> {
    let walker = automation
        .get_control_view_walker()
        .map_err(|e| format!("UI Automation: {e}"))?;
    let mut current = root.clone();
    for part in path.split('.') {
        let index: usize = part.parse().map_err(|_| "element_id inválido".to_owned())?;
        current = walker
            .get_children(&current)
            .and_then(|children| children.into_iter().nth(index))
            .ok_or("Controle não encontrado. A janela mudou: rode desktop_inspect de novo.")?;
    }
    Ok(current)
}

fn activate(element: &UIElement) -> Option<&'static str> {
    if let Ok(pattern) = element.get_pattern::<UIInvokePattern>() {
        if pattern.invoke().is_ok() {
            return Some("invoke");
        }
    }
    if let Ok(pattern) = element.get_pattern::<UITogglePattern>() {
        if pattern.toggle().is_ok() {
            return Some("toggle");
        }
    }
    if let Ok(pattern) = element.get_pattern::<UISelectionItemPattern>() {
        if pattern.select().is_ok() {
            return Some("select");
        }
    }
    if let Ok(pattern) = element.get_pattern::<UIExpandCollapsePattern>() {
        if pattern.expand().is_ok() {
            return Some("expand");
        }
    }
    None
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)]).to_lowercase()
}
/// Win32 EDIT, RichEdit and the WinForms/WPF-hosted variants all have "edit" in the class.
fn is_edit_class(class: &str) -> bool {
    class.contains("edit")
}
fn is_button_class(class: &str) -> bool {
    class.contains("button")
}

/// The window handle behind a UI Automation element, if it has one of its own.
fn native_hwnd(element: &UIElement) -> Option<HWND> {
    let hwnd: HWND = element.get_native_window_handle().ok()?.into();
    (!hwnd.0.is_null()).then_some(hwnd)
}

unsafe extern "system" fn collect_child(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `&mut Vec<HWND>` handed to `EnumChildWindows` below.
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    BOOL(1)
}
/// First visible edit control inside a window, for "type into this window" requests.
fn first_edit_control(top: HWND) -> Option<HWND> {
    let mut children: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(
            Some(top),
            Some(collect_child),
            LPARAM(&mut children as *mut _ as isize),
        );
        children
            .into_iter()
            .find(|hwnd| IsWindowVisible(*hwnd).as_bool() && is_edit_class(&class_name(*hwnd)))
    }
}

/// `WM_SETTEXT` / `EM_REPLACESEL` write straight into an edit control without focusing
/// it, so the foreground window does not change. Marshalled across processes by Windows.
fn set_text_native(edit: HWND, text: &str, append: bool) -> DesktopResult<()> {
    let mut wide: Vec<u16> = text
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .encode_utf16()
        .collect();
    wide.push(0);
    let pointer = LPARAM(wide.as_ptr() as isize);
    let send = |message: u32, wparam: usize, lparam: LPARAM| unsafe {
        let mut result = 0usize;
        let answered = SendMessageTimeoutW(
            edit,
            message,
            WPARAM(wparam),
            lparam,
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            Some(&mut result),
        );
        (answered.0 != 0).then_some(result)
    };
    let sent = if append {
        let length = unsafe { GetWindowTextLengthW(edit) }.max(0) as usize;
        send(EM_SETSEL, length, LPARAM(length as isize))
            .and_then(|_| send(EM_REPLACESEL, 1, pointer))
    } else {
        send(WM_SETTEXT, 0, pointer)
    };
    sent.map(|_| ())
        .ok_or_else(|| "O campo não respondeu.".to_owned())
}

/// Puts the user's window back if the target app grabbed the foreground by itself
/// (some UI Automation providers do). It only reverts changes caused by the app that
/// was operated, never a window the user switched to on their own.
fn restore_foreground(previous: HWND) {
    unsafe {
        if SetForegroundWindow(previous).as_bool() {
            return;
        }
        // Not allowed to take the foreground directly: briefly share the input queue of
        // the current foreground thread, which permits it. No input is generated.
        let current_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let this_thread = GetCurrentThreadId();
        if current_thread != 0
            && current_thread != this_thread
            && AttachThreadInput(this_thread, current_thread, true).as_bool()
        {
            let _ = SetForegroundWindow(previous);
            let _ = AttachThreadInput(this_thread, current_thread, false);
        }
    }
}

fn guard_focus(
    target_pid: u32,
    action: impl FnOnce() -> DesktopResult<String>,
) -> DesktopResult<String> {
    let previous = unsafe { GetForegroundWindow() };
    let mut message = action()?;
    thread::sleep(FOCUS_SETTLE);
    let now = unsafe { GetForegroundWindow() };
    if previous.0.is_null() || now == previous {
        return Ok(message);
    }
    let mut now_pid = 0u32;
    unsafe { GetWindowThreadProcessId(now, Some(&mut now_pid)) };
    if now_pid == target_pid {
        restore_foreground(previous);
        let restored = unsafe { GetForegroundWindow() } == previous;
        message.push_str(if restored {
            " O app puxou o foco para si; devolvi o foco à janela anterior do usuário."
        } else {
            " AVISO: o app puxou o foco para si e não consegui devolvê-lo."
        });
    }
    Ok(message)
}

fn make_lparam(x: i32, y: i32) -> LPARAM {
    LPARAM((((y as u16 as u32) << 16) | (x as u16 as u32)) as i32 as isize)
}

fn post(hwnd: HWND, message: u32, wparam: usize, lparam: LPARAM) -> DesktopResult<()> {
    unsafe { PostMessageW(Some(hwnd), message, WPARAM(wparam), lparam) }
        .map_err(|e| format!("A janela não aceitou a mensagem: {e}"))
}

/// Deepest child window under a screen point.
fn window_at(top: HWND, screen: POINT) -> (HWND, POINT) {
    let mut current = top;
    let mut local = screen;
    unsafe {
        let _ = ScreenToClient(current, &mut local);
        for _ in 0..16 {
            let child =
                ChildWindowFromPointEx(current, local, CWP_SKIPINVISIBLE | CWP_SKIPDISABLED);
            if child.0.is_null() || child == current {
                break;
            }
            current = child;
            local = screen;
            let _ = ScreenToClient(current, &mut local);
        }
    }
    (current, local)
}

fn click_with_messages(id: u64, x: i32, y: i32, kind: ClickKind) -> DesktopResult<String> {
    let top = hwnd_of(id);
    let rect = window_rect(top).ok_or("Janela não encontrada")?;
    if x < 0 || y < 0 || x >= rect.right - rect.left || y >= rect.bottom - rect.top {
        return Err("Coordenadas fora da janela.".into());
    }
    let screen = POINT {
        x: rect.left + x,
        y: rect.top + y,
    };
    let (target, local) = window_at(top, screen);
    let lparam = make_lparam(local.x, local.y);
    post(target, WM_MOUSEMOVE, 0, lparam)?;
    match kind {
        ClickKind::Left => {
            post(target, WM_LBUTTONDOWN, MK_LBUTTON, lparam)?;
            post(target, WM_LBUTTONUP, 0, lparam)?;
        }
        ClickKind::Double => {
            post(target, WM_LBUTTONDOWN, MK_LBUTTON, lparam)?;
            post(target, WM_LBUTTONUP, 0, lparam)?;
            post(target, WM_LBUTTONDBLCLK, MK_LBUTTON, lparam)?;
            post(target, WM_LBUTTONUP, 0, lparam)?;
        }
        ClickKind::Right => {
            post(target, WM_RBUTTONDOWN, MK_RBUTTON, lparam)?;
            post(target, WM_RBUTTONUP, 0, lparam)?;
        }
    }
    Ok(format!(
        "Clique enviado por mensagem ao ponto ({x},{y}). Apps que ignoram mensagens de mouse não reagem; confira com uma captura."
    ))
}

/// The control that has keyboard focus *inside the target's own thread*. That state is
/// kept per thread, so it exists even while the window is in the background.
fn focus_window(top: HWND) -> HWND {
    unsafe {
        let thread_id = GetWindowThreadProcessId(top, None);
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread_id, &mut info).is_ok() && !info.hwndFocus.0.is_null() {
            return info.hwndFocus;
        }
    }
    top
}

fn virtual_key(key: &str) -> Option<(u16, bool)> {
    let key = key.to_ascii_lowercase();
    let plain = |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| Some((vk.0, false));
    let extended =
        |vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| Some((vk.0, true));
    match key.as_str() {
        "enter" => plain(VK_RETURN),
        "tab" => plain(VK_TAB),
        "escape" => plain(VK_ESCAPE),
        "backspace" => plain(VK_BACK),
        "space" => plain(VK_SPACE),
        "delete" => extended(VK_DELETE),
        "up" => extended(VK_UP),
        "down" => extended(VK_DOWN),
        "left" => extended(VK_LEFT),
        "right" => extended(VK_RIGHT),
        "home" => extended(VK_HOME),
        "end" => extended(VK_END),
        "pageup" => extended(VK_PRIOR),
        "pagedown" => extended(VK_NEXT),
        other => {
            let number: u16 = other.strip_prefix('f')?.parse().ok()?;
            (1..=10)
                .contains(&number)
                .then_some((VK_F1.0 + number - 1, false))
        }
    }
}

fn scale_down(img: RgbImage, max_edge: u32) -> RgbImage {
    let longest = img.width().max(img.height());
    if longest <= max_edge {
        return img;
    }
    let ratio = f64::from(max_edge) / f64::from(longest);
    let width = ((f64::from(img.width()) * ratio).round() as u32).max(1);
    let height = ((f64::from(img.height()) * ratio).round() as u32).max(1);
    image::imageops::resize(&img, width, height, FilterType::Triangle)
}

fn capture_window(hwnd: HWND, width: i32, height: i32) -> DesktopResult<RgbImage> {
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap.into());
        let drawn = PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        // GetDIBits requires the bitmap not to be selected into a device context.
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // negative: rows run top to bottom
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; width as usize * height as usize * 4];
        let lines = GetDIBits(
            screen,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr() as *mut c_void),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        if !drawn || lines == 0 {
            return Err("A janela não permitiu a captura.".into());
        }
        if bgra
            .chunks(4096)
            .all(|chunk| chunk.iter().all(|byte| *byte == 0))
        {
            return Err(
                "A captura saiu totalmente preta: este app não permite captura em segundo plano."
                    .into(),
            );
        }
        let rgb: Vec<u8> = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0]])
            .collect();
        RgbImage::from_raw(width as u32, height as u32, rgb)
            .ok_or_else(|| "Falha ao montar a imagem.".to_owned())
    }
}

pub struct WindowsDesktop;

impl WindowsDesktop {
    pub fn new() -> Self {
        ensure_dpi_awareness();
        Self
    }
}

impl Desktop for WindowsDesktop {
    fn list_windows(&self) -> DesktopResult<Vec<WindowInfo>> {
        let foreground = unsafe { GetForegroundWindow() };
        // Owned windows (Save dialogs, message boxes) are listed too: they are often
        // exactly what needs a click.
        Ok(top_level_windows()
            .into_iter()
            .filter_map(|hwnd| describe_window(hwnd, foreground))
            .collect())
    }

    fn window(&self, id: u64) -> DesktopResult<WindowInfo> {
        describe_window(hwnd_of(id), unsafe { GetForegroundWindow() })
            .ok_or_else(|| "Janela não encontrada".to_owned())
    }

    fn screenshot(&self, id: u64, max_edge: u32, jpeg: bool) -> DesktopResult<Screenshot> {
        let window = self.window(id)?;
        if window.minimized {
            return Err("A janela está minimizada e não pode ser capturada. Peça ao usuário para restaurá-la.".into());
        }
        let (width, height) = (window.rect.width(), window.rect.height());
        if width <= 0 || height <= 0 || i64::from(width) * i64::from(height) > MAX_PIXELS {
            return Err("Tamanho de janela fora do intervalo suportado.".into());
        }
        let image = scale_down(capture_window(hwnd_of(id), width, height)?, max_edge);
        let (out_width, out_height) = (image.width(), image.height());
        let mut data = Vec::new();
        if jpeg {
            JpegEncoder::new_with_quality(&mut data, JPEG_QUALITY)
                .encode_image(&image)
                .map_err(|e| format!("Falha ao codificar JPEG: {e}"))?;
        } else {
            image
                .write_to(&mut Cursor::new(&mut data), ImageFormat::Png)
                .map_err(|e| format!("Falha ao codificar PNG: {e}"))?;
        }
        Ok(Screenshot {
            data,
            mime: if jpeg { "image/jpeg" } else { "image/png" },
            width: out_width,
            height: out_height,
        })
    }

    fn inspect(&self, id: u64, max_depth: u8, max_nodes: usize) -> DesktopResult<Vec<Element>> {
        with_timeout(move || {
            let automation = automation()?;
            let root = root_element(&automation, id)?;
            let walker = automation
                .get_control_view_walker()
                .map_err(|e| format!("UI Automation: {e}"))?;
            let mut elements = Vec::new();
            walk(&walker, &root, "", 1, max_depth, max_nodes, &mut elements);
            Ok(elements)
        })
    }

    fn click(&self, id: u64, target: &ClickTarget, kind: ClickKind) -> DesktopResult<String> {
        let target = target.clone();
        let pid = self.window(id)?.pid;
        with_timeout(move || {
            guard_focus(pid, || match target {
                ClickTarget::Element(path) => {
                    let automation = automation()?;
                    let root = root_element(&automation, id)?;
                    let element = resolve(&automation, &root, &path)?;
                    if kind == ClickKind::Left {
                        // A standard button takes BM_CLICK without any focus change, and a
                        // posted message never blocks on a modal dialog it opens.
                        if let Some(button) =
                            native_hwnd(&element).filter(|h| is_button_class(&class_name(*h)))
                        {
                            post(button, BM_CLICK, 0, LPARAM(0))?;
                            return Ok("Botão acionado por mensagem de janela (sem mover o foco)."
                                .to_owned());
                        }
                        if let Some(method) = activate(&element) {
                            return Ok(format!("Controle acionado por UI Automation ({method})."));
                        }
                    }
                    // No pattern applies (or a right/double click was asked for): click the
                    // control's center with window messages.
                    let rect = rect_of(&element);
                    let window = window_rect(hwnd_of(id)).ok_or("Janela não encontrada")?;
                    let x = (rect.left + rect.right) / 2 - window.left;
                    let y = (rect.top + rect.bottom) / 2 - window.top;
                    click_with_messages(id, x, y, kind)
                }
                ClickTarget::Point { x, y } => click_with_messages(id, x, y, kind),
            })
        })
    }

    fn type_text(
        &self,
        id: u64,
        element: Option<&str>,
        text: &str,
        append: bool,
    ) -> DesktopResult<String> {
        let element_path = element.map(str::to_owned);
        let text = text.to_owned();
        let pid = self.window(id)?.pid;
        with_timeout(move || {
            guard_focus(pid, || {
                let top = hwnd_of(id);
                let automation = automation()?;
                let root = root_element(&automation, id)?;
                let element = match &element_path {
                    Some(path) => Some(resolve(&automation, &root, path)?),
                    None => None,
                };
                // 1. An edit control of its own: write to it with window messages, which
                //    never touches the focus.
                let focused = focus_window(top);
                let native_edit = element
                    .as_ref()
                    .and_then(native_hwnd)
                    .filter(|hwnd| is_edit_class(&class_name(*hwnd)))
                    .or_else(|| {
                        if element.is_some() {
                            return None;
                        }
                        (focused != top && is_edit_class(&class_name(focused)))
                            .then_some(focused)
                            .or_else(|| first_edit_control(top))
                    });
                if let Some(edit) = native_edit {
                    set_text_native(edit, &text, append)?;
                    return Ok(
                        "Texto enviado ao campo por mensagem de janela (sem mover o foco)."
                            .to_owned(),
                    );
                }
                // 2. A control without its own window: UI Automation. Some providers pull
                //    the focus to the app when they do this, which `guard_focus` undoes.
                if let Some(element) = &element {
                    if let Ok(pattern) = element.get_pattern::<UIValuePattern>() {
                        let new_value = if append {
                            format!("{}{}", pattern.get_value().unwrap_or_default(), text)
                        } else {
                            text.clone()
                        };
                        if pattern.set_value(&new_value).is_ok() {
                            return Ok("Texto definido no campo por UI Automation.".to_owned());
                        }
                    }
                }
                // 3. Characters posted to the control with focus in the target's own
                //    thread. This always appends at the caret.
                // A line break is a single carriage return for an edit control.
                for unit in text.replace("\r\n", "\n").encode_utf16() {
                    let unit = if unit == u16::from(b'\n') {
                        u16::from(b'\r')
                    } else {
                        unit
                    };
                    post(focused, WM_CHAR, usize::from(unit), LPARAM(1))?;
                }
                Ok(
                    "Texto enviado por mensagens de teclado ao controle com foco da janela."
                        .to_owned(),
                )
            })
        })
    }

    fn press_key(&self, id: u64, key: &str) -> DesktopResult<String> {
        let (vk, extended) = virtual_key(key).ok_or("Tecla não suportada")?;
        let pid = self.window(id)?.pid;
        guard_focus(pid, || {
            let destination = focus_window(hwnd_of(id));
            let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as isize;
            let ext = if extended { 1isize << 24 } else { 0 };
            let down = LPARAM(1 | (scan << 16) | ext);
            let up = LPARAM(1 | (scan << 16) | ext | (1 << 30) | (1 << 31));
            post(destination, WM_KEYDOWN, usize::from(vk), down)?;
            post(destination, WM_KEYUP, usize::from(vk), up)?;
            Ok(format!("Tecla {key} enviada à janela."))
        })
    }

    fn scroll(&self, id: u64, element: Option<&str>, dx: i32, dy: i32) -> DesktopResult<String> {
        let element_path = element.map(str::to_owned);
        let pid = self.window(id)?.pid;
        with_timeout(move || {
            guard_focus(pid, || {
                let automation = automation()?;
                let root = root_element(&automation, id)?;
                let scrollable = match &element_path {
                    Some(path) => Some(resolve(&automation, &root, path)?),
                    None => None,
                };
                if let Some(Ok(pattern)) = scrollable
                    .as_ref()
                    .map(|e| e.get_pattern::<UIScrollPattern>())
                {
                    let horizontal = match dx.signum() {
                        1 => ScrollAmount::SmallIncrement,
                        -1 => ScrollAmount::SmallDecrement,
                        _ => ScrollAmount::NoAmount,
                    };
                    // Positive dy means "up", which is a decrement of the vertical position.
                    let vertical = match dy.signum() {
                        1 => ScrollAmount::SmallDecrement,
                        -1 => ScrollAmount::SmallIncrement,
                        _ => ScrollAmount::NoAmount,
                    };
                    for _ in 0..dx.abs().max(dy.abs()) {
                        pattern
                            .scroll(horizontal, vertical)
                            .map_err(|e| format!("Rolagem recusada: {e}"))?;
                    }
                    return Ok("Rolagem feita por UI Automation.".to_owned());
                }
                let top = hwnd_of(id);
                let rect = window_rect(top).ok_or("Janela não encontrada")?;
                let (center_x, center_y) =
                    ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
                let (destination, _) = window_at(
                    top,
                    POINT {
                        x: center_x,
                        y: center_y,
                    },
                );
                let screen = make_lparam(center_x, center_y);
                if dy != 0 {
                    let delta = (dy * WHEEL_DELTA) as i16 as u16 as usize;
                    post(destination, WM_MOUSEWHEEL, delta << 16, screen)?;
                }
                if dx != 0 {
                    let delta = (dx * WHEEL_DELTA) as i16 as u16 as usize;
                    post(destination, WM_MOUSEHWHEEL, delta << 16, screen)?;
                }
                Ok("Rolagem enviada por mensagens de roda do mouse.".to_owned())
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_extended_keys_are_mapped() {
        assert_eq!(virtual_key("Enter"), Some((VK_RETURN.0, false)));
        assert_eq!(virtual_key("left"), Some((VK_LEFT.0, true)));
        assert_eq!(virtual_key("F1"), Some((VK_F1.0, false)));
        assert_eq!(virtual_key("f10"), Some((VK_F1.0 + 9, false)));
        assert_eq!(virtual_key("f11"), None);
        assert_eq!(virtual_key("ctrl"), None);
    }
    #[test]
    fn coordinates_pack_into_lparam() {
        assert_eq!(make_lparam(5, 7).0 & 0xFFFF, 5);
        assert_eq!((make_lparam(5, 7).0 >> 16) & 0xFFFF, 7);
    }
    #[test]
    fn images_are_scaled_by_their_longest_edge() {
        let image = RgbImage::new(2000, 1000);
        let scaled = scale_down(image, 1000);
        assert_eq!((scaled.width(), scaled.height()), (1000, 500));
        let small = scale_down(RgbImage::new(300, 200), 1000);
        assert_eq!((small.width(), small.height()), (300, 200));
    }
    #[test]
    fn the_process_tree_includes_this_process() {
        let chain = protected_process_ids();
        assert_eq!(chain[0], std::process::id());
        assert!(
            chain.len() >= 2,
            "the parent (terminal or test runner) must be protected too"
        );
    }
    #[test]
    fn this_process_can_list_windows_without_crashing() {
        // The window list depends on the session; it only has to work, not be non-empty.
        assert!(WindowsDesktop::new().list_windows().is_ok());
    }

    // ---- live test: needs a real desktop session, so it only runs on request ----
    //   cargo test live_ -- --ignored --nocapture

    use std::{
        fs, os::windows::process::CommandExt, path::PathBuf, process::Command, time::Instant,
    };
    use windows::Win32::UI::{
        Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
        WindowsAndMessaging::GetCursorPos,
    };

    const TARGET_SCRIPT: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition @'
using System.Windows.Forms;
public class QuietForm : Form { protected override bool ShowWithoutActivation { get { return true; } } }
'@
$form = New-Object QuietForm
$form.Text = $env:BRIDGE_TEST_TITLE
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(1480, 820)
$form.Size = New-Object System.Drawing.Size(400, 260)
$form.ShowInTaskbar = $false
$box = New-Object System.Windows.Forms.TextBox
$box.Multiline = $true; $box.Name = 'box'; $box.AccessibleName = 'campo'
$box.Location = New-Object System.Drawing.Point(10, 10); $box.Size = New-Object System.Drawing.Size(360, 100)
$btn = New-Object System.Windows.Forms.Button
$btn.Name = 'go'; $btn.Text = 'Gravar'; $btn.AccessibleName = 'Gravar'
$btn.Location = New-Object System.Drawing.Point(10, 120); $btn.Size = New-Object System.Drawing.Size(120, 40)
$btn.Add_Click({ Set-Content -LiteralPath $env:BRIDGE_TEST_OUT -Value ('CLICKED|' + $box.Text) -Encoding UTF8 })
$form.Controls.AddRange(@($box, $btn))
[System.Windows.Forms.Application]::Run($form)
"#;

    fn cursor() -> (i32, i32) {
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
        }
        (point.x, point.y)
    }
    /// Tick of the last REAL keyboard/mouse event. Posted messages and UI Automation do
    /// not touch it; `SendInput` would, so it proves no input was injected.
    fn last_input_tick() -> u32 {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        unsafe {
            let _ = GetLastInputInfo(&mut info);
        }
        info.dwTime
    }
    fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(20) {
            if let Some(found) = probe() {
                return found;
            }
            thread::sleep(Duration::from_millis(200));
        }
        panic!("tempo esgotado esperando: {what}");
    }

    #[test]
    #[ignore = "abre uma janela de teste no desktop por alguns segundos"]
    fn live_capture_and_input_do_not_disturb_the_user() {
        let desktop = WindowsDesktop::new();
        let unique = crate::clock::now_nanos();
        let title = format!("BridgeLiveTest-{unique}");
        let dir = std::env::temp_dir().join(format!("bridge-live-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        let script: PathBuf = dir.join("target.ps1");
        let output: PathBuf = dir.join("clicked.txt");
        fs::write(&script, TARGET_SCRIPT).unwrap();
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .env("BRIDGE_TEST_TITLE", &title)
            .env("BRIDGE_TEST_OUT", &output)
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("start the target window");

        let window = wait_for("a janela de teste aparecer", || {
            desktop
                .list_windows()
                .ok()?
                .into_iter()
                .find(|w| w.title == title)
        });
        // Let the form finish painting before measuring anything.
        thread::sleep(Duration::from_millis(1500));

        let cursor_before = cursor();
        let foreground_before = unsafe { GetForegroundWindow() };
        let input_before = last_input_tick();
        println!(
            "janela {} em {:?}; cursor {:?}",
            window.id, window.rect, cursor_before
        );

        let foreground_now = || {
            let hwnd = unsafe { GetForegroundWindow() };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            // Class and process only; titles of the user's own windows stay out of the output.
            format!(
                "{} [{}, visivel={}]",
                id_of(hwnd),
                process_name(pid),
                unsafe { IsWindowVisible(hwnd).as_bool() }
            )
        };
        println!(
            "foco antes: {} (janela de teste: {}, pid do teste: {})",
            foreground_now(),
            window.id,
            window.pid
        );

        // 1. Screenshot of this one window.
        let shot = desktop
            .screenshot(window.id, 800, true)
            .expect("screenshot");
        println!("foco apos captura: {}", foreground_now());
        assert_eq!(shot.mime, "image/jpeg");
        assert_eq!((shot.width, shot.height), (400, 260));
        let decoded = image::load_from_memory(&shot.data)
            .expect("a valid JPEG")
            .to_rgb8();
        let mut colors = std::collections::HashSet::new();
        for pixel in decoded.pixels().step_by(37) {
            colors.insert((pixel[0] / 16, pixel[1] / 16, pixel[2] / 16));
        }
        assert!(
            colors.len() >= 3,
            "the capture looks blank ({} colors)",
            colors.len()
        );
        fs::write(
            std::env::temp_dir().join("bridge-live-shot.jpg"),
            &shot.data,
        )
        .unwrap();
        let png = desktop
            .screenshot(window.id, 200, false)
            .expect("png screenshot");
        assert_eq!((png.mime, png.width), ("image/png", 200));

        // 2. Controls through UI Automation.
        let elements = desktop.inspect(window.id, 4, 100).expect("inspect");
        println!("foco apos inspecionar: {}", foreground_now());
        println!(
            "controles: {:?}",
            elements
                .iter()
                .map(|e| (&e.id, &e.role, &e.name, &e.actions))
                .collect::<Vec<_>>()
        );
        let edit = elements
            .iter()
            .find(|e| e.actions.contains(&"value".to_owned()) && e.role.contains("Edit"))
            .expect("the text box");
        let button = elements
            .iter()
            .find(|e| e.role.contains("Button") && e.name == "Gravar")
            .expect("the button");

        // 3. Type into the box, then press the button, neither with real input.
        let typed = desktop
            .type_text(window.id, Some(&edit.id), "Olá do bridge — ação", false)
            .expect("type");
        println!("digitar: {typed}");
        println!("foco apos digitar: {}", foreground_now());
        assert!(
            typed.contains("sem mover o foco"),
            "typing should use the native path: {typed}"
        );
        let clicked = desktop
            .click(
                window.id,
                &ClickTarget::Element(button.id.clone()),
                ClickKind::Left,
            )
            .expect("click");
        assert!(
            clicked.contains("sem mover o foco"),
            "clicking should use the native path: {clicked}"
        );
        println!("clicar: {clicked}");
        println!("foco apos clique UIA: {}", foreground_now());
        let written = wait_for("o clique gravar o arquivo", || {
            fs::read_to_string(&output)
                .ok()
                .filter(|t| t.contains("CLICKED"))
        });
        assert!(
            written.contains("Olá do bridge — ação"),
            "the text must reach the app: {written:?}"
        );

        // 4. A click by coordinates, which goes through window messages.
        fs::remove_file(&output).unwrap();
        let (x, y) = (
            (button.rect.left + button.rect.right) / 2 - window.rect.left,
            (button.rect.top + button.rect.bottom) / 2 - window.rect.top,
        );
        let by_point = desktop
            .click(window.id, &ClickTarget::Point { x, y }, ClickKind::Left)
            .expect("point click");
        println!("clicar por ponto: {by_point}");
        println!("foco apos clique por mensagem: {}", foreground_now());
        wait_for("o clique por mensagem gravar o arquivo", || {
            fs::read_to_string(&output)
                .ok()
                .filter(|t| t.contains("CLICKED"))
        });

        // 5. The point of all this: the user was not disturbed.
        let cursor_after = cursor();
        let foreground_after = unsafe { GetForegroundWindow() };
        let input_after = last_input_tick();
        // 6. The UI Automation fallback is known to make some apps grab the focus; the
        //    guard must hand it back to the user's window.
        let button_path = button.id.clone();
        let window_id = window.id;
        let guarded = guard_focus(window.pid, || {
            let automation = automation()?;
            let root = root_element(&automation, window_id)?;
            let element = resolve(&automation, &root, &button_path)?;
            activate(&element)
                .map(|method| format!("UIA {method}."))
                .ok_or_else(|| "sem padrão".to_owned())
        });
        thread::sleep(Duration::from_millis(400));
        let foreground_after_fallback = unsafe { GetForegroundWindow() };
        println!(
            "fallback UIA com guarda: {guarded:?}; foco: {}",
            foreground_now()
        );
        let _ = child.kill();
        let _ = child.wait();
        fs::remove_dir_all(&dir).ok();
        assert_eq!(
            foreground_after_fallback, foreground_before,
            "the guard did not give the focus back after the UIA fallback"
        );
        assert_eq!(cursor_after, cursor_before, "the real cursor moved");
        assert_eq!(
            foreground_after, foreground_before,
            "the foreground window changed"
        );
        assert_eq!(
            input_after, input_before,
            "real input events were generated (or the user touched the PC meanwhile)"
        );
    }
}
