//! The Projects section's filter menu: search the projects, show one of
//! them only (or all), and choose how they are sorted.

use std::path::{Path, PathBuf};

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

#[derive(Clone, Default, PartialEq, Eq)]
pub enum ProjectSelection {
    #[default]
    All,
    Project(PathBuf),
    ChatsOnly,
}

impl ProjectSelection {
    pub fn includes(&self, project: Option<&Path>) -> bool {
        match self {
            Self::All => true,
            Self::Project(path) => project == Some(path.as_path()),
            Self::ChatsOnly => project.is_none(),
        }
    }
}

pub enum ProjectFilterEvent {
    Pick(ProjectSelection),
    /// Sort by name (true) or by last activity.
    SortByName(bool),
    FlatByTime(bool),
}

pub struct ProjectFilter {
    input: Entity<InputState>,
    projects: Vec<PathBuf>,
    icons: std::collections::HashMap<PathBuf, crate::threads::ProjectIcon>,
    selected: ProjectSelection,
    by_name: bool,
    flat: bool,
    _subscription: Subscription,
}

impl EventEmitter<ProjectFilterEvent> for ProjectFilter {}

fn name(project: &std::path::Path) -> String {
    crate::threads::project_label(project)
}

impl ProjectFilter {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Search projects…")));
        let _subscription = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| match event {
            InputEvent::Change => cx.notify(),
            // Enter picks the first project that matches.
            InputEvent::PressEnter { .. } => {
                if let Some(first) = this.matches(cx).into_iter().next() {
                    cx.emit(ProjectFilterEvent::Pick(ProjectSelection::Project(first)));
                } else if this.matches_chats_only(cx) {
                    cx.emit(ProjectFilterEvent::Pick(ProjectSelection::ChatsOnly));
                }
            }
            _ => {}
        });
        Self { input, projects: Vec::new(), icons: Default::default(), selected: ProjectSelection::All, by_name: false, flat: false, _subscription }
    }

    /// What to list as the menu opens: a fresh search, focused.
    pub fn open(
        &mut self,
        projects: Vec<PathBuf>,
        icons: std::collections::HashMap<PathBuf, crate::threads::ProjectIcon>,
        selected: ProjectSelection,
        by_name: bool,
        flat: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.projects = projects;
        self.icons = icons;
        self.selected = selected;
        self.by_name = by_name;
        self.flat = flat;
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    fn matches_chats_only(&self, cx: &App) -> bool {
        let query = self.input.read(cx).value().trim().to_lowercase();
        t("Chats only").to_lowercase().contains(&query)
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
                row("all-projects".into(), self.selected == ProjectSelection::All)
                    .child(Icon::new(IconName::Folder).size(ICON).text_color(theme.muted_foreground))
                    .child(div().flex_1().child(t("All projects")))
                    .child(check(self.selected == ProjectSelection::All))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProjectFilterEvent::Pick(ProjectSelection::All)))),
            );
        }
        let show_chats_only = self.matches_chats_only(cx);
        if matches.is_empty() && !show_chats_only {
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
            let selected = matches!(&self.selected, ProjectSelection::Project(path) if path == &project);
            let label: SharedString = name(&project).into();
            let pick = project.clone();
            list = list.child(
                row(("project-filter", ix).into(), selected)
                    .child(ProjectAvatar { project: project.clone(), name: label.clone(), icon: self.icons.get(&project).cloned(), size: None })
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
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ProjectFilterEvent::Pick(ProjectSelection::Project(pick.clone()))))),
            );
        }

        if show_chats_only {
            let selected = self.selected == ProjectSelection::ChatsOnly;
            list = list.child(
                row("chats-only".into(), selected)
                    .child(Icon::new(IconName::MessageCircle).size(ICON).text_color(theme.muted_foreground))
                    .child(div().flex_1().child(t("Chats only")))
                    .child(check(selected))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProjectFilterEvent::Pick(ProjectSelection::ChatsOnly)))),
            );
        }

        let display = div()
            .pt(layout::SPACE_1)
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().px(layout::SPACE_2).py(px(4.)).text_size(TEXT)
                .text_color(theme.muted_foreground).child(t("Display mode")))
            .children([("Grouped by project", false), ("In one list, by time", true)].into_iter().map(|(label, value)| {
                row(("project-layout", usize::from(value)).into(), false)
                    .child(div().flex_1().child(t(label)))
                    .child(check(self.flat == value))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.flat = value;
                        cx.emit(ProjectFilterEvent::FlatByTime(value));
                        cx.notify();
                    }))
            }));

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
            .child(display)
            .when(!self.flat, |this| this.child(sort))
    }
}
