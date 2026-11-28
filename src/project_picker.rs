//! The composer's project menu: search the device's projects and pick the
//! one a new thread works in, make a new one, or work in none.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::i18n::t;
use crate::style::layout;
use crate::style::size::{ICON, TEXT};
use crate::threads::project_label;

pub enum ProjectPickerEvent {
    /// Work in this project; `None` works in none.
    Pick(Option<PathBuf>),
    /// Make a new project.
    New,
}

pub struct ProjectPicker {
    input: Entity<InputState>,
    projects: Vec<PathBuf>,
    selected: Option<PathBuf>,
    _subscription: Subscription,
}

impl EventEmitter<ProjectPickerEvent> for ProjectPicker {}

impl ProjectPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Search projects…")));
        let _subscription = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| match event {
            InputEvent::Change => cx.notify(),
            // Enter picks the first project that matches.
            InputEvent::PressEnter { .. } => {
                if let Some(first) = this.matches(cx).into_iter().next() {
                    cx.emit(ProjectPickerEvent::Pick(Some(first)));
                }
            }
            _ => {}
        });
        Self { input, projects: Vec::new(), selected: None, _subscription }
    }

    /// What to list as the menu opens: a fresh search, focused.
    pub fn open(&mut self, projects: Vec<PathBuf>, selected: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.projects = projects;
        self.selected = selected;
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn matches(&self, cx: &App) -> Vec<PathBuf> {
        let query = self.input.read(cx).value().trim().to_lowercase();
        self.projects
            .iter()
            .filter(|p| query.is_empty() || project_label(p).to_lowercase().contains(&query))
            .cloned()
            .collect()
    }
}

impl Render for ProjectPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let row = |id: ElementId, checked: bool| {
            div()
                .id(id)
                .h(px(32.))
                .flex_none()
                .px(layout::SPACE_2)
                .flex()
                .items_center()
                .gap(layout::SPACE_2)
                .rounded(layout::RADIUS)
                .cursor_pointer()
                .text_size(TEXT)
                .when(checked, |this| this.bg(theme.list_active))
                .when(!checked, |this| this.hover(|this| this.bg(theme.list_hover)))
        };
        let icon = |name: IconName| Icon::new(name).size(ICON).flex_none().text_color(theme.muted_foreground);
        let searching = !self.input.read(cx).value().trim().is_empty();
        let matches = self.matches(cx);

        let mut list = div().id("project-picker-list").max_h(px(300.)).overflow_y_scroll().flex().flex_col().gap(px(2.));
        if !searching {
            list = list.child(
                row("new-project".into(), false)
                    .child(icon(IconName::FolderPlus))
                    .child(div().flex_1().child(t("New project")))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProjectPickerEvent::New))),
            );
        }
        if searching && matches.is_empty() {
            list = list.child(
                div()
                    .px(layout::SPACE_2)
                    .py(layout::SPACE_2)
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("No matching projects")),
            );
        }
        for (ix, project) in matches.into_iter().enumerate() {
            let selected = self.selected.as_ref() == Some(&project);
            let pick = project.clone();
            list = list.child(
                row(("project-pick", ix).into(), selected)
                    .child(icon(IconName::Folder))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(project_label(&project)),
                    )
                    .when(selected, |this| this.child(Icon::new(IconName::Check).size(ICON).flex_none().text_color(theme.primary)))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ProjectPickerEvent::Pick(Some(pick.clone()))))),
            );
        }

        div()
            .w(px(260.))
            .p(layout::SPACE_2)
            .flex()
            .flex_col()
            .gap(layout::SPACE_2)
            .child(
                // Like the model picker's: no box, a rule under it.
                div().pb(px(4.)).border_b_1().border_color(theme.border).child(
                    Input::new(&self.input)
                        .small()
                        .text_size(TEXT)
                        .appearance(false)
                        .prefix(Icon::new(IconName::Search).size(ICON).text_color(theme.muted_foreground)),
                ),
            )
            .child(list)
            .child(
                div().pt(layout::SPACE_1).border_t_1().border_color(theme.border).child(
                    row("no-project".into(), false)
                        .child(icon(IconName::X))
                        .child(div().flex_1().child(t("Don't work in a project")))
                        .when(self.selected.is_none(), |this| {
                            this.child(Icon::new(IconName::Check).size(ICON).flex_none().text_color(theme.primary))
                        })
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ProjectPickerEvent::Pick(None)))),
                ),
            )
    }
}
