//! The icon that lets the window close without the watches stopping.
//!
//! Pathlight's promise is that monitoring survives the window. On macOS the
//! Swift app keeps a menu bar item; here the same job is a tray icon, and
//! without one there would be a running process with no way back to it — so
//! when the tray cannot be created the window stays the whole app and closing
//! it really does stop.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// Fixed ids rather than the generated ones, so the menu items can be built
/// on whichever thread this platform needs and still be recognised here.
const SHOW: &str = "pathlight-show";
const QUIT: &str = "pathlight-quit";

/// What the user asked for from outside the window.
pub enum Wish {
    Show,
    Quit,
}

pub struct Tray {
    wishes: Receiver<Wish>,
    /// Dropping the icon takes it out of the tray, so it is held for as long
    /// as the app runs. On Linux it belongs to its own thread instead.
    #[cfg(not(target_os = "linux"))]
    _icon: tray_icon::TrayIcon,
}

impl Tray {
    /// Everything the user asked for since the last frame.
    pub fn wishes(&self) -> impl Iterator<Item = Wish> + '_ {
        std::iter::from_fn(|| self.wishes.try_recv().ok())
    }
}

/// The tray icon, or `None` on a desktop that will not give us one.
///
/// Must be called from the thread running the event loop, once the loop is
/// running: on macOS that is a hard requirement, and on Windows the icon's
/// messages arrive on the creating thread.
#[cfg(not(target_os = "linux"))]
pub fn install(ctx: &egui::Context) -> Option<Tray> {
    let icon = build()?;
    let (sender, wishes) = mpsc::channel();
    listen(ctx, sender);
    Some(Tray {
        wishes,
        _icon: icon,
    })
}

/// The tray icon on its own gtk thread.
///
/// tray-icon's Linux backend is a gtk widget wanting a gtk main loop, and
/// eframe's loop is not one. A dedicated thread gives it the loop it needs
/// without either loop blocking the other.
#[cfg(target_os = "linux")]
pub fn install(ctx: &egui::Context) -> Option<Tray> {
    let (sender, wishes) = mpsc::channel();
    let (report, built) = mpsc::channel();
    std::thread::Builder::new()
        .name("pathlight-tray".into())
        .spawn(move || {
            if gtk::init().is_err() {
                let _ = report.send(false);
                return;
            }
            let Some(icon) = build() else {
                let _ = report.send(false);
                return;
            };
            let _ = report.send(true);
            gtk::main();
            drop(icon);
        })
        .ok()?;
    // Bounded, because this runs on the first frame: a desktop that never
    // answers must leave a usable window behind, not a white rectangle.
    if !built
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap_or(false)
    {
        return None;
    }
    listen(ctx, sender);
    Some(Tray { wishes })
}

fn build() -> Option<tray_icon::TrayIcon> {
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id(SHOW, "Open Pathlight", true, None),
        &MenuItem::with_id(QUIT, "Quit Pathlight", true, None),
    ])
    .ok()?;
    TrayIconBuilder::new()
        .with_tooltip("Pathlight — still watching")
        .with_menu(Box::new(menu))
        .with_icon(icon()?)
        .build()
        .ok()
}

/// Turns the library's global event handlers into wishes on a channel, and
/// wakes the interface: while the window is hidden nothing else asks for a
/// frame, so without this the click that should reopen it would sit unread.
fn listen(ctx: &egui::Context, sender: Sender<Wish>) {
    let ctx = ctx.clone();
    // A `Sender` is not `Sync` and both handlers must be; one mutex around
    // one send per click is not the cost worth avoiding here.
    let sender = Mutex::new(sender);
    let post: Arc<dyn Fn(Option<Wish>) + Send + Sync> = Arc::new(move |wish| {
        if let Some(wish) = wish {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(wish);
            }
        }
        ctx.request_repaint();
    });

    let menu = Arc::clone(&post);
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        menu(match event.id.0.as_str() {
            SHOW => Some(Wish::Show),
            QUIT => Some(Wish::Quit),
            _ => None,
        })
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        // On release, so one click is one wish rather than two.
        post(match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => Some(Wish::Show),
            _ => None,
        })
    }));
}

/// A ring in the green the interface already uses for a running watch.
///
/// Drawn rather than shipped: 32 pixels of one shape is less to maintain than
/// an asset per platform, and a tray icon nobody can see is the failure this
/// module exists to avoid.
fn icon() -> Option<Icon> {
    Icon::from_rgba(ring(), SIZE as u32, SIZE as u32).ok()
}

const SIZE: i32 = 32;

fn ring() -> Vec<u8> {
    const OUTER: f32 = 14.0;
    const INNER: f32 = 8.0;
    let center = (SIZE - 1) as f32 / 2.0;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let distance = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            // Coverage rather than a hard edge, so the ring does not look like
            // a staircase at this size.
            let coverage = (OUTER - distance).min(distance - INNER).clamp(0.0, 1.0);
            rgba.extend([0x2e, 0x9e, 0x5b, (coverage * 255.0) as u8]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The icon is arithmetic rather than an asset, so the thing that can
    /// break is the arithmetic: the wrong length is rejected by every
    /// platform, and an all-transparent icon is an invisible promise.
    #[test]
    fn the_drawn_icon_is_a_visible_ring() {
        let rgba = ring();
        assert_eq!(rgba.len(), (SIZE * SIZE * 4) as usize);

        let alpha_at = |x: i32, y: i32| rgba[((y * SIZE + x) * 4 + 3) as usize];
        let solid = (0..SIZE)
            .flat_map(|y| (0..SIZE).map(move |x| (x, y)))
            .filter(|(x, y)| alpha_at(*x, *y) == 255)
            .count();

        assert!(solid > 100, "only {solid} solid pixels: nothing to see");
        assert_eq!(alpha_at(16, 16), 0, "the middle of a ring is a hole");
        assert_eq!(alpha_at(0, 0), 0, "the corner of a ring is empty");
    }
}
