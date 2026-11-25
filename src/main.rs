mod agent_icons;
mod agent_panel;
mod agl_bridge;
mod chat_tabs;
mod db;
mod file_tree;
mod git;
mod git_panel;
mod i18n;
mod icons;
mod model_picker;
mod settings;
mod paths;
mod peer;
mod project_avatar;
mod remote;
mod remote_folders;
mod search_palette;
mod style;
mod surface_host;
mod sync;
mod surfaces;
mod terminal;
mod themes;
mod threads;
#[cfg(target_os = "macos")]
mod updater;
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
        NewTab,
        ReopenTab,
        NextTab,
        PrevTab,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectLastTab,
        OpenSettings,
        CheckForUpdates,
        OpenSurface,
        Search,
    ]
);

/// Set when the updater runs, so the menu offers "Check for Updates…".
struct UpdaterRunning;
impl Global for UpdaterRunning {}

/// The menu bar, in the current language; set again when it changes.
pub fn set_app_menus(cx: &mut App) {
    use i18n::t;
    let mut app_menu = vec![MenuItem::action(t("Settings…"), OpenSettings)];
    #[cfg(target_os = "macos")]
    if cx.has_global::<UpdaterRunning>() {
        app_menu.push(MenuItem::action(t("Check for Updates…"), CheckForUpdates));
    }
    app_menu.extend([MenuItem::separator(), MenuItem::action(t("Quit CodeAny"), Quit)]);
    cx.set_menus([
        Menu::new("CodeAny").items(app_menu),
        Menu::new(t("File")).items([
            MenuItem::action(t("New Thread"), NewThread),
            MenuItem::action(t("Open Folder…"), OpenFolder),
            MenuItem::action(t("Save"), Save),
            MenuItem::action(t("Close Tab"), CloseTab),
        ]),
        Menu::new(t("View")).items([
            MenuItem::action(t("Open Editor Window"), OpenEditor),
            MenuItem::separator(),
            MenuItem::action(t("Toggle Sidebar"), ToggleSidebar),
            MenuItem::action(t("Toggle Git Changes"), ToggleGitPanel),
            MenuItem::action(t("Open a Surface…"), OpenSurface),
            MenuItem::action(t("Toggle Terminal"), ToggleTerminal),
            MenuItem::separator(),
            MenuItem::action(t("Refresh"), RefreshProject),
        ]),
    ]);
}

fn main() {
    // Agents and computer use run as CodeAny itself (see `agl_bridge::set_home`);
    // those launches come back here first.
    if let Some(code) = agl::app::hold_from_args() {
        std::process::exit(code);
    }
    // Started from a terminal (`make dev`), leave it: an interactive login
    // shell agl runs to read PATH would otherwise take the terminal's
    // foreground and keep it, so Ctrl-C stops reaching the terminal. Done
    // here, before any thread exists, so children still start with
    // posix_spawn: forking this multithreaded app crashes the child on macOS.
    // From Finder there is no terminal, and this changes nothing.
    #[cfg(unix)]
    // SAFETY: setsid only changes this process's session; no threads yet.
    unsafe {
        libc::setsid();
    }
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
    // The interface language, before anything is drawn.
    i18n::apply(i18n::preference_from_setting(
        db::Db::open(&paths::database()).ok().and_then(|db| db.setting::<Option<String>>("language")).flatten(),
    ));
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
                KeyBinding::new("cmd-t", NewTab, Some("Workspace")),
                KeyBinding::new("cmd-shift-t", ReopenTab, Some("Workspace")),
                KeyBinding::new("ctrl-tab", NextTab, Some("Workspace")),
                KeyBinding::new("ctrl-shift-tab", PrevTab, Some("Workspace")),
                KeyBinding::new("cmd-shift-]", NextTab, Some("Workspace")),
                KeyBinding::new("cmd-shift-[", PrevTab, Some("Workspace")),
                KeyBinding::new("cmd-alt-right", NextTab, Some("Workspace")),
                KeyBinding::new("cmd-alt-left", PrevTab, Some("Workspace")),
                KeyBinding::new("cmd-1", SelectTab1, Some("Workspace")),
                KeyBinding::new("cmd-2", SelectTab2, Some("Workspace")),
                KeyBinding::new("cmd-3", SelectTab3, Some("Workspace")),
                KeyBinding::new("cmd-4", SelectTab4, Some("Workspace")),
                KeyBinding::new("cmd-5", SelectTab5, Some("Workspace")),
                KeyBinding::new("cmd-6", SelectTab6, Some("Workspace")),
                KeyBinding::new("cmd-7", SelectTab7, Some("Workspace")),
                KeyBinding::new("cmd-8", SelectTab8, Some("Workspace")),
                KeyBinding::new("cmd-9", SelectLastTab, Some("Workspace")),
                KeyBinding::new("cmd-alt-t", OpenSurface, Some("Workspace")),
                KeyBinding::new("cmd-p", Search, Some("Workspace")),
            ]);
            terminal::init(cx);
            model_picker::init(cx);

            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_app_quit(|_| async {
                agl_bridge::shutdown();
            })
            .detach();
            #[cfg(target_os = "macos")]
            if updater::start(cx) {
                cx.set_global(UpdaterRunning);
                cx.on_action(|_: &CheckForUpdates, _| updater::check());
            }
            set_app_menus(cx);

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
