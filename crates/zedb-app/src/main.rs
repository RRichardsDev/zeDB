mod components;
mod grid_spike;
mod rt;
mod theme;

use gpui::{
    div, point, prelude::*, px, rgb, size, App, Application, Bounds, Context, Entity, IntoElement,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};
use zedb_ch::{ChClient, ChConfig};
use zedb_core::{load_connections, save_connections, ConnectionConfig, EnvTier};

use components::text_input::{self, TextInput};
use grid_spike::GridSpike;
use theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, TEXT, TEXT_DIM};

struct ConnectionForm {
    editing: Option<usize>,
    original_name: Option<String>,
    name: Entity<TextInput>,
    url: Entity<TextInput>,
    user: Entity<TextInput>,
    database: Entity<TextInput>,
    password: Entity<TextInput>,
    tier: EnvTier,
    read_only: bool,
}

struct Workspace {
    grid: Entity<GridSpike>,
    connections: Vec<ConnectionConfig>,
    selected: Option<usize>,
    connected: Option<String>,
    connecting: Option<String>,
    form: Option<ConnectionForm>,
    notice: Option<String>,
}

impl Workspace {
    fn new(grid: Entity<GridSpike>) -> Self {
        match load_connections() {
            Ok(connections) => Self {
                selected: (!connections.is_empty()).then_some(0),
                connections,
                grid,
                connected: None,
                connecting: None,
                form: None,
                notice: None,
            },
            Err(error) => Self {
                grid,
                connections: Vec::new(),
                selected: None,
                connected: None,
                connecting: None,
                form: None,
                notice: Some(format!("Could not load connections: {error}")),
            },
        }
    }

    fn title_bar(&self) -> impl IntoElement {
        div()
            .h(px(36.))
            .flex_none()
            .w_full()
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .flex()
            .items_center()
            .pl(px(90.))
            .pr_3()
            .text_sm()
            .text_color(rgb(TEXT))
            .child("zeDB")
    }

    fn tier_color(tier: EnvTier) -> u32 {
        match tier {
            EnvTier::Dev => 0x3fb950,
            EnvTier::Staging => 0xd29922,
            EnvTier::Production => 0xf85149,
        }
    }

