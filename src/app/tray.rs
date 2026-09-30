//! System tray icon with a small context menu.

use std::sync::mpsc::{Receiver, channel};

use anyhow::Result;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMsg {
    Show,
    Toggle,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    toggle: CheckMenuItem,
    rx: Receiver<TrayMsg>,
}

fn tooltip(active: bool) -> String {
    if active {
        "ClearMic - noise cancellation ON".to_string()
    } else {
        "ClearMic - noise cancellation OFF".to_string()
    }
}

impl Tray {
    pub fn new(ctx: egui::Context, active: bool) -> Result<Self> {
        let menu = Menu::new();
        let show = MenuItem::with_id("show", "Open ClearMic", true, None);
        let toggle = CheckMenuItem::with_id("toggle", "Noise cancellation", true, active, None);
        let quit = MenuItem::with_id("quit", "Quit ClearMic", true, None);
        menu.append_items(&[&show, &toggle, &PredefinedMenuItem::separator(), &quit])?;

        let icon = TrayIconBuilder::new()
            .with_id("clearmic")
            .with_tooltip(tooltip(active))
            .with_icon(icon::tray_icon(active))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        let (tx, rx) = channel();
        {
            let tx = tx.clone();
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let msg = match e.id.0.as_str() {
                    "show" => Some(TrayMsg::Show),
                    "toggle" => Some(TrayMsg::Toggle),
                    "quit" => Some(TrayMsg::Quit),
                    _ => None,
                };
                if let Some(m) = msg {
                    let _ = tx.send(m);
                    ctx.request_repaint();
                }
            }));
        }
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            let show = matches!(
                e,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if show {
                let _ = tx.send(TrayMsg::Show);
                ctx.request_repaint();
            }
        }));

        Ok(Self { icon, toggle, rx })
    }

    pub fn poll(&self) -> Vec<TrayMsg> {
        self.rx.try_iter().collect()
    }

    pub fn set_active(&self, active: bool) {
        self.toggle.set_checked(active);
        let _ = self.icon.set_icon(Some(icon::tray_icon(active)));
        let _ = self.icon.set_tooltip(Some(tooltip(active)));
    }
}
