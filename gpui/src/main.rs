mod bridge;
mod locale;
mod model;
mod ui;
use gpui::{prelude::*, *};
actions!(batteryhog, [Quit]);
fn main() {
    let application = Application::new();
    application.on_reopen(|cx| {
        cx.activate(true);
        for window in cx.windows() {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        }
    });
    application.run(|cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![Menu {
            name: "Battery Hog".into(),
            items: vec![MenuItem::action("Quit Battery Hog", Quit)],
        }]);
        let bounds = Bounds::centered(None, size(px(1200.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(940.), px(650.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Battery Hog".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                let view = cx.new(ui::BatteryHog::new);
                window.focus(&view.read(cx).focus);
                view
            },
        )
        .expect("Unable to open Battery Hog window");
        cx.activate(true);
    });
}
