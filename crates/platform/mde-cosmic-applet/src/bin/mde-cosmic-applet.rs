//! GUI-6 — the libcosmic panel shell for the Magic Mesh cosmic-applet.
//!
//! Thin glue over the crate's render-agnostic logic layer
//! (`mde_cosmic_applet`): it draws the mesh-health pip into the Cosmic
//! panel, opens a popover of quick actions, and routes each action to
//! its Bus verb (`mde-bus`) or a Workbench deep-link (`mde-workbench
//! --focus <slug>`). All the decisions — pip derivation, the
//! action→verb/link tables, the launch argv — live in the lib and are
//! unit-tested there; this binary only wires them to libcosmic + the
//! mesh, so the applet and the Front Door never disagree.
//!
//! The pip refreshes off `mackesd peers --json` (PD-1 directory parity,
//! the same record `pip_from_directory` reads). It needs a live Cosmic
//! session to draw; run it under `cosmic-comp` or let `cosmic-panel`
//! host it.

use std::time::Duration;

use cosmic::app::{Core, Task};
use cosmic::iced::window::Id;
use cosmic::iced::Rectangle;
use cosmic::iced::{time, Subscription};
use cosmic::surface::action::{app_popup, destroy_popup};
use cosmic::widget;
use cosmic::{Application, Element};

use mde_cosmic_applet::{action_bus_topic, launch_argv, pip_from_directory, Pip, QuickAction};

const ID: &str = "com.mackes.MagicMeshApplet";
/// Pip refresh cadence — the mesh directory changes on the heartbeat
/// timescale, so a 10 s poll is live enough without busy-spinning.
const REFRESH: Duration = Duration::from_secs(10);

struct Applet {
    core: Core,
    popup: Option<Id>,
    pip: Pip,
}

#[derive(Clone, Debug)]
enum Message {
    /// Periodic pip refresh fired.
    Tick,
    /// A refresh resolved.
    PipLoaded(Pip),
    /// A popover quick action was pressed.
    Action(QuickAction),
    /// libcosmic popup lifecycle.
    PopupClosed(Id),
    Surface(cosmic::surface::Action),
    /// An off-thread bus publish completed — nothing to render.
    Noop,
}

/// Shell `mackesd peers --json` and derive the pip. Any failure
/// (mackesd absent, not-ok, garbage) collapses to `Pip::Down` via the
/// logic layer — honest, never a fake-healthy.
async fn load_pip() -> Pip {
    match tokio::process::Command::new("mackesd")
        .args(["peers", "--json"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            pip_from_directory(&String::from_utf8_lossy(&out.stdout))
        }
        _ => Pip::Down,
    }
}

/// Symbolic icon name the panel renders for each pip state (freedesktop
/// symbolic set, present in the Cosmic icon theme).
fn pip_icon(pip: Pip) -> &'static str {
    match pip {
        Pip::Healthy => "network-transmit-receive-symbolic",
        Pip::Degraded => "network-error-symbolic",
        Pip::Down => "network-offline-symbolic",
    }
}

/// Synchronously publish a quick action's Bus verb. BLOCKING — opens the
/// (multi-MB) system `index.sqlite` and writes one message, so callers MUST
/// run it off the UI thread (see the `Message::Action` arm). Factored out of
/// the old `fire()` so the popup can close instantly while this runs in a
/// `spawn_blocking` task.
fn publish_action(topic: &'static str) {
    if let Some(dir) = mde_bus::client_data_dir() {
        if let Ok(persist) = mde_bus::persist::Persist::open(dir) {
            let _ = persist.write(
                topic,
                mde_bus::hooks::config::Priority::Default,
                None,
                Some("{}"),
            );
        }
    }
}