    fn tier_badge(tier: EnvTier) -> impl IntoElement {
        div()
            .px_2()
            .py(px(2.))
            .rounded(px(3.))
            .bg(rgb(Self::tier_color(tier)))
            .text_color(rgb(0x101215))
            .text_xs()
            .child(tier.label().to_uppercase())
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .connections
            .iter()
            .enumerate()
            .map(|(index, connection)| {
                let selected = self.selected == Some(index);
                let connected = self.connected.as_deref() == Some(connection.name.as_str());
                div()
                    .id(("connection", index))
                    .w_full()
                    .px_2()
                    .py_2()
                    .rounded(px(3.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(selected, |row| row.bg(rgb(0x303640)))
                    .hover(|row| row.bg(rgb(0x2a2f37)).cursor_pointer())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(index);
                        this.notice = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .text_color(rgb(TEXT))
                            .child(connection.name.clone())
                            .when(connected, |row| {
                                row.child(div().size(px(7.)).rounded_full().bg(rgb(0x3fb950)))
                            }),
                    )
                    .child(Self::tier_badge(connection.tier))
            })
            .collect::<Vec<_>>();

        div()
            .w(px(240.))
            .flex_none()
            .h_full()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .text_color(rgb(TEXT_DIM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("CONNECTIONS")
                    .child(
                        div()
                            .id("add-connection")
                            .px_2()
                            .py_1()
                            .rounded(px(3.))
                            .text_color(rgb(TEXT))
                            .child("+")
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(|this, _, _, cx| this.start_add(cx))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(rows.is_empty(), |list| {
                        list.child(
                            div()
                                .pt_3()
                                .text_color(rgb(TEXT_DIM))
                                .child("No saved connections"),
                        )
                    })
                    .children(rows),
            )
            .when(self.selected.is_some(), |sidebar| {
                sidebar.child(
                    div()
                        .id("edit-connection")
                        .w_full()
                        .py_2()
                        .flex()
                        .justify_center()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .text_color(rgb(TEXT))
                        .child("Edit connection")
                        .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                        .on_click(cx.listener(|this, _, _, cx| this.start_edit(cx))),
                )
            })
    }

    fn input(
        content: impl Into<String>,
        placeholder: &'static str,
        secret: bool,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let content = content.into();
        cx.new(|cx| TextInput::new(content, placeholder, secret, cx))
    }

    fn start_add(&mut self, cx: &mut Context<Self>) {
        self.form = Some(ConnectionForm {
            editing: None,
            original_name: None,
            name: Self::input("", "staging", false, cx),
            url: Self::input("http://localhost:8123", "http://host:8123", false, cx),
            user: Self::input("default", "default", false, cx),
            database: Self::input("", "optional", false, cx),
            password: Self::input("", "stored in macOS Keychain", true, cx),
            tier: EnvTier::Dev,
            read_only: true,
        });
        self.notice = None;
        cx.notify();
    }

    fn start_edit(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected else {
            return;
        };
        let connection = self.connections[index].clone();
        self.form = Some(ConnectionForm {
            editing: Some(index),
            original_name: Some(connection.name.clone()),
            name: Self::input(connection.name, "staging", false, cx),
            url: Self::input(connection.url, "http://host:8123", false, cx),
            user: Self::input(connection.user, "default", false, cx),
            database: Self::input(
                connection.database.unwrap_or_default(),
                "optional",
                false,
                cx,
            ),
            password: Self::input("", "leave blank to keep existing", true, cx),
            tier: connection.tier,
            read_only: connection.read_only,
        });
        self.notice = None;
        cx.notify();
    }

    fn cancel_form(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.notice = None;
        cx.notify();
    }

    fn cycle_tier(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.tier = form.tier.next();
            cx.notify();
        }
    }

    fn toggle_read_only(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.read_only = !form.read_only;
            cx.notify();
        }
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        let value = |input: &Entity<TextInput>| input.read(cx).text().trim().to_string();
        let name = value(&form.name);
        let url = value(&form.url);
        let user = value(&form.user);
        let database = value(&form.database);
        let password = form.password.read(cx).text();
        let editing = form.editing;
        let original_name = form.original_name.clone();

        if name.is_empty() || url.is_empty() || user.is_empty() {
            self.notice = Some("Name, endpoint, and user are required".into());
            cx.notify();
            return;
        }
        if self
            .connections
            .iter()
            .enumerate()
            .any(|(index, connection)| Some(index) != editing && connection.name == name)
        {
            self.notice = Some(format!("A connection named {name:?} already exists"));
            cx.notify();
            return;
        }

        let connection = ConnectionConfig {
            name: name.clone(),
            url,
            user,
            database: (!database.is_empty()).then_some(database),
            tier: form.tier,
            read_only: form.read_only,
        };
        let mut updated = self.connections.clone();
        if let Some(index) = editing {
            updated[index] = connection;
        } else {
            updated.push(connection);
        }

        let secret_result = if password.is_empty() {
            match original_name.as_deref() {
                Some(old) if old != name => zedb_core::secrets::rename(old, &name),
                _ => Ok(()),
            }
        } else {
            zedb_core::secrets::set_password(&name, &password).and_then(|_| {
                if let Some(old) = original_name.as_deref().filter(|old| *old != name) {
                    zedb_core::secrets::delete_password(old)?;
                }
                Ok(())
            })
        };
        if let Err(error) = secret_result {
            self.notice = Some(format!("Could not update macOS Keychain: {error}"));
            cx.notify();
            return;
        }
        if let Err(error) = save_connections(&updated) {
            self.notice = Some(format!("Could not save connections: {error}"));
            cx.notify();
            return;
        }

        self.connections = updated;
        self.selected = Some(editing.unwrap_or(self.connections.len() - 1));
        self.form = None;
        self.notice = Some(format!("Saved {name}"));
        cx.notify();
    }

    fn connect_selected(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selected else {
            return;
        };
        let connection = self.connections[index].clone();
        let password = match zedb_core::secrets::get_password(&connection.name) {
            Ok(password) => password,
            Err(error) => {
                self.notice = Some(format!("Could not read macOS Keychain: {error}"));
                cx.notify();
                return;
            }
        };
        let name = connection.name.clone();
        let client = ChClient::new(ChConfig {
            url: connection.url,
            user: connection.user,
            password,
            database: connection.database,
            read_only: connection.read_only,
        });
        self.connecting = Some(name.clone());
        self.notice = Some(format!("Connecting to {name}..."));
        cx.notify();

        let task = rt::tokio().spawn(async move { client.ping().await });
        cx.spawn(async move |this, cx| {
            let connected = task.await.unwrap_or(false);
            this.update(cx, |this, cx| {
                this.connecting = None;
                if connected {
                    this.connected = Some(name.clone());
                    this.notice = Some(format!("Connected to {name}"));
                } else {
                    this.notice = Some(format!("Could not connect to {name}"));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn disconnect(&mut self, cx: &mut Context<Self>) {
        if let Some(name) = self.connected.take() {
            self.notice = Some(format!("Disconnected from {name}"));
        }
        cx.notify();
    }

    fn field(label: &'static str, input: Entity<TextInput>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(label))
            .child(input)
    }

    fn form_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self.form.as_ref().expect("form panel requires a form");
        let heading = if form.editing.is_some() {
            "Edit connection"
        } else {
            "Add connection"
        };
        div()
            .size_full()
            .bg(rgb(BG))
            .p_6()
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(520.))
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_lg().text_color(rgb(TEXT)).child(heading))
                    .child(Self::field("NAME", form.name.clone()))
                    .child(Self::field("HTTP ENDPOINT", form.url.clone()))
                    .child(Self::field("USER", form.user.clone()))
                    .child(Self::field("DATABASE", form.database.clone()))
                    .child(Self::field("PASSWORD", form.password.clone()))
                    .child(
                        div()
                            .flex()
                            .gap_3()
                            .child(
                                div()
                                    .id("cycle-tier")
                                    .flex_1()
                                    .h(px(34.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child("Environment")
                                    .child(Self::tier_badge(form.tier))
                                    .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| this.cycle_tier(cx))),
                            )
                            .child(
                                div()
                                    .id("toggle-read-only")
                                    .flex_1()
                                    .h(px(34.))
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child("Read only")
                                    .child(if form.read_only { "ON" } else { "OFF" })
                                    .when(form.read_only, |button| button.text_color(rgb(0x3fb950)))
                                    .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.toggle_read_only(cx)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("cancel-connection")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(3.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child("Cancel")
                                    .hover(|button| button.bg(rgb(BG_SIDEBAR)).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_form(cx))),
                            )
                            .child(
                                div()
                                    .id("save-connection")
                                    .px_4()
                                    .py_2()
                                    .rounded(px(3.))
                                    .bg(rgb(0x2f6f9f))
                                    .text_color(rgb(0xffffff))
                                    .child("Save")
                                    .hover(|button| button.bg(rgb(0x3884bd)).cursor_pointer())
                                    .on_click(cx.listener(|this, _, _, cx| this.save_form(cx))),
                            ),
                    ),
            )
    }

    fn connection_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.and_then(|index| self.connections.get(index));
        let selected_connected = selected
            .map(|connection| self.connected.as_deref() == Some(connection.name.as_str()))
            .unwrap_or(false);
        div()
            .h(px(38.))
            .flex_none()
            .w_full()
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(BG_SIDEBAR))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(selected, |row, connection| {
                        row.child(connection.name.clone())
                            .child(Self::tier_badge(connection.tier))
                    })
                    .when(selected.is_none(), |row| row.child("Select a connection")),
            )
            .child(
                div()
                    .id("connect-toggle")
                    .px_3()
                    .py_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_color(rgb(TEXT))
                    .child(if self.connecting.is_some() {
                        "Connecting..."
                    } else if selected_connected {
                        "Disconnect"
                    } else {
                        "Connect"
                    })
                    .when(self.connecting.is_none() && selected.is_some(), |button| {
                        button
                            .hover(|button| button.bg(rgb(0x303640)).cursor_pointer())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if selected_connected {
                                    this.disconnect(cx);
                                } else {
                                    this.connect_selected(cx);
                                }
                            }))
                    }),
            )
    }

    fn status_bar(&self) -> impl IntoElement {
        let status = self
            .notice
            .clone()
            .unwrap_or_else(|| match &self.connected {
                Some(name) => format!("Connected to {name}"),
                None => "Not connected".to_string(),
            });
        div()
            .h(px(28.))
            .flex_none()
            .w_full()
            .bg(rgb(BG_STATUS))
            .border_t_1()
            .border_color(rgb(BORDER))
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .text_xs()
            .text_color(rgb(TEXT_DIM))
            .child(status)
            .child(concat!("zedb ", env!("CARGO_PKG_VERSION"), " | M3"))
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Menlo")
            .text_sm()
            .child(self.title_bar())
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .when(self.form.is_some(), |main| main.child(self.form_panel(cx)))
                            .when(self.form.is_none(), |main| {
                                main.child(self.connection_toolbar(cx))
                                    .child(div().flex_1().min_h_0().child(self.grid.clone()))
                            }),
                    ),
            )
            .child(self.status_bar())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        text_input::init(cx);
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("zeDB".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(12.))),
                }),
                ..Default::default()
            },
            |_, cx| {
                let grid = cx.new(GridSpike::new);
                cx.new(|_| Workspace::new(grid))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
