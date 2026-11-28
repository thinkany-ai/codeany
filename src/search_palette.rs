//! Search for commands, threads, and projects.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon,
    input::{Input, InputEvent, InputState},
    kbd::Kbd,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::i18n::{t, tf};
use crate::style::AppButtonStyle as _;
use crate::style::size::{ICON, TEXT};

pub const WIDTH: f32 = 560.;
/// The tallest the palette grows; its top stays put as results change.
pub const MAX_HEIGHT: f32 = 560.;

pub enum Target {
    Action(Box<dyn Action>),
    Thread(u64),
    Project(PathBuf),
    ChooseThreadProject,
    NewThreadInProject(Option<PathBuf>),
    NewProject,
    ChangeTheme,
}

impl Clone for Target {
    fn clone(&self) -> Self {
        match self {
            Target::Action(action) => Target::Action(action.boxed_clone()),
            Target::Thread(id) => Target::Thread(*id),
            Target::Project(path) => Target::Project(path.clone()),
            Target::ChooseThreadProject => Target::ChooseThreadProject,
            Target::NewThreadInProject(path) => Target::NewThreadInProject(path.clone()),
            Target::NewProject => Target::NewProject,
            Target::ChangeTheme => Target::ChangeTheme,
        }
    }
}

pub struct Item {
    pub icon: IconName,
    pub agent: Option<String>,
    pub label: String,
    /// Second-line context for threads, right-aligned context for projects.
    pub detail: Option<String>,
    pub shortcut: Option<Kbd>,
    pub target: Target,
    /// Newer first among equal matches.
    pub updated_at: u64,
}

struct Row {
    item: usize,
    heading: Option<&'static str>,
}

pub enum SearchPaletteEvent {
    Pick(Target),
    Back,
}

impl EventEmitter<SearchPaletteEvent> for SearchPalette {}

