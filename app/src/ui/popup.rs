//! System notification for permission requests while the window is unfocused
//! (Wayland will not let us steal focus). The full popup is drawn in the room
//! itself (ADR 0014); the notification's buttons answer through the same path.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::{gio, glib};
use orch_core::{broker::Answer, model::AppState};
use uuid::Uuid;

const NOTIFICATION: &str = "permission";
const ACTION: &str = "permission-answer";

pub struct Notifier {
    app: adw::Application,
    /// Request already announced.
    notified: RefCell<Option<Uuid>>,
}

impl Notifier {
    pub fn new(app: &adw::Application, on_answer: impl Fn(Uuid, Answer) + 'static) -> Self {
        let on_answer: Rc<dyn Fn(Uuid, Answer)> = Rc::new(on_answer);
        // Buttons come back as `app.permission-answer("<id>:<answer>")`.
        let action = gio::SimpleAction::new(ACTION, Some(glib::VariantTy::STRING));
        action.connect_activate(move |_, parameter| {
            let Some(value) = parameter.and_then(glib::Variant::get::<String>) else {
                return;
            };
            let Some((id, answer)) = value.split_once(':') else {
                return;
            };
            let answer = match answer {
                "allow" => Answer::Allow,
                "deny" => Answer::Deny,
                _ => return,
            };
            if let Ok(id) = Uuid::parse_str(id) {
                on_answer(id, answer);
            }
        });
        app.add_action(&action);
        Self {
            app: app.clone(),
            notified: RefCell::new(None),
        }
    }

    /// Notifies about the oldest pending request while unfocused; withdraws it once
    /// nothing is pending.
    pub fn sync(&self, state: &AppState, focused: bool) {
        let Some(request) = state.permissions.first() else {
            if self.notified.borrow_mut().take().is_some() {
                self.app.withdraw_notification(NOTIFICATION);
            }
            return;
        };
        if focused || *self.notified.borrow() == Some(request.id) {
            return;
        }
        let agent = state.agent(request.agent_id);
        let name = agent.map_or("?", |a| a.name.as_str());
        let group = agent
            .and_then(|a| state.group(a.group_id))
            .map_or("?", |g| g.name.as_str());
        let notification =
            gio::Notification::new(&format!("{name} · {group} wants {}", request.tool));
        notification.set_body(Some(&request.input_summary()));
        notification.set_priority(gio::NotificationPriority::Urgent);
        for (label, answer) in [("Allow", "allow"), ("Deny", "deny")] {
            let target = format!("{}:{answer}", request.id).to_variant();
            notification.add_button_with_target_value(
                label,
                &format!("app.{ACTION}"),
                Some(&target),
            );
        }
        self.app
            .send_notification(Some(NOTIFICATION), &notification);
        *self.notified.borrow_mut() = Some(request.id);
    }
}
