//! Create a project, or edit one: a name, the device it is on, and the
//! folder it works in, which the person picks.

use crate::style::AppButtonStyle as _;

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    menu::DropdownMenu as _,
    WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::i18n::{t, tf};
use crate::agl_bridge;
use crate::style::layout;
use crate::style::size::{ICON, TEXT};
use crate::threads::{ProjectIcon, folder_label};
use crate::project_avatar::ProjectAvatar;

pub const WIDTH: f32 = 480.;

/// A device a project can be made on.
#[derive(Clone)]
pub struct DialogDevice {
    /// `None` is this one.
    pub id: Option<String>,
    pub name: String,
    pub online: bool,
}

pub enum ProjectDialogEvent {
    /// Make a project called `name` of `folder` on `device` (`None`: this one).
    Create { name: String, device: Option<String>, folder: PathBuf },
    Rename { project: PathBuf, name: String, icon: Option<ProjectIcon> },
    Cancel,
}

pub struct ProjectDialog {
    name: Entity<InputState>,
    /// The project being renamed; `None` while making one.
    renaming: Option<PathBuf>,
    icon: Option<ProjectIcon>,
    importing_icon: bool,
    icon_error: bool,
    devices: Vec<DialogDevice>,
    device: Option<String>,
    folder: Option<PathBuf>,
    /// The dialog that picks a folder of another device, while it is up.
    picker: Option<(Entity<FolderPicker>, Subscription)>,
    _subscription: Subscription,
}

impl EventEmitter<ProjectDialogEvent> for ProjectDialog {}

impl ProjectDialog {
    /// A dialog for a new project on `device`, or one renaming `renaming`
    /// (its folder, and what it is called now).
    pub fn new(
        devices: Vec<DialogDevice>,
        device: Option<String>,
        renaming: Option<(PathBuf, String, Option<ProjectIcon>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(t("Project name"));
            if let Some((_, current, _)) = &renaming {
                input.set_value(current.clone(), window, cx);
            }
            input
        });
        let _subscription = cx.subscribe(&name, |this: &mut Self, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter { .. } => this.submit(cx),
            // The button follows whether there is a name.
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        Self {
            name,
            icon: renaming.as_ref().and_then(|(_, _, icon)| icon.clone()),
            importing_icon: false,
            icon_error: false,
            renaming: renaming.map(|(project, _, _)| project),
            devices,
            device,
            folder: None,
            picker: None,
            _subscription,
        }
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.name.update(cx, |input, cx| input.focus(window, cx));
    }

    /// What the project will be called: what was typed, else its folder's name.
    fn title(&self, cx: &App) -> Option<String> {
        let typed = self.name.read(cx).value().trim().to_string();
        if !typed.is_empty() {
            return Some(typed);
        }
        self.folder.as_deref().map(folder_label)
    }