pub struct SearchPalette {
    input: Entity<InputState>,
    previous: Option<Entity<Self>>,
    commands: Vec<Item>,
    /// None when choosing a project for a new thread.
    quick_actions: Option<usize>,
    others: Vec<Item>,
    /// Indices into `commands` then `others` (offset by commands.len()).
    rows: Vec<Row>,
    selected: usize,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

impl SearchPalette {
    pub fn new(
        commands: Vec<Item>,
        quick_actions: Option<usize>,
        others: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t(if quick_actions.is_some() {
                "Run a command, or search threads and projects…"
            } else {
                "Search projects…"
            }))
        });
        let subscription =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => this.filter(cx),
                    InputEvent::PressEnter { .. } => this.confirm(cx),
                    _ => {}
                },
            );
        let mut this = Self {
            input,
            previous: None,
            commands,
            quick_actions,
            others,
            rows: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        };
        this.filter(cx);
        this
    }

    pub fn with_previous(mut self, previous: Entity<Self>) -> Self {
        self.previous = Some(previous);
        self
    }

    pub fn previous(&self) -> Option<Entity<Self>> {
        self.previous.clone()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.input.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn item(&self, ix: usize) -> &Item {
        match ix.checked_sub(self.commands.len()) {
            Some(other) => &self.others[other],
            None => &self.commands[ix],
        }
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_lowercase()
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let mut commands = Vec::new();
        let mut threads = Vec::new();
        let mut projects = Vec::new();
        for (ix, item) in self.commands.iter().chain(&self.others).enumerate() {
            let relevance = if query.is_empty() {
                0
            } else {
                score(&item.label, &query)
            };
            if relevance < 0 {
                continue;
            }
            let group = match item.target {
                Target::Thread(_) => &mut threads,
                Target::Project(_) => &mut projects,
                Target::NewThreadInProject(Some(_)) if self.quick_actions.is_none() => {
                    &mut projects
                }
                _ => &mut commands,
            };
            group.push((relevance, item.updated_at, ix));
        }
        for group in [&mut commands, &mut threads, &mut projects] {
            group.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        }
        let groups = if self.quick_actions.is_none() {
            [
                ("Project results", projects),
                ("Quick actions", commands),
                ("Thread results", threads),
            ]
        } else if query.is_empty() {
            commands.truncate(self.quick_actions.unwrap_or_default());
            threads.truncate(5);
            projects.truncate(3);
            [
                ("Quick actions", commands),
                ("Recent threads", threads),
                ("Recent projects", projects),
            ]
        } else {
            [
                ("Thread results", threads),
                ("Project results", projects),
                ("Command results", commands),
            ]
        };
        self.rows = groups
            .into_iter()
            .flat_map(|(heading, group)| {
                group
                    .into_iter()
                    .enumerate()
                    .map(move |(index, (_, _, item))| Row {
                        item,
                        heading: (index == 0).then_some(heading),
                    })
            })
            .collect();
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn select(&mut self, selected: usize, cx: &mut Context<Self>) {
        self.selected = selected;
        let headers = self
            .rows
            .iter()
            .take(selected + 1)
            .filter(|row| row.heading.is_some())
            .count();
        self.scroll.scroll_to_item(selected + headers);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(self.selected) {
            cx.emit(SearchPaletteEvent::Pick(self.item(row.item).target.clone()));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let last = self.rows.len().saturating_sub(1);
        match event.keystroke.key.as_str() {
            "up" => self.select(self.selected.saturating_sub(1), cx),
            "down" => self.select((self.selected + 1).min(last), cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

/// Exact, prefix, then substring; -1 for no match.
fn score(label: &str, query: &str) -> i32 {
    let label = label.to_lowercase();
    if label == query {
        100
    } else if label.starts_with(query) {
        60
    } else if label.contains(query) {
        30
    } else {
        -1
    }
}

impl Render for SearchPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let query = self.query(cx);
        let max_list = (window.viewport_size().height * 0.7).min(px(MAX_HEIGHT)) - px(41.);
        let dim = theme.muted_foreground;

        let rows = self.rows.iter().enumerate().flat_map(|(n, row)| {
            let item = self.item(row.item);
            let active = n == self.selected;
            let title = div().flex_1().min_w_0().text_size(TEXT).truncate().child(
                if item.label.is_empty() {
                    t("(untitled)").into()
                } else {
                    item.label.clone()
                },
            );
            let content = if let Some(agent) = &item.agent {
                let icon_size = crate::style::size::icon_pixels(cx);
                let icon_gap = px(10.);
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(icon_gap)
                            .child(
                                div()
                                    .flex_none()
                                    .child(crate::agent_icons::agent_icon(agent, icon_size, cx)),
                            )
                            .child(title)
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(TEXT)
                                    .text_color(dim)
                                    .child(crate::threads::relative_time(item.updated_at)),
                            ),
                    )
                    .children(item.detail.clone().map(|detail| {
                        div()
                            .pl(icon_size + icon_gap)
                            .text_size(rems(0.875))
                            .text_color(dim)
                            .truncate()
                            .child(detail)
                    }))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(10.))
                    .child(Icon::new(item.icon).size(ICON).flex_none().text_color(dim))
                    .child(title)
                    .when_some(item.detail.clone(), |this, detail| {
                        this.child(
                            div()
                                .max_w(relative(0.38))
                                .min_w_0()
                                .text_size(TEXT)
                                .text_color(dim)
                                .truncate()
                                .child(detail),
                        )
                    })
                    .into_any_element()
            };
            let entry = div()
                .id(("search-row", row.item))
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(active, |this| this.bg(theme.primary.opacity(0.14)))
                .on_mouse_move(cx.listener(move |this, _, _, cx| {
                    if this.selected != n {
                        this.selected = n;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = n;
                    this.confirm(cx);
                }))
                .child(content)
                .when(matches!(item.target, Target::ChooseThreadProject), |this| {
                    this.child(
                        Icon::new(IconName::ChevronRight)
                            .size(ICON)
                            .flex_none()
                            .text_color(dim),
                    )
                })
                .when_some(item.shortcut.clone(), |this, shortcut| {
                    this.child(
                        div()
                            .flex_none()
                            .text_size(TEXT)
                            .text_color(if active { theme.foreground } else { dim })
                            .child(shortcut.outline().text_size(TEXT)),
                    )
                });
            let heading = row.heading.map(|label| {
                div()
                    .px(px(10.))
                    .pt(px(7.))
                    .pb(px(4.))
                    .text_size(TEXT)
                    .text_color(dim)
                    .child(t(label))
                    .into_any_element()
            });
            heading
                .into_iter()
                .chain(std::iter::once(entry.into_any_element()))
        });

        div()
            .key_context("SearchPalette")
            .capture_key_down(cx.listener(Self::on_key_down))
            .w_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(14.))
                    .py(px(10.))
                    .border_b_1()
                    .border_color(theme.border)
                    .text_size(TEXT)
                    .when(self.previous.is_none(), |this| {
                        this.child(crate::icons::search().size(ICON).text_color(dim))
                    })
                    .when(self.previous.is_some(), |this| {
                        this.child(
                            crate::style::chrome_button("search-back")
                                .child(Icon::new(IconName::ArrowLeft).size(ICON).text_color(dim))
                                .accessibility_label(t("Back"))
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(SearchPaletteEvent::Back)),
                                )
                                .app_tooltip(t("Back")),
                        )
                    })
                    .child(Input::new(&self.input).text_size(TEXT).appearance(false)),
            )
            .child(
                div()
                    .id("search-results")
                    .track_scroll(&self.scroll)
                    .max_h(max_list)
                    .overflow_y_scroll()
                    .p(px(6.))
                    .when(!query.is_empty() && self.rows.is_empty(), |this| {
                        this.child(
                            div()
                                .py(px(20.))
                                .text_center()
                                .text_size(TEXT)
                                .text_color(dim)
                                .child(tf(
                                    "No matches for \"{query}\".",
                                    &[("query", self.input.read(cx).value().trim().to_string())],
                                )),
                        )
                    })
                    .children(rows),
            )
    }
}

pub fn thread_item(thread: &crate::threads::Thread, detail: String) -> Item {
    Item {
        icon: IconName::MessageSquare,
        agent: Some(thread.agent.clone()),
        label: thread.title.clone(),
        detail: Some(detail),
        shortcut: None,
        target: Target::Thread(thread.id),
        updated_at: thread.updated_at,
    }
}

pub fn project_item(path: &PathBuf) -> Item {
    Item {
        icon: IconName::Folder,
        agent: None,
        label: crate::threads::project_label(path),
        detail: Some(path.display().to_string()),
        shortcut: None,
        target: Target::Project(path.clone()),
        updated_at: 0,
    }
}
