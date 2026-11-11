//! The transport icons, drawn in the colour of the text around them.

use std::cell::Cell;
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::prelude::*;

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Record,
    Stop,
    Play,
    Pause,
    Settings,
}

/// A 16-pixel icon.
#[derive(Clone)]
pub struct IconView {
    area: gtk::DrawingArea,
    icon: Rc<Cell<Icon>>,
}

impl IconView {
    pub fn new(icon: Icon) -> Self {
        let area = gtk::DrawingArea::builder()
            .content_width(16)
            .content_height(16)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        let icon = Rc::new(Cell::new(icon));
        area.set_draw_func({
            let icon = icon.clone();
            move |area, cr, _, _| {
                let color = area.color();
                cr.set_source_rgba(
                    color.red().into(),
                    color.green().into(),
                    color.blue().into(),
                    color.alpha().into(),
                );
                match icon.get() {
                    Icon::Record => cr.arc(8.0, 8.0, 4.5, 0.0, TAU),
                    Icon::Stop => cr.rectangle(4.0, 4.0, 8.0, 8.0),
                    Icon::Play => {
                        cr.move_to(5.0, 3.0);
                        cr.line_to(13.0, 8.0);
                        cr.line_to(5.0, 13.0);
                        cr.close_path();
                    }
                    Icon::Pause => {
                        cr.rectangle(4.0, 3.5, 3.0, 9.0);
                        cr.rectangle(9.0, 3.5, 3.0, 9.0);
                    }
                    Icon::Settings => {
                        // A cog: a ring with eight teeth.
                        cr.set_line_width(2.0);
                        cr.arc(8.0, 8.0, 3.8, 0.0, TAU);
                        let _ = cr.stroke();
                        cr.set_line_width(2.4);
                        for tooth in 0..8 {
                            let angle = f64::from(tooth) * TAU / 8.0;
                            let (sin, cos) = angle.sin_cos();
                            cr.move_to(8.0 + cos * 4.8, 8.0 + sin * 4.8);
                            cr.line_to(8.0 + cos * 7.0, 8.0 + sin * 7.0);
                        }
                        let _ = cr.stroke();
                        return;
                    }
                }
                // Drawing only fails if the surface is gone; nothing to show then.
                let _ = cr.fill();
            }
        });
        Self { area, icon }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    pub fn set(&self, icon: Icon) {
        self.icon.set(icon);
        self.area.queue_draw();
    }
}

/// A button showing an icon followed by a label.
pub fn button(icon: &IconView, label: &gtk::Label) -> gtk::Button {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    content.set_halign(gtk::Align::Center);
    content.append(icon.widget());
    content.append(label);
    gtk::Button::builder().child(&content).build()
}
