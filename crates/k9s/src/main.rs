use gpui::{Bounds, WindowBounds, WindowOptions, prelude::*, px, size};
use k9s_app::Options;
use k9s_ui::{Quit, Workspace, key_bindings};

fn parse_args() -> (Options, bool) {
    let mut options = Options::default();
    let mut read_only = true;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-n" | "--namespace" => options.namespace = args.next(),
            "--context" => options.context = args.next(),
            "-A" | "--all-namespaces" => options.all_namespaces = true,
            "--write" => read_only = false,
            _ => {}
        }
    }
    (options, read_only)
}

fn main() {
    let (options, read_only) = parse_args();
    let updates = k9s_app::start(options.clone());
    gpui_platform::application().run(move |cx| {
        cx.bind_keys(key_bindings());
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some("k9s-gpui".into()),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    Workspace::new(updates, window, cx)
                        .with_read_only(read_only)
                        .with_starter(options, k9s_app::start)
                })
            },
        );
        if let Err(error) = opened {
            eprintln!("unable to open window: {error}");
            cx.quit();
        }
        cx.activate(true);
    });
}
