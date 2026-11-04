//! Choosing an agent and model: a rail of agents (and starred models) beside
//! a searchable model list, like the model menus of chat apps.

use std::collections::HashMap;

use agl::protocol::AgentInfo;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    tooltip::Tooltip,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{
    agent_icons::agent_icon,
    agl_bridge::{self, ModelInfo},
    threads::agent_name,
};
use crate::style::size::{ICON, ICON_LG, TEXT, TEXT_SM};

/// Models shown before "More models"; the rest stay folded.
const MAIN_MODELS: usize = 5;

actions!(model_picker, [Pick1, Pick2, Pick3, Pick4, Pick5, Pick6, Pick7, Pick8, Pick9]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-1", Pick1, Some("ModelPicker")),
        KeyBinding::new("cmd-2", Pick2, Some("ModelPicker")),
        KeyBinding::new("cmd-3", Pick3, Some("ModelPicker")),
        KeyBinding::new("cmd-4", Pick4, Some("ModelPicker")),
        KeyBinding::new("cmd-5", Pick5, Some("ModelPicker")),
        KeyBinding::new("cmd-6", Pick6, Some("ModelPicker")),
        KeyBinding::new("cmd-7", Pick7, Some("ModelPicker")),
        KeyBinding::new("cmd-8", Pick8, Some("ModelPicker")),
        KeyBinding::new("cmd-9", Pick9, Some("ModelPicker")),
    ]);
}

pub enum ModelPickerEvent {
    Select { agent: String, model: Option<String> },
    ToggleFavorite { agent: String, model: String },
}

enum Models {
    Loading,
    Ready(Vec<ModelInfo>),
    Failed(String),
}

/// What the list shows: starred models, or one agent's.
#[derive(Clone, PartialEq)]
enum Tab {
    Favorites,
    Agent(String),
}

#[derive(Clone)]
struct Row {
    agent: String,
    model: ModelInfo,
}

