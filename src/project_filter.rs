//! The Projects section's filter menu: search the projects, show one of
//! them only (or all), and choose how they are sorted.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::i18n::t;
use crate::project_avatar::ProjectAvatar;
use crate::style::layout;
use crate::style::size::{ICON, TEXT};

pub enum ProjectFilterEvent {
    /// Show only this project; `None` shows all of them.
    Pick(Option<PathBuf>),
    /// Sort by name (true) or by last activity.
    SortByName(bool),
}

pub struct ProjectFilter {
    input: Entity<InputState>,
    projects: Vec<PathBuf>,
    selected: Option<PathBuf>,
    by_name: bool,
    _subscription: Subscription,
}

impl EventEmitter<ProjectFilterEvent> for ProjectFilter {}

fn name(project: &std::path::Path) -> String {
    project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.display().to_string())
}

impl ProjectFilter {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Search projects…")));
        let _subscription = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| match event {
            InputEvent::Change => cx.notify(),
            // Enter picks the first project that matches.
            InputEvent::PressEnter { .. } => {
                if let Some(first) = this.matches(cx).into_iter().next() {
                    cx.emit(ProjectFilterEvent::Pick(Some(first)));
                }
            }
            _ => {}
        });
        Self { input, projects: Vec::new(), selected: None, by_name: false, _subscription }
    }

    /// What to list as the menu opens: a fresh search, focused.
    pub fn open(
        &mut self,
        projects: Vec<PathBuf>,
        selected: Option<PathBuf>,
        by_name: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.projects = projects;
        self.selected = selected;
        self.by_name = by_name;
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
            .filter(|p| query.is_empty() || name(p).to_lowercase().contains(&query))
            .cloned()
            .collect()
    }
}

impl Render for ProjectFilter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let row = |id: ElementId, checked: bool| {
            div()
                .id(id)
                .h(px(32.))
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
        let check = |on: bool| {
            div()
                .flex_none()
                .size(ICON)
                .when(on, |this| this.child(Icon::new(IconName::Check).size(ICON).text_color(theme.primary)))
        };
        let searching = !self.input.read(cx).value().trim().is_empty();
        let matches = self.matches(cx);

        let mut list = div().id("project-filter-list").max_h(px(320.)).overflow_y_scroll().flex().flex_col().gap(px(2.));
        if !searching {
            list = list.child(
                row("all-projects".into(), self.selected.is_none())
                    .child(Icon::new(IconName::Folder).size(ICON).text_color(theme.muted_foreground))
                    .child(div().flex_1().child(t("All projects")))
                    .child(check(self.selected.is_none()))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProjectFilterEvent::Pick(None)))),
            );
        }
        if matches.is_empty() {
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
            let label: SharedString = name(&project).into();
            let pick = project.clone();
            list = list.child(
                row(("project-filter", ix).into(), selected)
                    .child(ProjectAvatar { project: project.clone(), name: label.clone() })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(label),
                    )
                    .child(check(selected))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ProjectFilterEvent::Pick(Some(pick.clone()))))),
            );
        }

        let sort = div()
            .mt(layout::SPACE_1)
            .pt(layout::SPACE_1)
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .px(layout::SPACE_2)
                    .py(px(4.))
                    .text_size(TEXT)
                    .text_color(theme.muted_foreground)
                    .child(t("Sort projects")),
            )
            .children([("Recently active", false), ("Name", true)].into_iter().map(|(label, value)| {
                let on = self.by_name == value;
                row(SharedString::from(format!("sort-{value}")).into(), false)
                    .child(div().flex_1().child(t(label)))
                    .child(check(on))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.by_name = value;
                        cx.emit(ProjectFilterEvent::SortByName(value));
                        cx.notify();
                    }))
            }));

        div()
            .w(px(240.))
            .p(layout::SPACE_2)
            .flex()
            .flex_col()
            .gap(layout::SPACE_2)
            .child(
                // Like the model picker's: no box, a rule under it.
                div()
                    .pb(px(4.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Input::new(&self.input)
                            .small()
                            .text_size(TEXT)
                            .appearance(false)
                            .prefix(Icon::new(IconName::Search).size(ICON).text_color(theme.muted_foreground)),
                    ),
            )
            .child(list)
            .child(sort)
    }
}
