mod grid_spike;
mod theme;

use gpui::{
    div, point, prelude::*, px, rgb, size, App, Application, Bounds, Context, Entity,
    TitlebarOptions, Window, WindowBounds, WindowOptions,
};

use grid_spike::GridSpike;
use theme::{BG, BG_SIDEBAR, BG_STATUS, BORDER, TEXT, TEXT_DIM};

struct Workspace {
    grid: Entity<GridSpike>,
}

impl Workspace {
    /// Full-width top bar; the transparent native titlebar overlays it, so
    /// it is also the window drag area. Left padding clears the traffic
    /// lights.
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
            .text_color(rgb(TEXT_DIM))
            .child("zeDB")
    }

    fn sidebar(&self) -> impl IntoElement {
        div()
            .w(px(240.))
            .flex_none()
            .h_full()
            .bg(rgb(BG_SIDEBAR))
            .border_r_1()
            .border_color(rgb(BORDER))
            .p_3()
            .text_sm()
            .text_color(rgb(TEXT_DIM))
            .child("connections")
    }

    fn status_bar(&self) -> impl IntoElement {
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
            .text_xs()
            .text_color(rgb(TEXT_DIM))
            .child(concat!(
                "zedb ",
                env!("CARGO_PKG_VERSION"),
                " | grid spike (M2)"
            ))
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Menlo")
            .child(self.title_bar())
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_h_0()
                    .flex()
                    .child(self.sidebar())
                    .child(div().flex_1().h_full().min_w_0().child(self.grid.clone())),
            )
            .child(self.status_bar())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
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
                cx.new(|cx| Workspace {
                    grid: cx.new(GridSpike::new),
                })
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
