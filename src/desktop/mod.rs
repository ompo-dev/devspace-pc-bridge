//! What a remote chat may do on this PC: see windows and operate them without
//! taking over the real mouse or keyboard.
//!
//! Windows has a single cursor and a single input focus, so a true "second mouse"
//! does not exist. Instead input goes to one window through UI Automation or window
//! messages, which neither move the user's cursor nor steal focus. Apps that read
//! raw input (games, some Electron/UWP views) ignore that kind of input.

pub mod audit;
pub mod policy;
#[cfg(windows)]
pub mod windows_backend;

use serde::Serialize;

pub type DesktopResult<T> = Result<T, String>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub process: String,
    pub pid: u32,
    pub rect: Rect,
    pub minimized: bool,
    pub foreground: bool,
}

pub struct Screenshot {
    pub data: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize)]
pub struct Element {
    pub id: String,
    pub role: String,
    pub name: String,
    pub automation_id: String,
    pub rect: Rect,
    pub actions: Vec<String>,
    pub depth: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClickTarget {
    Element(String),
    /// Pixels relative to the window's top-left corner.
    Point {
        x: i32,
        y: i32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickKind {
    Left,
    Right,
    Double,
}

pub trait Desktop: Send + Sync {
    fn list_windows(&self) -> DesktopResult<Vec<WindowInfo>>;
    fn window(&self, id: u64) -> DesktopResult<WindowInfo> {
        self.list_windows()?
            .into_iter()
            .find(|window| window.id == id)
            .ok_or_else(|| "Janela não encontrada".to_owned())
    }
    fn screenshot(&self, id: u64, max_edge: u32, jpeg: bool) -> DesktopResult<Screenshot>;
    fn inspect(&self, id: u64, max_depth: u8, max_nodes: usize) -> DesktopResult<Vec<Element>>;
    fn click(&self, id: u64, target: &ClickTarget, kind: ClickKind) -> DesktopResult<String>;
    fn type_text(
        &self,
        id: u64,
        element: Option<&str>,
        text: &str,
        append: bool,
    ) -> DesktopResult<String>;
    fn press_key(&self, id: u64, key: &str) -> DesktopResult<String>;
    fn scroll(&self, id: u64, element: Option<&str>, dx: i32, dy: i32) -> DesktopResult<String>;
}

/// Keys that can be sent as plain key messages. Combinations with Ctrl/Alt need the
/// real keyboard state, which messages cannot fake, so they are not offered.
pub const SUPPORTED_KEYS: [&str; 24] = [
    "enter",
    "tab",
    "escape",
    "backspace",
    "delete",
    "space",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
];

pub fn is_supported_key(key: &str) -> bool {
    SUPPORTED_KEYS.contains(&key.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_dimensions() {
        let rect = Rect {
            left: 10,
            top: 20,
            right: 110,
            bottom: 70,
        };
        assert_eq!((rect.width(), rect.height()), (100, 50));
    }
    #[test]
    fn only_plain_keys_are_supported() {
        assert!(is_supported_key("Enter"));
        assert!(is_supported_key("F5"));
        assert!(!is_supported_key("ctrl+s"));
        assert!(!is_supported_key("alt+f4"));
        assert!(!is_supported_key(""));
    }
}
