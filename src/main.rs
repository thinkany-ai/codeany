mod agent_icons;
mod agent_panel;
mod agl_bridge;
mod db;
mod external;
mod file_tree;
mod git;
mod git_panel;
mod icons;
mod model_picker;
mod settings;
mod paths;
mod remote;
mod search_palette;
mod style;
mod surface_host;
mod surfaces;
mod terminal;
mod themes;
mod threads;
mod workspace;

use std::path::PathBuf;

use gpui_kit::*;

use crate::workspace::Workspace;

actions!(
    codeany,
    [
        Quit,
        OpenFolder,
        Save,
        CloseTab,
        ToggleSidebar,
        ToggleGitPanel,
        ToggleTerminal,
        RefreshProject,
        OpenEditor,
        NewThread,
        OpenSettings,
        OpenSurface,
        Search,
    ]
);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("serve" | "link")) {
        agl_bridge::set_home();
        std::process::exit(remote::serve_cli(&args));
    }

    let initial_root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok())
        .filter(|path| path.is_dir());

    agl_bridge::set_home();
    // Start agl early: it reads the login shell's PATH.
    agl_bridge::start_local();
    remote::init();

    gpui_kit::application()
        .with_assets(style::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-o", OpenFolder, Some("Workspace")),
                KeyBinding::new("cmd-s", Save, Some("Workspace")),
                KeyBinding::new("cmd-w", CloseTab, Some("Workspace")),
                KeyBinding::new("cmd-b", ToggleSidebar, Some("Workspace")),
                KeyBinding::new("cmd-shift-g", ToggleGitPanel, Some("Workspace")),
                KeyBinding::new("ctrl-`", ToggleTerminal, Some("Workspace")),
                KeyBinding::new("cmd-j", ToggleTerminal, Some("Workspace")),
                KeyBinding::new("cmd-e", OpenEditor, Some("Workspace")),
                KeyBinding::new("cmd-n", NewThread, Some("Workspace")),
                KeyBinding::new("cmd-,", OpenSettings, Some("Workspace")),
                KeyBinding::new("cmd-t", OpenSurface, Some("Workspace")),
                KeyBinding::new("cmd-p", Search, Some("Workspace")),
            ]);
            terminal::init(cx);
            model_picker::init(cx);

            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_app_quit(|_| async {
                agl_bridge::shutdown();
            })
            .detach();
            cx.set_menus([
                Menu::new("CodeAny").items([
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit CodeAny", Quit),
                ]),
                Menu::new("File").items([
                    MenuItem::action("New Thread", NewThread),
                    MenuItem::action("Open Folder…", OpenFolder),
                    MenuItem::action("Save", Save),
                    MenuItem::action("Close Tab", CloseTab),
                ]),
                Menu::new("View").items([
                    MenuItem::action("Open Editor Window", OpenEditor),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Sidebar", ToggleSidebar),
                    MenuItem::action("Toggle Git Changes", ToggleGitPanel),
                    MenuItem::action("Open a Surface…", OpenSurface),
                    MenuItem::action("Toggle Terminal", ToggleTerminal),
                    MenuItem::separator(),
                    MenuItem::action("Refresh", RefreshProject),
                ]),
            ]);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
                titlebar: Some(TitlebarOptions {
                    title: Some("CodeAny".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(13.))),
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                style::apply(window, cx);
                cx.new(|cx| Workspace::new(initial_root.clone(), window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
