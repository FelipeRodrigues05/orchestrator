//! The pet floating over the desktop (ADR 0012, step 9): a small transparent
//! layer-shell surface that walks along the bottom of the screen. Where the
//! compositor has no layer-shell (GNOME) the pet stays in the room.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::glib;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::{pet, render::Painter, room::Room};

/// Cells of the floating surface: the pet, its label and its shadow.
const COLS: usize = pet::WIDTH + 4;
const ROWS: usize = pet::HEIGHT + 2;
/// Pixels per tick while walking.
const STEP: i32 = 3;

pub fn supported() -> bool {
    gtk4_layer_shell::is_supported()
}

pub struct FloatingPet {
    window: gtk::Window,
    area: gtk::DrawingArea,
    x: RefCell<i32>,
    direction: RefCell<i32>,
}

impl FloatingPet {
    pub fn new(
        app: &adw::Application,
        room: &Rc<RefCell<Room>>,
        main: &adw::ApplicationWindow,
    ) -> Rc<Self> {
        let painter = Painter::new(main).transparent();
        let (width, height) = painter.size_of(COLS, ROWS);
        let area = gtk::DrawingArea::builder()
            .content_width(width)
            .content_height(height)
            .build();
        area.set_draw_func({
            let room = room.clone();
            move |area, cr, width, height| {
                let (cols, rows, view) = painter.fit(width, height);
                let grid = room.borrow_mut().compose_pet(cols, rows);
                painter.paint(area, cr, &grid, view, (width, height));
            }
        });
        let window = gtk::Window::builder()
            .application(app)
            .title("Orchestrator pet")
            .child(&area)
            .css_classes(["pet-float"])
            .build();
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("orchestrator-pet"));
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_anchor(Edge::Bottom, true);
        window.set_anchor(Edge::Left, true);
        window.set_margin(Edge::Bottom, 24);

        // A click on the pet brings the control room back.
        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let main = main.clone();
            move |_, _, _, _| main.present()
        });
        area.add_controller(click);

        Rc::new(Self {
            window,
            area,
            x: RefCell::new(80),
            direction: RefCell::new(1),
        })
    }

    /// Shows or hides the surface and moves the pet one step.
    pub fn tick(&self, visible: bool, standing: bool) {
        if visible != self.window.is_visible() {
            self.window.set_visible(visible);
        }
        if !visible {
            return;
        }
        if !standing {
            let screen = self.screen_width();
            let width = self.area.width().max(1);
            let mut x = self.x.borrow_mut();
            let mut direction = self.direction.borrow_mut();
            *x += STEP * *direction;
            if *x <= 0 || *x + width >= screen {
                *direction = -*direction;
                *x = (*x).clamp(0, (screen - width).max(0));
            }
            self.window.set_margin(Edge::Left, *x);
        }
        self.area.queue_draw();
    }

    fn screen_width(&self) -> i32 {
        let display = gtk::prelude::WidgetExt::display(&self.window);
        let monitor = self
            .window
            .surface()
            .and_then(|surface| display.monitor_at_surface(&surface))
            .or_else(|| {
                display
                    .monitors()
                    .item(0)
                    .and_then(|m| m.downcast::<gtk::gdk::Monitor>().ok())
            });
        monitor.map_or(1920, |m| m.geometry().width())
    }
}

/// Keeps the floating pet in step with the room, every animation tick.
pub fn install(app: &adw::Application, room: Rc<RefCell<Room>>, main: &adw::ApplicationWindow) {
    if !supported() {
        return;
    }
    let pet = FloatingPet::new(app, &room, main);
    glib::timeout_add_local(super::room::TICK, move || {
        let (visible, standing) = {
            let room = room.borrow();
            (room.floating(), room.pet_alert())
        };
        pet.tick(visible, standing);
        glib::ControlFlow::Continue
    });
}
