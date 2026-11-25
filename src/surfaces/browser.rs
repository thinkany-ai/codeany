//! Browser: a real web page (WKWebView through wry), for previewing what the
//! agent builds. A blank browser offers the dev servers running locally.

use crate::style::AppButtonStyle as _;

use std::process::Command;

use futures::{StreamExt as _, channel::mpsc};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_webview::WebView;

use super::{Extension, Host, Manifest, Surface, SurfaceContext, SurfaceView, empty_state};
use crate::i18n::{t, tf};
use crate::style::size::{ICON, TEXT};

/// Ports dev servers usually listen on.
const DEV_PORTS: &[u16] = &[3000, 3001, 4000, 4200, 4321, 5000, 5173, 5174, 8000, 8080, 8787, 8888];

pub struct Browser {
    manifest: Manifest,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            manifest: Manifest {
                id: "browser",
                name: "Browser",
                icon: IconName::Globe,
                key: 'B',
            },
        }
    }
}

impl Extension for Browser {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn build(&self, _ctx: &SurfaceContext, _host: Host, window: &mut Window, cx: &mut App) -> Box<dyn Surface> {
        Box::new(cx.new(|cx| BrowserSurface::new(window, cx)))
    }
}

/// Make what was typed into a URL: add a scheme, or search for it.
fn to_url(input: &str) -> String {
    let input = input.trim();
    if input.contains("://") || input.starts_with("about:") {
        return input.to_string();
    }
    let host = input.split(['/', ':']).next().unwrap_or("");
    if host == "localhost" || host.parse::<std::net::Ipv4Addr>().is_ok() {
        return format!("http://{input}");
    }
    if input.contains('.') && !input.contains(' ') {
        return format!("https://{input}");
    }
    let query: String = input
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://www.google.com/search?q={query}")
}

/// Dev-server ports that something listens on. Blocks briefly.
fn listening_dev_ports() -> Vec<u16> {
    let Ok(out) = Command::new("lsof").args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fn"]).output() else {
        return Vec::new();
    };
    let mut ports: Vec<u16> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('n')?.rsplit(':').next()?.parse().ok())
        .filter(|port| DEV_PORTS.contains(port))
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
}

pub struct BrowserSurface {
    address: Entity<InputState>,
    /// The address bar's placeholder as last set, to follow language changes.
    placeholder: &'static str,
    webview: Option<Entity<WebView>>,
    error: Option<String>,
    active: bool,
    dev_ports: Vec<u16>,
    _subscriptions: Vec<Subscription>,
    _url_updates: Option<Task<()>>,
}

