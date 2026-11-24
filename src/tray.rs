//! Native Windows notification icon. Only local, deliberate menu actions can
//! reveal the terminal, disable remote desktop or close the bridge.

use crate::setup::TaskEvent;
use std::{
    ffi::c_void,
    sync::{
        mpsc::{self, Sender},
        OnceLock,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::{
    core::w,
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::{Console::GetConsoleWindow, LibraryLoader::GetModuleHandleW},
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
                DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, LoadIconW,
                PostMessageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, ShowWindow,
                TrackPopupMenu, TranslateMessage, IDI_APPLICATION, MF_STRING, MSG, SW_HIDE,
                SW_SHOW, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE,
                WINDOW_STYLE, WM_APP, WM_CLOSE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_RBUTTONUP,
                WNDCLASSW,
            },
        },
    },
};

const TRAY_MESSAGE: u32 = WM_APP + 20;
const CMD_SHOW: usize = 1001;
const CMD_DISABLE: usize = 1002;
const CMD_QUIT: usize = 1003;
static EVENTS: OnceLock<Sender<TaskEvent>> = OnceLock::new();

pub fn show_console() {
    unsafe {
        let console = GetConsoleWindow();
        if !console.is_invalid() {
            let _ = ShowWindow(console, SW_SHOW);
            let _ = SetForegroundWindow(console);
        }
    }
}

pub fn hide_console() -> Result<(), String> {
    let console = unsafe { GetConsoleWindow() };
    if console.is_invalid() {
        return Err("Este terminal nao oferece uma janela Win32 para ocultar.".into());
    }
    unsafe {
        let _ = ShowWindow(console, SW_HIDE);
    }
    Ok(())
}

fn emit(event: TaskEvent) {
    if let Some(tx) = EVENTS.get() {
        let _ = tx.send(event);
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, _w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        TRAY_MESSAGE if l.0 as u32 == WM_LBUTTONDBLCLK => {
            show_console();
            LRESULT(0)
        }
        TRAY_MESSAGE if l.0 as u32 == WM_RBUTTONUP => {
            if let Ok(menu) = unsafe { CreatePopupMenu() } {
                unsafe {
                    let _ = AppendMenuW(menu, MF_STRING, CMD_SHOW, w!("Mostrar terminal"));
                    let _ =
                        AppendMenuW(menu, MF_STRING, CMD_DISABLE, w!("Desligar desktop remoto"));
                    let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, w!("Sair do bridge"));
                    let mut point = POINT::default();
                    let _ = GetCursorPos(&mut point);
                    let _ = SetForegroundWindow(hwnd);
                    let choice = TrackPopupMenu(
                        menu,
                        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                        point.x,
                        point.y,
                        None,
                        hwnd,
                        None,
                    );
                    match choice.0 as usize {
                        CMD_SHOW => show_console(),
                        CMD_DISABLE => emit(TaskEvent::TrayDisable),
                        CMD_QUIT => emit(TaskEvent::TrayQuit),
                        _ => {}
                    }
                    let _ = DestroyMenu(menu);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, _w, l) },
    }
}

fn serve(ready: Sender<Result<usize, String>>) {
    let outcome = (|| -> Result<(HWND, NOTIFYICONDATAW), String> {
        let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
        let instance = HINSTANCE(module.0);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("DevSpacePCBridgeTray"),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err("Nao foi possivel criar classe da bandeja.".into());
        }
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("DevSpacePCBridgeTray"),
                w!("DevSpace PC Bridge"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|e| e.to_string())?;
        let mut icon = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: TRAY_MESSAGE,
            hIcon: unsafe { LoadIconW(None, IDI_APPLICATION) }.map_err(|e| e.to_string())?,
            ..Default::default()
        };
        for (dest, ch) in icon
            .szTip
            .iter_mut()
            .zip("DevSpace PC Bridge".encode_utf16())
        {
            *dest = ch;
        }
        if !unsafe { Shell_NotifyIconW(NIM_ADD, &icon) }.as_bool() {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return Err("Windows recusou icone da bandeja.".into());
        }
        Ok((hwnd, icon))
    })();
    match outcome {
        Ok((hwnd, icon)) => {
            let _ = ready.send(Ok(hwnd.0 as usize));
            let mut msg = MSG::default();
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
            }
        }
        Err(err) => {
            let _ = ready.send(Err(err));
        }
    }
}

pub struct Tray {
    hwnd: usize,
    worker: Option<JoinHandle<()>>,
}
impl Tray {
    pub fn start(tx: Sender<TaskEvent>) -> Result<Self, String> {
        let _ = EVENTS.set(tx);
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = thread::spawn(move || serve(ready_tx));
        let hwnd = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())??;
        Ok(Self {
            hwnd,
            worker: Some(worker),
        })
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut c_void)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
