//! ⌘P: one box to run a command, open a thread or switch project, after
//! Termany's search palette.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, kbd::Kbd,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::threads::relative_time;
use crate::style::size::{ICON, TEXT, TEXT_XS, TITLE};

pub const WIDTH: f32 = 560.;
/// The tallest the palette grows; its top stays put as results change.
pub const MAX_HEIGHT: f32 = 560.;

pub enum Target {
    Action(Box<dyn Action>),
    Thread(u64),
    Project(PathBuf),
}

impl Clone for Target {
    fn clone(&self) -> Self {
        match self {
            Target::Action(action) => Target::Action(action.boxed_clone()),
            Target::Thread(id) => Target::Thread(*id),
            Target::Project(path) => Target::Project(path.clone()),
        }
    }
}

pub struct Item {
    pub icon: IconName,
    pub label: String,
    /// Dim second line: a thread's project, a project's path.
    pub detail: Option<String>,
    /// Right-hand badge: a shortcut for commands, else the kind.
    pub badge: Badge,
    pub target: Target,
    /// Newer first among equal matches.
    pub updated_at: u64,
}

pub enum Badge {
    Keys(Option<Kbd>),
    Kind(&'static str),
}

pub enum SearchPaletteEvent {
    Pick(Target),
}

impl EventEmitter<SearchPaletteEvent> for SearchPalette {}

pub struct SearchPalette {
    input: Entity<InputState>,
    commands: Vec<Item>,
    others: Vec<Item>,
    /// Indices into `commands` then `others` (offset by commands.len()).
    rows: Vec<usize>,
    selected: usize,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

impl SearchPalette {
    pub fn new(commands: Vec<Item>, others: Vec<Item>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Run a command, or search threads and projects…")
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => this.filter(cx),
            InputEvent::PressEnter { .. } => this.confirm(cx),
            _ => {}
        });
        let mut this = Self {
            input,
            commands,
            others,
            rows: Vec::new(),
            selected: 0,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        };
        this.filter(cx);
        this
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

    /// Commands first, then threads and projects, each by how well they match.
    fn filter(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.rows = if query.is_empty() {
            (0..self.commands.len()).collect()
        } else {
            let rank = |items: &[Item], offset: usize| {
                let mut hits: Vec<(i32, u64, usize)> = items
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, item)| {
                        let score = score(&item.label, &query);
                        (score >= 0).then_some((score, item.updated_at, ix + offset))
                    })
                    .collect();
                hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
                hits.into_iter().map(|(_, _, ix)| ix)
            };
            rank(&self.commands, 0)
                .chain(rank(&self.others, self.commands.len()))
                .collect()
        };
        self.selected = 0;
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn select(&mut self, selected: usize, cx: &mut Context<Self>) {
        self.selected = selected;
        // Children of the list: an optional section header, then the rows.
        let header = usize::from(self.query(cx).is_empty());
        self.scroll.scroll_to_item(selected + header);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(&ix) = self.rows.get(self.selected) {
            cx.emit(SearchPaletteEvent::Pick(self.item(ix).target.clone()));
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

        let rows = self.rows.iter().enumerate().map(|(n, &ix)| {
            let item = self.item(ix);
            let active = n == self.selected;
            div()
                .id(("search-row", ix))
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
                .child(Icon::new(item.icon).size(ICON).flex_none().text_color(dim))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(TEXT)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(if item.label.is_empty() { "(untitled)".into() } else { item.label.clone() }),
                        )
                        .when_some(item.detail.clone(), |this, detail| {
                            this.child(
                                div()
                                    .text_size(TEXT_XS)
                                    .text_color(dim)
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(detail),
                            )
                        }),
                )
                .map(|this| match &item.badge {
                    Badge::Keys(Some(kbd)) => this.child(
                        div()
                            .flex_none()
                            .text_size(TEXT_XS)
                            .text_color(if active { theme.foreground } else { dim })
                            .child(kbd.clone().outline()),
                    ),
                    Badge::Keys(None) => this,
                    Badge::Kind(kind) => this.child(
                        div()
                            .flex_none()
                            .text_size(TEXT_XS)
                            .text_color(dim)
                            .child(kind.to_uppercase()),
                    ),
                })
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
                    .text_size(TITLE)
                    .child(crate::icons::search().size(ICON).text_color(dim))
                    .child(Input::new(&self.input).appearance(false)),
            )
            .child(
                div()
                    .id("search-results")
                    .track_scroll(&self.scroll)
                    .max_h(max_list)
                    .overflow_y_scroll()
                    .p(px(6.))
                    .when(query.is_empty(), |this| {
                        this.child(
                            div()
                                .px(px(6.))
                                .pt(px(4.))
                                .pb(px(4.))
                                .text_size(TEXT_XS)
                                .text_color(dim)
                                .child("COMMANDS"),
                        )
                    })
                    .when(!query.is_empty() && self.rows.is_empty(), |this| {
                        this.child(
                            div()
                                .py(px(20.))
                                .text_center()
                                .text_size(TEXT)
                                .text_color(dim)
                                .child(format!("No matches for \"{}\".", self.input.read(cx).value().trim())),
                        )
                    })
                    .children(rows),
            )
    }
}

/// A thread's row: its project and how long ago it moved.
pub fn thread_item(id: u64, title: &str, project: Option<&PathBuf>, updated_at: u64) -> Item {
    let project = project
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let when = relative_time(updated_at);
    Item {
        icon: IconName::MessageSquare,
        label: title.to_string(),
        detail: Some(match project {
            Some(project) => format!("{project} · {when}"),
            None => when,
        }),
        badge: Badge::Kind("Thread"),
        target: Target::Thread(id),
        updated_at,
    }
}

pub fn project_item(path: &PathBuf) -> Item {
    Item {
        icon: IconName::Folder,
        label: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        detail: Some(path.display().to_string()),
        badge: Badge::Kind("Project"),
        target: Target::Project(path.clone()),
        updated_at: 0,
    }
}