    /// Whether there is enough to go on: a name to rename to, a folder for
    /// a new project.
    fn ready(&self, cx: &App) -> bool {
        if self.importing_icon { return false; }
        match self.renaming {
            Some(_) => self.title(cx).is_some(),
            None => self.folder.is_some(),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.ready(cx) { return; }
        let Some(name) = self.title(cx) else { return };
        match (self.renaming.clone(), self.folder.clone()) {
            (Some(project), _) => cx.emit(ProjectDialogEvent::Rename { project, name, icon: self.icon.clone() }),
            (None, Some(folder)) => cx.emit(ProjectDialogEvent::Create { name, device: self.device.clone(), folder }),
            // A project is a folder: there is none to make yet.
            (None, None) => {}
        }
    }

    fn set_device(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        if self.device != device {
            // A folder is a folder of one device.
            self.device = device;
            self.folder = None;
            cx.notify();
        }
    }

    /// Pick the project's folder in a dialog of its own: the system's here,
    /// one that lists the device's folders elsewhere.
    fn add_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(machine) = self.device.clone() {
            let picker = cx.new(|cx| FolderPicker::new(machine, window, cx));
            let subscription = cx.subscribe_in(&picker, window, |this, _, event: &FolderPickerEvent, window, cx| {
                // The picker is the dialog on top.
                window.close_dialog(cx);
                this.picker = None;
                if let FolderPickerEvent::Pick(dir) = event {
                    this.folder = Some(dir.clone());
                }
                cx.notify();
            });
            self.picker = Some((picker.clone(), subscription));
            window.open_dialog(cx, move |dialog, _, _| {
                dialog
                    .w(px(WIDTH))
                    .centered()
                    .title(t("Choose source folder"))
                    // Enter belongs to the path being typed.
                    .on_ok(|_, _, _| false)
                    .child(picker.clone())
            });
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t("Add").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            this.update(cx, |this, cx| {
                // The device changed while the dialog was up.
                if this.device.is_none() {
                    this.folder = Some(path);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn device_name(&self) -> String {
        self.devices
            .iter()
            .find(|d| d.id == self.device)
            .map(|d| d.name.clone())
            .unwrap_or_else(crate::remote::this_name)
    }

    fn choose_icon_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.importing_icon = true;
        self.icon_error = false;
        cx.notify();
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("Choose image…").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let selected = path.is_some();
            let icon = match path {
                Some(path) => cx.background_executor()
                    .spawn(async move { crate::project_avatar::import_icon(&path) }).await,
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.importing_icon = false;
                this.icon_error = selected && icon.is_none();
                if icon.is_some() { this.icon = icon; }
                cx.notify();
            });
        }).detach();
    }

    fn render_icon(&self, project: PathBuf, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.entity().downgrade();
        let name = self.title(cx).unwrap_or_else(|| folder_label(&project));
        div().flex().flex_col().gap(layout::SPACE_2)
            .child(div().flex().items_center().gap(layout::SPACE_2)
                .child(ProjectAvatar { project, name: name.into(), icon: self.icon.clone(), size: None })
                .child(Button::new("project-emoji").ghost().disabled(self.importing_icon).app_label(t("Emoji"))
                    .dropdown_menu(move |menu, _, _| {
                        ["📁", "💻", "🚀", "🛠️", "🎨", "📚", "🧪", "🌱", "⭐", "💡"]
                            .into_iter().fold(menu, |menu, emoji| {
                                let panel = panel.clone();
                                menu.item(crate::style::menu_item(emoji).on_click(move |_, _, cx| {
                                    let _ = panel.update(cx, |this, cx| {
                                        this.icon = Some(ProjectIcon::Emoji(emoji.into()));
                                        this.icon_error = false;
                                        cx.notify();
                                    });
                                }))
                            })
                    }))
                .child(Button::new("project-image").ghost().disabled(self.importing_icon).app_label(t("Choose image…"))
                    .on_click(cx.listener(|this, _, window, cx| this.choose_icon_image(window, cx))))
                .when(self.icon.is_some(), |this| this.child(
                    Button::new("reset-project-icon").ghost().disabled(self.importing_icon).app_label(t("Reset icon"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.icon = None;
                            this.icon_error = false;
                            cx.notify();
                        })),
                )))
            .when(self.icon_error, |this| this.child(
                div().text_size(TEXT).text_color(cx.theme().danger)
                    .child(t("Choose a PNG, JPEG, WebP or SVG image up to 4 MB.")),
            ))
    }

    fn render_folder(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let frame = div().rounded_lg().border_1().border_color(theme.border);
        if let Some(folder) = &self.folder {
            return frame
                .h(px(40.))
                .px(layout::SPACE_2)
                .flex()
                .items_center()
                .gap(layout::SPACE_2)
                .text_size(TEXT)
                .child(Icon::new(IconName::Folder).size(ICON).flex_none().text_color(theme.muted_foreground))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(folder.display().to_string()),
                )
                .child(
                    Button::new("project-folder-clear")
                        .ghost()
                        .small()
                        .app_icon(IconName::X)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.folder = None;
                            cx.notify();
                        })),
                )
                .into_any_element();
        }
        // Where the folder is: this device, or another that is online.
        let devices = self.devices.clone();
        let (current, dialog) = (self.device.clone(), cx.entity().downgrade());
        let device = Button::new("project-device")
            .ghost()
            .small()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_1)
                    .text_size(TEXT)
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.device_name())
                    .child(Icon::new(IconName::ChevronDown).size(ICON).text_color(theme.muted_foreground)),
            )
            .dropdown_menu(move |mut menu, _, _| {
                for device in &devices {
                    let label = if device.online { device.name.clone() } else { format!("{} ({})", device.name, t("offline")) };
                    let (pick, dialog) = (device.id.clone(), dialog.clone());
                    menu = menu.item(
                        crate::style::menu_item(label)
                            .checked(device.id == current)
                            .disabled(!device.online)
                            .on_click(move |_, _, cx| {
                                dialog.update(cx, |this, cx| this.set_device(pick.clone(), cx)).ok();
                            }),
                    );
                }
                menu
            });
        frame
            .py_5()
            .px(layout::SPACE_2)
            .flex()
            .flex_col()
            .items_center()
            .gap(layout::SPACE_2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_1)
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("Add a folder on"))
                    .child(device),
            )
            .child(
                Button::new("project-folder-add")
                    .outline()
                    .small()
                    .app_icon(IconName::FolderPlus)
                    .app_label(t("Add"))
                    .on_click(cx.listener(|this, _, window, cx| this.add_folder(window, cx))),
            )
            .child(
                div()
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground.opacity(0.8))
                    .text_center()
                    .child(t("Choose a folder as the project's workspace.")),
            )
            .into_any_element()
    }
}