impl Application for Applet {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Message>) {
        let applet = Applet {
            core,
            popup: None,
            pip: Pip::Down,
        };
        // Prime the pip immediately so the panel isn't briefly wrong.
        (
            applet,
            Task::perform(load_pip(), |p| cosmic::Action::App(Message::PipLoaded(p))),
        )
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        time::every(REFRESH).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                return Task::perform(load_pip(), |p| cosmic::Action::App(Message::PipLoaded(p)))
            }
            Message::PipLoaded(pip) => self.pip = pip,
            Message::Action(action) => {
                // SUBAUDIT — close the popover IMMEDIATELY so the click feels
                // instant. The old path ran the Bus write (`Persist::open` on
                // the multi-MB system `index.sqlite`) synchronously on the
                // SingleThreadExecutor, freezing the applet for seconds per
                // click. Now the popup closes first and the publish (or the
                // Workbench launch) runs off the UI thread.
                let close: Task<Message> = if let Some(id) = self.popup.take() {
                    cosmic::task::message(cosmic::Action::Cosmic(cosmic::app::Action::Surface(
                        destroy_popup(id),
                    )))
                } else {
                    Task::none()
                };
                if let Some(topic) = action_bus_topic(action) {
                    let write = Task::perform(
                        async move {
                            let _ =
                                tokio::task::spawn_blocking(move || publish_action(topic)).await;
                        },
                        |()| cosmic::Action::App(Message::Noop),
                    );
                    return Task::batch([close, write]);
                } else if let Some(argv) = launch_argv(action) {
                    // Process spawn is already non-blocking (fire-and-forget).
                    if let Some((cmd, args)) = argv.split_first() {
                        let _ = std::process::Command::new(cmd).args(args).spawn();
                    }
                }
                return close;
            }
            Message::Noop => {}
            Message::PopupClosed(id) => {
                if self.popup.as_ref() == Some(&id) {
                    self.popup = None;
                }
            }
            Message::Surface(a) => {
                return cosmic::task::message(cosmic::Action::Cosmic(
                    cosmic::app::Action::Surface(a),
                ));
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let have_popup = self.popup;
        let pip = self.pip;
        let btn = self
            .core
            .applet
            .icon_button(pip_icon(pip))
            .on_press_with_rectangle(move |offset, bounds| {
                if let Some(id) = have_popup {
                    Message::Surface(destroy_popup(id))
                } else {
                    Message::Surface(app_popup::<Applet>(
                        move |state: &mut Applet| {
                            let new_id = Id::unique();
                            state.popup = Some(new_id);
                            let mut popup_settings = state.core.applet.get_popup_settings(
                                state.core.main_window_id().unwrap(),
                                new_id,
                                None,
                                None,
                                None,
                            );
                            popup_settings.positioner.anchor_rect = Rectangle {
                                x: (bounds.x - offset.x) as i32,
                                y: (bounds.y - offset.y) as i32,
                                width: bounds.width as i32,
                                height: bounds.height as i32,
                            };
                            popup_settings
                        },
                        Some(Box::new(move |state: &Applet| {
                            Element::from(state.core.applet.popup_container(state.popover()))
                                .map(cosmic::Action::App)
                        })),
                    ))
                }
            });

        Element::from(self.core.applet.applet_tooltip::<Message>(
            btn,
            pip.tooltip().to_string(),
            self.popup.is_some(),
            Message::Surface,
            None,
        ))
    }

    fn view_window(&self, _id: Id) -> Element<'_, Message> {
        // All real content is rendered through the popover container
        // built in `view`; this is only hit for stray window ids.
        widget::text("").into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl Applet {
    /// The popover body: a header line of pip status, then one button
    /// per quick action.
    fn popover(&self) -> Element<'_, Message> {
        let mut col = widget::list_column().add(widget::text::body(self.pip.tooltip()));
        for action in [
            QuickAction::OpenNotifications,
            QuickAction::ToggleDnd,
            QuickAction::OpenPeers,
            QuickAction::OpenTransfers,
            QuickAction::OpenRegistration,
        ] {
            col = col.add(
                widget::button::text(action_label(action))
                    .width(cosmic::iced::Length::Fill)
                    .on_press(Message::Action(action)),
            );
        }
        col.into()
    }
}

/// Human label for a quick action's popover button.
fn action_label(action: QuickAction) -> &'static str {
    match action {
        QuickAction::OpenNotifications => "🔔 Notifications",
        QuickAction::ToggleDnd => "Toggle Do-Not-Disturb",
        QuickAction::OpenPeers => "Open Peers",
        QuickAction::OpenTransfers => "Open Transfers",
        QuickAction::OpenRegistration => "Join / Leave mesh",
    }
}

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<Applet>(())
}
