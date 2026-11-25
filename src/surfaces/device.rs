//! Device: a live view of an iOS Simulator, through `xcrun simctl`.
//!
//! Frames are screenshots taken a few times a second while the surface is
//! shown. Each is decoded here and the previous one freed explicitly: handing
//! GPUI a new image per frame would keep every decoded frame (about 12 MB on
//! a recent iPhone) in its image cache.

use crate::style::AppButtonStyle as _;

use std::{
    path::PathBuf,
    process::Command,
    sync::{Arc, OnceLock},
    time::Duration,
};

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::DropdownMenu as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;

use super::{Extension, Host, Manifest, Surface, SurfaceContext, SurfaceView, empty_state};
use crate::i18n::{t, tf};
use crate::style::size::TEXT;

const FRAME_INTERVAL: Duration = Duration::from_millis(400);

pub struct Device {
    manifest: Manifest,
}

impl Default for Device {
    fn default() -> Self {
        Self {
            manifest: Manifest {
                id: "device",
                name: "Device",
                icon: IconName::Smartphone,
                key: 'M',
            },
        }
    }
}

/// Xcode's developer directory: simctl ships with Xcode, not the Command
/// Line Tools that `xcode-select` often points at.
fn developer_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::var_os("DEVELOPER_DIR")
            .map(PathBuf::from)
            .into_iter()
            .chain(["/Applications/Xcode.app", "/Applications/Xcode-beta.app"].map(|app| {
                PathBuf::from(app).join("Contents/Developer")
            }))
            .find(|dir| dir.join("Platforms/iPhoneSimulator.platform").exists())
    })
    .as_ref()
}

fn simctl(args: &[&str]) -> Result<Vec<u8>, String> {
    let dir = developer_dir().ok_or_else(|| t("Xcode is not installed").to_string())?;
    let output = Command::new("xcrun")
        .arg("simctl")
        .args(args)
        .env("DEVELOPER_DIR", dir)
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Sim {
    udid: String,
    name: String,
    runtime: String,
    booted: bool,
}

/// Available simulators, booted ones first. Blocks.
fn list_devices() -> Result<Vec<Sim>, String> {
    let json: Value = serde_json::from_slice(&simctl(&["list", "devices", "available", "-j"])?)
        .map_err(|err| err.to_string())?;
    let mut sims: Vec<Sim> = json["devices"]
        .as_object()
        .into_iter()
        .flatten()
        .flat_map(|(runtime, devices)| {
            // com.apple.CoreSimulator.SimRuntime.iOS-26-0 -> iOS 26.0
            let runtime = runtime
                .rsplit('.')
                .next()
                .unwrap_or(runtime)
                .replacen('-', " ", 1)
                .replace('-', ".");
            devices.as_array().into_iter().flatten().map(move |d| Sim {
                udid: d["udid"].as_str().unwrap_or("").to_string(),
                name: d["name"].as_str().unwrap_or("").to_string(),
                runtime: runtime.clone(),
                booted: d["state"] == "Booted",
            })
        })
        .filter(|sim| sim.runtime.starts_with("iOS"))
        .collect();
    sims.sort_by(|a, b| b.booted.cmp(&a.booted).then(b.runtime.cmp(&a.runtime)).then(a.name.cmp(&b.name)));
    Ok(sims)
}

/// One screenshot as a GPUI image (BGRA). Blocks.
fn capture(udid: &str) -> Result<Arc<RenderImage>, String> {
    let path = std::env::temp_dir().join(format!("codeany-sim-{udid}.png"));
    simctl(&["io", udid, "screenshot", "--type=png", &path.to_string_lossy()])?;
    let mut rgba = image::open(&path).map_err(|err| err.to_string())?.into_rgba8();
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
}

impl Extension for Device {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn unavailable(&self, _ctx: &SurfaceContext) -> Option<&'static str> {
        developer_dir().is_none().then_some("Requires Xcode")
    }

    fn build(&self, _ctx: &SurfaceContext, _host: Host, _window: &mut Window, cx: &mut App) -> Box<dyn Surface> {
        Box::new(cx.new(|cx| {
            let mut surface = DeviceSurface {
                sims: Vec::new(),
                selected: None,
                frame: None,
                error: None,
                active: true,
                _mirror: None,
            };
            surface.reload_devices(cx);
            surface
        }))
    }
}

pub struct DeviceSurface {
    sims: Vec<Sim>,
    selected: Option<String>,
    frame: Option<Arc<RenderImage>>,
    error: Option<String>,
    active: bool,
    _mirror: Option<Task<()>>,
}