pub struct ModelPicker {
    agents: Vec<AgentInfo>,
    models: HashMap<String, Models>,
    tab: Tab,
    favorites: Vec<(String, String)>,
    current: (String, Option<String>),
    query: Entity<InputState>,
    highlighted: usize,
    show_more: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ModelPickerEvent> for ModelPicker {}

/// A model's name as people know it: "Claude Opus 5.5" rather than "Opus".
pub fn model_display_name(agent: &str, model: &ModelInfo) -> String {
    let name = if model.name.is_empty() { &model.id } else { &model.name };
    // Claude lists `Opus` with "Opus 5.5 · Best for …" as its description.
    let headline = model.description.split(" · ").next().unwrap_or("");
    if agent == "claude" && !name.contains('(') && headline.starts_with(name.as_str()) {
        return format!("Claude {headline}");
    }
    name.trim().trim_end_matches('\u{200b}').to_string()
}

impl ModelPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search models…"));
        let subscriptions = vec![cx.subscribe(&query, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.highlighted = 0;
                // Searching covers every agent, so load them all.
                let ids: Vec<String> = this.agents.iter().map(|a| a.id.clone()).collect();
                for id in ids {
                    this.ensure_models(&id, cx);
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => this.pick(this.highlighted, cx),
            _ => {}
        })];
        Self {
            agents: Vec::new(),
            models: HashMap::new(),
            tab: Tab::Favorites,
            favorites: Vec::new(),
            current: (String::new(), None),
            query,
            highlighted: 0,
            show_more: false,
            _subscriptions: subscriptions,
        }
    }

    /// Installed agents, from agl's detection.
    pub fn set_agents(&mut self, agents: &[AgentInfo], cx: &mut Context<Self>) {
        self.agents = agents.iter().filter(|a| a.available).cloned().collect();
        cx.notify();
    }

    pub fn set_favorites(&mut self, favorites: Vec<(String, String)>, cx: &mut Context<Self>) {
        self.favorites = favorites;
        cx.notify();
    }

    /// Prepare to show, on the current agent's models.
    pub fn open(
        &mut self,
        agent: String,
        model: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab = Tab::Agent(agent.clone());
        self.current = (agent.clone(), model);
        self.highlighted = 0;
        self.show_more = false;
        self.query.update(cx, |query, cx| {
            query.set_value("", window, cx);
            query.focus(window, cx);
        });
        self.ensure_models(&agent, cx);
        cx.notify();
    }

    /// The agent's models, once loaded.
    pub fn models(&self, agent: &str) -> Option<&[ModelInfo]> {
        match self.models.get(agent) {
            Some(Models::Ready(models)) => Some(models),
            _ => None,
        }
    }

    pub fn ensure_models(&mut self, agent: &str, cx: &mut Context<Self>) {
        if self.models.contains_key(agent) {
            return;
        }
        self.models.insert(agent.to_string(), Models::Loading);
        let id = agent.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let id = id.clone();
                    async move {
                        agl_bridge::get()
                            .map_err(anyhow::Error::msg)
                            .and_then(|agl| agl.list_models(&id))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                let models = match result {
                    Ok(models) => Models::Ready(models),
                    Err(err) => Models::Failed(format!("{err:#}")),
                };
                this.models.insert(id, models);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn agent_label(&self, id: &str) -> String {
        self.agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| agent_name(id))
    }

    fn is_favorite(&self, agent: &str, model: &str) -> bool {
        self.favorites.iter().any(|(a, m)| a == agent && m == model)
    }

    /// The rows on screen, in order, and how many models are folded away.
    fn rows(&self, cx: &App) -> (Vec<Row>, usize) {
        let query = self.query.read(cx).value().trim().to_lowercase();
        if !query.is_empty() {
            let rows = self
                .agents
                .iter()
                .filter_map(|agent| Some((agent, self.models(&agent.id)?)))
                .flat_map(|(agent, models)| {
                    models.iter().map(|model| Row {
                        agent: agent.id.clone(),
                        model: model.clone(),
                    })
                })
                .filter(|row| {
                    let name = model_display_name(&row.agent, &row.model).to_lowercase();
                    name.contains(&query)
                        || row.model.id.to_lowercase().contains(&query)
                        || self.agent_label(&row.agent).to_lowercase().contains(&query)
                })
                .collect();
            return (rows, 0);
        }
        match &self.tab {
            Tab::Favorites => {
                let rows = self
                    .favorites
                    .iter()
                    .map(|(agent, id)| Row {
                        agent: agent.clone(),
                        model: self
                            .models(agent)
                            .and_then(|models| models.iter().find(|m| &m.id == id).cloned())
                            .unwrap_or_else(|| ModelInfo {
                                id: id.clone(),
                                name: id.clone(),
                                description: String::new(),
                                efforts: Vec::new(),
                                default: false,
                            }),
                    })
                    .collect();
                (rows, 0)
            }
            Tab::Agent(agent) => {
                let models = self.models(agent).unwrap_or_default();
                let mut rows: Vec<Row> = models
                    .iter()
                    .map(|model| Row {
                        agent: agent.clone(),
                        model: model.clone(),
                    })
                    .collect();
                let folded = rows.len().saturating_sub(MAIN_MODELS);
                if !self.show_more && folded > 0 {
                    rows.truncate(MAIN_MODELS);
                    return (rows, folded);
                }
                (rows, 0)
            }
        }
    }

    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let (rows, _) = self.rows(cx);
        let Some(row) = rows.get(ix) else { return };
        cx.emit(ModelPickerEvent::Select {
            agent: row.agent.clone(),
            model: Some(row.model.id.clone()),
        });
    }

    fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if let Tab::Agent(agent) = &tab {
            self.ensure_models(agent, cx);
        }
        // Favorites may come from any agent; load their names.
        if tab == Tab::Favorites {
            let agents: Vec<String> = self.favorites.iter().map(|(a, _)| a.clone()).collect();
            for agent in agents {
                self.ensure_models(&agent, cx);
            }
        }
        self.tab = tab;
        self.highlighted = 0;
        self.show_more = false;
        cx.notify();
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let rail_button = |id: SharedString, selected: bool, child: AnyElement| {
            div()
                .id(id)
                .relative()
                .size(px(40.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_lg()
                .cursor_pointer()
                .when(selected, |this| this.bg(theme.list_active))
                .when(!selected, |this| this.hover(|this| this.bg(theme.list_hover)))
                .child(child)
                .when(selected, |this| {
                    this.child(
                        div()
                            .absolute()
                            .right(px(-8.))
                            .top(px(10.))
                            .w(px(3.))
                            .h(px(20.))
                            .rounded_full()
                            .bg(theme.primary),
                    )
                })
        };

        let mut rail = div()
            .id("model-picker-rail")
            .w(px(56.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .py_2()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .border_r_1()
            .border_color(theme.border)
            .child(
                rail_button(
                    "rail-favorites".into(),
                    self.tab == Tab::Favorites,
                    Icon::new(IconName::Star)
                        .size(ICON_LG)
                        .text_color(theme.foreground)
                        .into_any_element(),
                )
                .on_click(cx.listener(|this, _, _, cx| this.select_tab(Tab::Favorites, cx))),
            )
            .child(div().my_1().w(px(32.)).h(px(1.)).bg(theme.border));
        for agent in &self.agents {
            let id = agent.id.clone();
            rail = rail.child(
                rail_button(
                    format!("rail-{id}").into(),
                    self.tab == Tab::Agent(id.clone()),
                    agent_icon(&id, px(24.), cx),
                )
                .tooltip({
                    let name = agent.name.clone();
                    move |window, cx| Tooltip::new(name.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_tab(Tab::Agent(id.clone()), cx)
                })),
            );
        }
        rail
    }

    fn render_row(&self, ix: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let favorite = self.is_favorite(&row.agent, &row.model.id);
        let current = self.current.0 == row.agent
            && match &self.current.1 {
                Some(model) => *model == row.model.id,
                None => row.model.default || row.model.id == "default",
            };
        let (agent, model) = (row.agent.clone(), row.model.id.clone());
        div()
            .id(("model-row", ix))
            .group("model-row")
            .px_3()
            .py(px(7.))
            .flex()
            .items_center()
            .gap_3()
            .rounded_lg()
            .cursor_pointer()
            .when(ix == self.highlighted, |this| this.bg(theme.list_active))
            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                if this.highlighted != ix {
                    this.highlighted = ix;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(TEXT)
                            .font_weight(FontWeight::MEDIUM)
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(model_display_name(&row.agent, &row.model)),
                            )
                            .when(current, |this| {
                                this.child(Icon::new(IconName::Check).size_3p5().text_color(theme.primary))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(TEXT_SM)
                            .text_color(theme.muted_foreground)
                            .child(agent_icon(&row.agent, px(14.), cx))
                            .child(self.agent_label(&row.agent)),
                    ),
            )
            .when(ix < 9, |this| {
                this.child(
                    div()
                        .flex_none()
                        .px(px(7.))
                        .py(px(2.))
                        .rounded_md()
                        .bg(theme.secondary)
                        .text_size(TEXT_SM)
                        .text_color(theme.muted_foreground)
                        .child(format!("⌘{}", ix + 1)),
                )
            })
            .child(
                div()
                    .id(("favorite", ix))
                    .flex_none()
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .hover(|this| this.bg(theme.list_hover))
                    .child(
                        Icon::new(IconName::Star)
                            .size(ICON)
                            .text_color(if favorite { theme.warning } else { theme.muted_foreground }),
                    )
                    .on_click(cx.listener({
                        let (agent, model) = (agent.clone(), model.clone());
                        move |_, _, _, cx| {
                            cx.stop_propagation();
                            cx.emit(ModelPickerEvent::ToggleFavorite {
                                agent: agent.clone(),
                                model: model.clone(),
                            });
                        }
                    })),
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(ModelPickerEvent::Select {
                    agent: agent.clone(),
                    model: Some(model.clone()),
                })
            }))
            .into_any_element()
    }
}

