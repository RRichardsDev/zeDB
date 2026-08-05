use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, Window, WindowBounds,
    WindowOptions,
};

// Placeholder palette until a real theme system exists.
const BG: u32 = 0x1e2227;
const BG_SIDEBAR: u32 = 0x23272e;
const BG_STATUS: u32 = 0x191c20;
const BORDER: u32 = 0x33383f;
const TEXT: u32 = 0xaab2bd;
const TEXT_DIM: u32 = 0x6b7380;

struct Workspace;

impl Workspace {
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

    fn main_pane(&self) -> impl IntoElement {
        div()
            .flex_1()
            .h_full()
            .bg(rgb(BG))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(TEXT_DIM))
            .child("zeDB")
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
            .child(concat!("zedb ", env!("CARGO_PKG_VERSION")))
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
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .child(self.sidebar())
                    .child(self.main_pane()),
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
                ..Default::default()
            },
            |_, cx| cx.new(|_| Workspace),
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
