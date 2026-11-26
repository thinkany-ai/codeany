//! Surfaces: the panels that open beside a conversation (browser, terminal,
//! files, diff, pull request, device). Each one is an extension: a
//! [`Manifest`] that says what it is, a check for whether it can open in the
//! current context, and a view it builds on demand.
//!
//! Extensions are compiled in and listed in [`builtin`]. The API keeps hosts
//! and extensions apart — an extension only sees a [`SurfaceContext`] and asks
//! the host for anything beyond its own view through [`Host`] — so other
//! kinds of extension (out of process, scripted) can implement it later.

mod browser;
mod device;
mod diff;
mod files;
mod pull_request;
mod terminal;

use std::{path::PathBuf, rc::Rc};

use gpui_kit::assets::IconName;
use gpui_kit::*;
use crate::style::size::TEXT;

/// What every surface knows about where it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceContext {
    /// The open project, if any.
    pub project: Option<PathBuf>,
    /// The project's git work tree, if it is in one.
    pub git_root: Option<PathBuf>,
}

/// Things a surface asks the host to do.
pub enum Request {
    /// Open a file in the editor.
    OpenFile(PathBuf),
    /// Open a URL in the browser surface.
    OpenUrl(String),
}

/// The host side of an extension: how a surface reaches the rest of the app.
#[derive(Clone)]
pub struct Host(Rc<dyn Fn(Request, &mut Window, &mut App)>);

impl Host {
    pub fn new(handler: impl Fn(Request, &mut Window, &mut App) + 'static) -> Self {
        Self(Rc::new(handler))
    }

    pub fn request(&self, request: Request, window: &mut Window, cx: &mut App) {
        (self.0)(request, window, cx)
    }
}

pub struct Manifest {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: IconName,
    /// The key that opens it from the surface picker.
    pub key: char,
}

/// A surface extension.
pub trait Extension {
    fn manifest(&self) -> &Manifest;

    /// Why the surface cannot open in this context, if it cannot.
    fn unavailable(&self, _ctx: &SurfaceContext) -> Option<&'static str> {
        None
    }

    fn build(&self, ctx: &SurfaceContext, host: Host, window: &mut Window, cx: &mut App) -> Box<dyn Surface>;
}

/// A surface's view, as the host drives it.
pub trait SurfaceView: Render {
    /// The project or repository changed.
    fn set_context(&mut self, _ctx: &SurfaceContext, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Shown or hidden (another tab, or the panel closed). Native views such
    /// as the browser hide themselves, since they draw above the app.
    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Something on disk may have changed, e.g. after an agent's turn.
    fn refresh(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    /// Called when the user asks to open a URL.
    fn open_url(&mut self, _url: &str, _window: &mut Window, _cx: &mut Context<Self>) {}
}

/// An open surface, type-erased for the host.
pub trait Surface {
    fn view(&self) -> AnyView;
    fn set_context(&self, ctx: &SurfaceContext, window: &mut Window, cx: &mut App);
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    fn refresh(&self, window: &mut Window, cx: &mut App);
    fn open_url(&self, url: &str, window: &mut Window, cx: &mut App);
}

impl<V: SurfaceView> Surface for Entity<V> {
    fn view(&self) -> AnyView {
        self.clone().into()
    }

    fn set_context(&self, ctx: &SurfaceContext, window: &mut Window, cx: &mut App) {
        self.update(cx, |view, cx| view.set_context(ctx, window, cx));
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |view, cx| view.set_active(active, window, cx));
    }

    fn refresh(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |view, cx| view.refresh(window, cx));
    }

    fn open_url(&self, url: &str, window: &mut Window, cx: &mut App) {
        self.update(cx, |view, cx| view.open_url(url, window, cx));
    }
}

/// The built-in extensions, in picker order.
pub fn builtin() -> Vec<Rc<dyn Extension>> {
    vec![
        Rc::new(browser::Browser::default()),
        Rc::new(terminal::TerminalExt::default()),
        Rc::new(files::Files::default()),
        Rc::new(diff::Diff::default()),
        Rc::new(pull_request::PullRequest::default()),
        // iOS simulators: Xcode is macOS only.
        #[cfg(target_os = "macos")]
        Rc::new(device::Device::default()),
    ]
}

/// The user's login-shell PATH, so CLIs installed outside the system dirs
/// (gh, adb) are found when the app is launched from Finder.
pub(crate) fn shell_path() -> std::ffi::OsString {
    agl::shellenv::path_var()
}

/// A placeholder for a surface with nothing to show yet.
pub(crate) fn empty_state(text: impl Into<SharedString>, cx: &App) -> Div {
    use gpui_kit::component::ActiveTheme as _;
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px_6()
        .text_size(TEXT)
        .text_color(cx.theme().muted_foreground)
        .text_center()
        .child(text.into())
}
