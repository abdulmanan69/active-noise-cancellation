//! Global keyboard shortcut (Ctrl+Shift+M) to toggle noise cancellation.

use std::sync::mpsc::{Receiver, channel};

use anyhow::Result;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

pub const LABEL: &str = "Ctrl+Shift+M";

pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    toggle: HotKey,
    rx: Receiver<()>,
}

impl Hotkeys {
    pub fn new(ctx: egui::Context) -> Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let toggle = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyM);
        manager.register(toggle)?;
        let (tx, rx) = channel();
        let id = toggle.id();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.id() == id && matches!(e.state(), HotKeyState::Pressed) {
                let _ = tx.send(());
                ctx.request_repaint();
            }
        }));
        Ok(Self {
            manager,
            toggle,
            rx,
        })
    }

    /// True when the shortcut was pressed an odd number of times since the last poll.
    pub fn poll_toggle(&self) -> bool {
        self.rx.try_iter().count() % 2 == 1
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        let _ = self.manager.unregister(self.toggle);
        GlobalHotKeyEvent::set_event_handler(None::<fn(GlobalHotKeyEvent)>);
    }
}