impl Render for ProjectDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let creating = self.renaming.is_none();
        let ready = self.ready(cx);
        let label = |text: &'static str| div().text_size(TEXT).font_weight(FontWeight::MEDIUM).child(t(text));
        // A folder picked first names the project until one is typed.
        let suggested = self.folder.as_deref().map(folder_label).filter(|_| self.name.read(cx).value().trim().is_empty());
        div()
            .flex()
            .flex_col()
            .gap(layout::SPACE_2)
            .when_some(self.renaming.clone(), |this, project| {
                this.child(label("Project icon")).child(self.render_icon(project, cx))
            })
            .child(label("Project name").when(!creating, |label| label.mt(layout::SPACE_2)))
            .child(Input::new(&self.name).text_size(TEXT))
            .when_some(suggested, |this, name| {
                this.child(
                    div()
                        .text_size(TEXT)
                        .text_color(cx.theme().muted_foreground)
                        .child(tf("It is called “{name}”, as its folder, unless you name it.", &[("name", name)])),
                )
            })
            .child(label("Source folder").mt(layout::SPACE_2))
            .map(|this| match self.renaming.clone() {
                None => this.child(self.render_folder(cx)),
                // A project is its folder: that stays as it is.
                Some(folder) => this.child(
                    div()
                        .h(px(40.))
                        .px(layout::SPACE_2)
                        .flex()
                        .items_center()
                        .gap(layout::SPACE_2)
                        .rounded_lg()
                        .border_1()
                        .border_color(cx.theme().border)
                        .text_size(TEXT)
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(IconName::Folder).size(ICON).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(folder.display().to_string()),
                        ),
                ),
            })
            .child(
                div()
                    .mt(layout::SPACE_2)
                    .flex()
                    .justify_end()
                    .gap(layout::SPACE_2)
                    .child(
                        Button::new("project-dialog-cancel")
                            .ghost()
                            .app_label(t("Cancel"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ProjectDialogEvent::Cancel))),
                    )
                    .child(
                        Button::new("project-dialog-submit")
                            .primary()
                            .disabled(!ready)
                            .app_label(t(if creating { "Create project" } else { "Save" }))
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
    }
}

pub enum FolderPickerEvent {
    Pick(PathBuf),
    Cancel,
}