impl DeviceSurface {
    fn reload_devices(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let sims = cx.background_executor().spawn(async { list_devices() }).await;
            this.update(cx, |this, cx| {
                match sims {
                    Ok(sims) => {
                        if this.selected.as_ref().is_none_or(|udid| !sims.iter().any(|s| &s.udid == udid)) {
                            this.selected = sims.first().map(|s| s.udid.clone());
                        }
                        this.sims = sims;
                        this.error = None;
                    }
                    Err(err) => this.error = Some(err),
                }
                this.restart_mirror(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn selected_sim(&self) -> Option<&Sim> {
        let udid = self.selected.as_ref()?;
        self.sims.iter().find(|s| &s.udid == udid)
    }

    /// Take screenshots while shown and the device is running.
    fn restart_mirror(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.selected_sim().filter(|s| s.booted && self.active).map(|s| s.udid.clone()) else {
            self._mirror = None;
            return;
        };
        self._mirror = Some(cx.spawn(async move |this, cx| {
            loop {
                let shot = {
                    let udid = udid.clone();
                    cx.background_executor().spawn(async move { capture(&udid) }).await
                };
                let alive = this.update(cx, |this, cx| {
                    match shot {
                        Ok(frame) => {
                            if let Some(old) = this.frame.replace(frame) {
                                // Free the last frame's texture; see the module docs.
                                cx.defer(move |cx| {
                                    if let Some(window) = cx.active_window() {
                                        let _ = window.update(cx, |_, window, _| window.drop_image(old));
                                    }
                                });
                            }
                            this.error = None;
                        }
                        Err(err) => this.error = Some(err),
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(FRAME_INTERVAL).await;
            }
        }));
    }

    fn select(&mut self, udid: String, cx: &mut Context<Self>) {
        self.selected = Some(udid);
        self.frame = None;
        self.restart_mirror(cx);
        cx.notify();
    }

    fn boot(&mut self, cx: &mut Context<Self>) {
        let Some(udid) = self.selected.clone() else { return };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    simctl(&["boot", &udid])?;
                    // The Simulator app shows the device and keeps it responsive.
                    let _ = Command::new("open").args(["-a", "Simulator"]).status();
                    Ok::<_, String>(())
                })
                .await;
            this.update(cx, |this, cx| {
                if let Err(err) = result {
                    this.error = Some(err);
                }
                this.reload_devices(cx);
            })
            .ok();
        })
        .detach();
    }
}

impl SurfaceView for DeviceSurface {
    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        self.restart_mirror(cx);
    }

    fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.reload_devices(cx);
    }
}

impl Render for DeviceSurface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let sim = self.selected_sim().cloned();
        let sims = self.sims.clone();
        let current = self.selected.clone();
        let entity = cx.entity().downgrade();
        let picker = Button::new("device-picker")
            .ghost()
            .small()
            .app_icon(IconName::Smartphone)
            .app_label(
                sim.as_ref()
                    .map(|s| format!("{} · {}", s.name, s.runtime))
                    .unwrap_or_else(|| t("No simulator").into()),
            )
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for sim in &sims {
                    let (entity, udid) = (entity.clone(), sim.udid.clone());
                    let device = format!("{} · {}", sim.name, sim.runtime);
                    let label = if sim.booted { tf("{device} · running", &[("device", device)]) } else { device };
                    menu = menu.item(
                        crate::style::menu_item(label)
                            .checked(current.as_deref() == Some(sim.udid.as_str()))
                            .on_click(move |_, _, cx| {
                                let udid = udid.clone();
                                entity.update(cx, |this, cx| this.select(udid, cx)).ok();
                            }),
                    );
                }
                menu
            });

        let body: AnyElement = match (&sim, &self.frame, &self.error) {
            (_, _, Some(err)) if self.frame.is_none() => empty_state(err.clone(), cx).into_any_element(),
            (None, _, _) => empty_state(t("No iOS simulators are installed"), cx).into_any_element(),
            (Some(sim), _, _) if !sim.booted => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(tf("{name} is not running", &[("name", sim.name.clone())]))
                .child(
                    Button::new("boot-device")
                        .primary()
                        .small()
                        .app_label(t("Boot"))
                        .on_click(cx.listener(|this, _, _, cx| this.boot(cx))),
                )
                .into_any_element(),
            (_, Some(frame), _) => div()
                .size_full()
                .p_4()
                .flex()
                .items_center()
                .justify_center()
                .child(img(frame.clone()).size_full().object_fit(ObjectFit::Contain))
                .into_any_element(),
            _ => empty_state(t("Connecting…"), cx).into_any_element(),
        };

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
                    .child(picker)
                    .child(div().flex_1())
                    .when(sim.as_ref().is_some_and(|s| s.booted), |this| {
                        this.child(
                            Button::new("open-simulator")
                                .ghost()
                                .xsmall()
                                .app_icon(IconName::ExternalLink)
                                .on_click(|_, _, _| {
                                    let _ = Command::new("open").args(["-a", "Simulator"]).status();
                                })
                                .app_tooltip(t("Open in Simulator")),
                        )
                    }),
            )
            .child(div().flex_1().min_h_0().bg(theme.sidebar).child(body))
    }
}
