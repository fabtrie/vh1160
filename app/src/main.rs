#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! VH1160 Control: desktop UI for the Vector VH1160 / VH1150 ECU test box.

mod app;
mod chart;
mod settings;
mod theme;
mod ui;
mod worker;

use std::borrow::Cow;

use gpui::{
    App, AppContext, AssetSource, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*]
    };
}

/// Tabler icons (MIT, see assets/icons/LICENSE), extracted from the Tabler webfont.
static EMBEDDED: &[(&str, &[u8])] = icons!(
    "circuit-switch-open",
    "device-desktop",
    "sun",
    "moon",
    "refresh",
    "plug",
    "plug-connected",
    "alert-triangle",
    "x",
    "power",
    "rotate-clockwise",
    "shield-check",
    "chart-line",
    "bolt-off",
    "route",
    "ruler-measure",
    "cpu",
    "player-pause",
    "player-play",
    "download",
    "wave-square",
    "activity",
    "switch-horizontal",
    "chevron-down",
    "check",
);

/// Our icons first, then gpui-component's own (its inputs use a few).
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match EMBEDDED.iter().find(|(p, _)| *p == path) {
            Some((_, bytes)) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = EMBEDDED
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| (*p).into())
            .collect();
        out.extend(gpui_kit_assets::Assets.list(path)?);
        Ok(out)
    }
}

fn main() {
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            // Share Tech Mono (OFL 1.1) for the LCD readouts, as in the Manson app.
            cx.text_system()
                .add_fonts(vec![Cow::Borrowed(
                    include_bytes!("../assets/fonts/ShareTechMono-Regular.ttf").as_slice(),
                )])
                .expect("load LCD font");

            let settings = settings::Settings::load();
            let system_dark = matches!(
                cx.window_appearance(),
                gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
            );
            theme::apply(app::palette_for(settings.theme, system_dark), cx);

            let bounds = Bounds::centered(None, size(px(1120.0), px(800.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("VH1160 Control".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(760.0), px(560.0))),
                focus: true,
                ..Default::default()
            };
            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| app::VhApp::new(settings, window, cx));
                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            })
            .expect("open window");
            cx.activate(true);
        });
}