/// Choose a folder on another device: type its path, or walk there. The
/// folders come from that device's agl, like the conversation's.
pub struct FolderPicker {
    machine: String,
    path: Entity<InputState>,
    dir: Option<PathBuf>,
    /// The folders in `dir`, while loading `None`.
    entries: Option<Result<Vec<String>, String>>,
    /// The name of a folder to make in `dir`, while one is typed.
    new_name: Entity<InputState>,
    naming: bool,
    /// Why the folder could not be made.
    new_error: Option<SharedString>,
    _load: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<FolderPickerEvent> for FolderPicker {}

impl FolderPicker {
    /// Opens on the device's home folder.
    fn new(machine: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let path = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/folder"));
        let new_name = cx.new(|cx| InputState::new(window, cx).placeholder(t("Folder name")));
        let _subscriptions = [
            cx.subscribe_in(&path, window, |this: &mut Self, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let typed = input.read(cx).value().trim().to_string();
                    if !typed.is_empty() {
                        this.go(PathBuf::from(typed), window, cx);
                    }
                }
            }),
            cx.subscribe_in(&new_name, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.make_folder(window, cx),
                InputEvent::Change if this.new_error.is_some() => {
                    this.new_error = None;
                    cx.notify();
                }
                _ => {}
            }),
        ];
        let home = agl_bridge::backend(Some(&machine)).home();
        let _load = Some(cx.spawn_in(window, async move |this, cx| {
            let home = home.await;
            this.update_in(cx, |this, window, cx| match home {
                Ok(home) => this.go(home, window, cx),
                Err(err) => {
                    this.entries = Some(Err(format!("{err:#}")));
                    cx.notify();
                }
            })
            .ok();
        }));
        Self { machine, path, dir: None, entries: None, new_name, naming: false, new_error: None, _load, _subscriptions }
    }

    /// Make the folder named in the box inside the one shown, and go into it.
    fn make_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.dir.clone() else { return };
        let name = self.new_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        // One folder, right here: not a path somewhere else.
        if name == "." || name == ".." || name.contains(['/', '\\']) {
            self.new_error = Some(t("A folder name cannot contain a slash.").into());
            cx.notify();
            return;
        }
        let made = cx.background_executor().spawn({
            let (machine, dir, name) = (self.machine.clone(), dir.clone(), name.clone());
            async move {
                let output = crate::exec::run(Some(&machine), &dir, &["mkdir", "--", &name], &[])?;
                if output.ok() { Ok(()) } else { Err(output.stderr.trim().to_string()) }
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let made = made.await;
            this.update_in(cx, |this, window, cx| match made {
                Ok(()) => {
                    this.naming = false;
                    this.new_error = None;
                    this.new_name.update(cx, |input, cx| input.set_value("", window, cx));
                    this.go(dir.join(&name), window, cx);
                }
                Err(err) => {
                    this.new_error = Some(err.into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn go(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.dir = Some(dir.clone());
        self.entries = None;
        self.path.update(cx, |input, cx| input.set_value(dir.display().to_string(), window, cx));
        let list = agl_bridge::backend(Some(&self.machine)).list_dirs(dir.clone());
        self._load = Some(cx.spawn(async move |this, cx| {
            let entries = list.await.map_err(|err| format!("{err:#}"));
            this.update(cx, |this, cx| {
                // Moved on meanwhile.
                if this.dir.as_ref() == Some(&dir) {
                    this.entries = Some(entries);
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for FolderPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let dir = self.dir.clone();
        let parent = dir.as_ref().and_then(|d| d.parent()).map(PathBuf::from);
        let note = |text: SharedString, color: Hsla| div().p(layout::SPACE_2).text_size(TEXT).text_color(color).child(text);
        let list: AnyElement = match &self.entries {
            None => note(t("Loading…").into(), theme.muted_foreground).into_any_element(),
            Some(Err(err)) => note(err.clone().into(), theme.danger).into_any_element(),
            Some(Ok(entries)) if entries.is_empty() => note(t("No folders here").into(), theme.muted_foreground).into_any_element(),
            Some(Ok(entries)) => div()
                .flex()
                .flex_col()
                .children(entries.iter().enumerate().map(|(ix, name)| {
                    let target = dir.clone().unwrap_or_default().join(name);
                    div()
                        .id(("source-folder", ix))
                        .h(px(32.))
                        .px(layout::SPACE_2)
                        .flex()
                        .items_center()
                        .gap(layout::SPACE_2)
                        .rounded(layout::RADIUS)
                        .cursor_pointer()
                        .text_size(TEXT)
                        .hover(|this| this.bg(theme.list_hover))
                        .child(Icon::new(IconName::Folder).size(ICON).flex_none().text_color(theme.muted_foreground))
                        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(name.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| this.go(target.clone(), window, cx)))
                }))
                .into_any_element(),
        };
        // The folder listed is the one used; a path that failed to list is not.
        let usable = dir.clone().filter(|_| matches!(self.entries, Some(Ok(_))));
        // A new folder goes into the one listed.
        let new_folder: AnyElement = if self.naming && usable.is_some() {
            div()
                .flex()
                .items_center()
                .gap(layout::SPACE_1)
                .child(div().w(px(170.)).child(Input::new(&self.new_name).small().text_size(TEXT)))
                .child(
                    Button::new("source-folder-make")
                        .ghost()
                        .small()
                        .app_icon(IconName::Check)
                        .on_click(cx.listener(|this, _, window, cx| this.make_folder(window, cx))),
                )
                .child(
                    Button::new("source-folder-make-cancel")
                        .ghost()
                        .small()
                        .app_icon(IconName::X)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.naming = false;
                            this.new_error = None;
                            cx.notify();
                        })),
                )
                .into_any_element()
        } else {
            Button::new("source-folder-new")
                .ghost()
                .app_icon(IconName::FolderPlus)
                .app_label(t("New folder"))
                .disabled(usable.is_none())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.naming = true;
                    this.new_name.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(layout::SPACE_2)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_1)
                    .child(
                        Button::new("source-folder-up")
                            .ghost()
                            .small()
                            .app_icon(IconName::ArrowUp)
                            .disabled(parent.is_none())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(parent) = parent.clone() {
                                    this.go(parent, window, cx);
                                }
                            })),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&self.path).text_size(TEXT))),
            )
            .child(
                div()
                    .id("source-folder-list")
                    .h(px(300.))
                    .p(layout::SPACE_1)
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .overflow_y_scroll()
                    .child(list),
            )
            .when_some(self.new_error.clone(), |this, err| {
                this.child(div().text_size(TEXT).text_color(theme.danger).child(err))
            })
            .child(
                div()
                    .mt(layout::SPACE_2)
                    .flex()
                    .items_center()
                    .gap(layout::SPACE_2)
                    .child(new_folder)
                    .child(div().flex_1())
                    .child(
                        Button::new("source-folder-cancel")
                            .ghost()
                            .app_label(t("Cancel"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FolderPickerEvent::Cancel))),
                    )
                    .child(
                        Button::new("source-folder-use")
                            .primary()
                            .disabled(usable.is_none())
                            .app_label(t("Use this folder"))
                            .when_some(usable, |this, dir| {
                                this.on_click(cx.listener(move |_, _, _, cx| cx.emit(FolderPickerEvent::Pick(dir.clone()))))
                            }),
                    ),
            )
    }
}
