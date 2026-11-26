// A GUI app on Windows: no console window opens with it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

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
mod keymap;
mod model_picker;
mod settings;
mod panel_views;
mod paths;
mod peer;
mod project_avatar;
mod project_filter;
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
/// Sparkle is macOS only; elsewhere nothing ever offers an update.
#[cfg(not(target_os = "macos"))]
mod updater {
    pub struct AvailableUpdate(pub Option<String>);
    impl gpui_kit::Global for AvailableUpdate {}
    pub fn check() {}
}
mod workspace;
mod workspace_switcher;

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

            terminal::init(cx);
            model_picker::init(cx);
            // Last: it keeps the bindings above and adds the rebindable ones.
            keymap::init(cx);

            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_app_quit(|_| async {
                // The last save may still be on its way to the disk.
                db::flush();
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
                window_min_size: Some(workspace::minimum_window_size()),
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