impl Render for ModelPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let (rows, folded) = self.rows(cx);
        let searching = !self.query.read(cx).value().trim().is_empty();
        let loading = match &self.tab {
            Tab::Agent(agent) => match self.models.get(agent) {
                Some(Models::Loading) | None => Some(None),
                Some(Models::Failed(err)) => Some(Some(err.clone())),
                Some(Models::Ready(_)) => None,
            },
            Tab::Favorites => None,
        };

        let empty = |text: String| {
            div()
                .py_8()
                .flex()
                .justify_center()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(text)
        };

        let mut list = div()
            .id("model-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .flex()
            .flex_col()
            .gap(px(2.));
        if rows.is_empty() {
            list = list.child(match (&loading, searching, &self.tab) {
                (Some(None), false, _) => div()
                    .py_8()
                    .flex()
                    .justify_center()
                    .child(Spinner::new().small())
                    .into_any_element(),
                (Some(Some(err)), false, _) => empty(err.clone()).into_any_element(),
                (_, true, _) => empty("No matching models".into()).into_any_element(),
                (_, false, Tab::Favorites) => {
                    empty("Star a model to keep it here".into()).into_any_element()
                }
                _ => empty("This agent lists no models".into()).into_any_element(),
            });
        }
        for (ix, row) in rows.iter().enumerate() {
            list = list.child(self.render_row(ix, row, cx));
        }
        if folded > 0 {
            list = list.child(
                div()
                    .id("more-models")
                    .px_3()
                    .py(px(7.))
                    .flex()
                    .items_center()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(|this| this.bg(theme.list_hover))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(div().text_size(TEXT).font_weight(FontWeight::MEDIUM).child("More models"))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{folded} models")),
                            ),
                    )
                    .child(Icon::new(IconName::ChevronRight).size(ICON).text_color(theme.muted_foreground))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_more = true;
                        cx.notify();
                    })),
            );
        }

        div()
            .key_context("ModelPicker")
            .on_action(cx.listener(|this, _: &Pick1, _, cx| this.pick(0, cx)))
            .on_action(cx.listener(|this, _: &Pick2, _, cx| this.pick(1, cx)))
            .on_action(cx.listener(|this, _: &Pick3, _, cx| this.pick(2, cx)))
            .on_action(cx.listener(|this, _: &Pick4, _, cx| this.pick(3, cx)))
            .on_action(cx.listener(|this, _: &Pick5, _, cx| this.pick(4, cx)))
            .on_action(cx.listener(|this, _: &Pick6, _, cx| this.pick(5, cx)))
            .on_action(cx.listener(|this, _: &Pick7, _, cx| this.pick(6, cx)))
            .on_action(cx.listener(|this, _: &Pick8, _, cx| this.pick(7, cx)))
            .on_action(cx.listener(|this, _: &Pick9, _, cx| this.pick(8, cx)))
            .w(px(520.))
            .h(px(400.))
            .flex()
            .child(self.render_rail(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mx_3()
                            .mt_2()
                            .mb_2()
                            .border_b_1()
                            .border_color(theme.primary.opacity(0.6))
                            .text_size(TEXT)
                            .child(
                                Input::new(&self.query)
                                    .appearance(false)
                                    .prefix(
                                        Icon::new(IconName::Search)
                                            .size(ICON)
                                            .text_color(theme.muted_foreground),
                                    ),
                            ),
                    )
                    .child(list),
            )
    }
}