impl BrowserSurface {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = t("Search or enter a URL");
        let address = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let subscriptions = vec![cx.subscribe_in(&address, window, |this, address, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                let text = address.read(cx).value().to_string();
                if !text.trim().is_empty() {
                    this.load(&to_url(&text), window, cx);
                }
            }
        })];
        cx.spawn(async move |this, cx| {
            let ports = cx.background_executor().spawn(async { listening_dev_ports() }).await;
            this.update(cx, |this, cx| {
                this.dev_ports = ports;
                cx.notify();
            })
            .ok();
        })
        .detach();
        address.update(cx, |address, cx| address.focus(window, cx));
        Self {
            address,
            placeholder,
            webview: None,
            error: None,
            active: true,
            dev_ports: Vec::new(),
            _subscriptions: subscriptions,
            _url_updates: None,
        }
    }

    fn load(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.address
            .update(cx, |address, cx| address.set_value(url.to_string(), window, cx));
        if let Some(webview) = &self.webview {
            webview.update(cx, |webview, _| webview.load_url(url));
            return;
        }
        // Keep the address bar in step with in-page navigation.
        let (tx, mut rx) = mpsc::unbounded::<String>();
        let builder = wry::WebViewBuilder::new()
            .with_url(url)
            .with_on_page_load_handler(move |_, url| {
                let _ = tx.unbounded_send(url);
            });
        match WebView::build(builder, window, cx) {
            Ok(webview) => {
                self.webview = Some(cx.new(|_| webview));
                self.error = None;
                self._url_updates = Some(cx.spawn_in(window, async move |this, cx| {
                    while let Some(url) = rx.next().await {
                        let alive = this.update_in(cx, |this, window, cx| {
                            this.address
                                .update(cx, |address, cx| address.set_value(url, window, cx));
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                }));
            }
            Err(err) => self.error = Some(tf("Could not create a web view: {err}", &[("err", err.to_string())])),
        }
        cx.notify();
    }

    fn navigate(&mut self, back: bool, cx: &mut Context<Self>) {
        if let Some(webview) = &self.webview {
            webview.update(cx, |webview, _| {
                let _ = if back { webview.back() } else { webview.forward() };
            });
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(webview) = &self.webview {
            let _ = webview.read(cx).raw().reload();
        }
    }

    fn render_start(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if self.dev_ports.is_empty() {
            return empty_state(t("Enter a URL above, e.g. localhost:3000"), cx).into_any_element();
        }
        div()
            .p_4()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .mb_1()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("Running on this Mac")),
            )
            .children(self.dev_ports.iter().map(|port| {
                let url = format!("http://localhost:{port}");
                div()
                    .id(("dev-port", *port as usize))
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(TEXT)
                    .hover(|this| this.bg(theme.list_hover))
                    .child(Icon::new(IconName::Globe).size(ICON).text_color(theme.muted_foreground))
                    .child(format!("localhost:{port}"))
                    .on_click(cx.listener(move |this, _, window, cx| this.load(&url, window, cx)))
            }))
            .into_any_element()
    }
}

impl SurfaceView for BrowserSurface {
    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        if let Some(webview) = &self.webview {
            webview.update(cx, |webview, _| if active { webview.show() } else { webview.hide() });
        }
    }

    fn open_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.load(url, window, cx);
    }
}

impl Render for BrowserSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // The native view draws above everything; keep it out of a dialog's way.
        let covered = window.has_active_dialog(cx);
        if let Some(webview) = &self.webview {
            let show = self.active && !covered;
            webview.update(cx, |webview, _| {
                if show != webview.visible() {
                    if show { webview.show() } else { webview.hide() }
                }
            });
        }
        let placeholder = t("Search or enter a URL");
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.address.update(cx, |address, cx| address.set_placeholder(placeholder, window, cx));
        }
        let has_page = self.webview.is_some();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("browser-back")
                            .ghost()
                            .xsmall()
                            .app_icon(IconName::ArrowLeft)
                            .disabled(!has_page)
                            .on_click(cx.listener(|this, _, _, cx| this.navigate(true, cx))),
                    )
                    .child(
                        Button::new("browser-forward")
                            .ghost()
                            .xsmall()
                            .app_icon(IconName::ArrowRight)
                            .disabled(!has_page)
                            .on_click(cx.listener(|this, _, _, cx| this.navigate(false, cx))),
                    )
                    .child(
                        Button::new("browser-reload")
                            .ghost()
                            .xsmall()
                            .app_icon(IconName::RotateCw)
                            .disabled(!has_page)
                            .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .mx_1()
                            .px_2()
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_size(TEXT)
                            .child(Input::new(&self.address).text_size(TEXT).appearance(false).small()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|this| match (&self.webview, &self.error) {
                        (_, Some(err)) => this.child(empty_state(err.clone(), cx)),
                        (Some(webview), None) => this.child(webview.clone()),
                        (None, None) => this.child(self.render_start(cx)),
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::to_url;

    #[test]
    fn typed_addresses_become_urls() {
        assert_eq!(to_url("localhost:3000"), "http://localhost:3000");
        assert_eq!(to_url("127.0.0.1:8080/x"), "http://127.0.0.1:8080/x");
        assert_eq!(to_url("github.com/a"), "https://github.com/a");
        assert_eq!(to_url("https://x.dev"), "https://x.dev");
        assert_eq!(to_url("rust gpui"), "https://www.google.com/search?q=rust+gpui");
    }
}
