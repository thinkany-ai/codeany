//! The agent panel's conversation and thread list as views of their own.
//!
//! The workspace embeds them cached: a frame redraws one of them only when
//! the panel changed. Without that, any animation in the window (the
//! spinner on a running tab, a pulsing status dot, "Working…") would
//! rebuild the whole workspace every frame while an agent works.

use gpui_kit::*;

use crate::agent_panel::AgentPanel;

pub struct ChatView {
    panel: Entity<AgentPanel>,
    _observe: [Subscription; 2],
}

impl ChatView {
    pub fn new(panel: Entity<AgentPanel>, cx: &mut Context<Self>) -> Self {
        let _observe = [
            cx.observe(&panel, |_, _, cx| cx.notify()),
            // Text sizes change row heights too.
            cx.observe_global::<crate::style::ThemeEpoch>(|this: &mut Self, cx| {
                this.panel.read(cx).remeasure_chat();
                cx.notify();
            }),
        ];
        Self { panel, _observe }
    }
}

/// Redraw on the panel's changes, and on the theme's (colors, light and
/// dark following the system, text sizes): those reach no entity.
fn observe<V: 'static>(panel: &Entity<AgentPanel>, cx: &mut Context<V>) -> [Subscription; 2] {
    [
        cx.observe(panel, |_, _, cx| cx.notify()),
        cx.observe_global::<crate::style::ThemeEpoch>(|_, cx| cx.notify()),
    ]
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel.update(cx, |panel, cx| panel.render_chat(cx))
    }
}

pub struct SidebarView {
    panel: Entity<AgentPanel>,
    _observe: [Subscription; 2],
}

impl SidebarView {
    pub fn new(panel: Entity<AgentPanel>, cx: &mut Context<Self>) -> Self {
        let _observe = observe(&panel, cx);
        Self { panel, _observe }
    }
}

impl Render for SidebarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel.update(cx, |panel, cx| panel.render_sidebar(cx))
    }
}

/// A view embedded so it redraws only when it changed.
pub fn cached(view: impl Into<AnyView>) -> impl IntoElement {
    view.into().cached(StyleRefinement::default().size_full())
}
