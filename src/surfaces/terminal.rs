//! Terminal: a shell in the project directory.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::*;

use super::{Extension, Host, Manifest, Surface, SurfaceContext, SurfaceView, empty_state};
use crate::i18n::tf;
use crate::terminal::TerminalView;

pub struct TerminalExt {
    manifest: Manifest,
}

impl Default for TerminalExt {
    fn default() -> Self {
        Self {
            manifest: Manifest {
                id: "terminal",
                name: "Terminal",
                icon: IconName::SquareTerminal,
                key: 'T',
            },
        }
    }
}

impl Extension for TerminalExt {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn build(&self, ctx: &SurfaceContext, _host: Host, window: &mut Window, cx: &mut App) -> Box<dyn Surface> {
        Box::new(cx.new(|cx| TerminalSurface::new(ctx.project.clone(), window, cx)))
    }
}

pub struct TerminalSurface {
    cwd: Option<PathBuf>,
    terminal: Result<Entity<TerminalView>, String>,
}

impl TerminalSurface {
    fn new(cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let terminal = TerminalView::new(cwd.clone(), cx).map_err(|err| tf("Failed to start a shell: {err}", &[("err", format!("{err:#}"))]));
        if let Ok(terminal) = &terminal {
            terminal.read(cx).focus_handle(cx).focus(window, cx);
        }
        Self { cwd, terminal }
    }
}

impl SurfaceView for TerminalSurface {
    fn set_context(&mut self, ctx: &SurfaceContext, window: &mut Window, cx: &mut Context<Self>) {
        // A new project gets a fresh shell in its directory.
        if ctx.project != self.cwd {
            *self = Self::new(ctx.project.clone(), window, cx);
            cx.notify();
        }
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let (true, Ok(terminal)) = (active, &self.terminal) {
            terminal.read(cx).focus_handle(cx).focus(window, cx);
        }
    }
}

impl Render for TerminalSurface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.terminal {
            Ok(terminal) => div().size_full().child(terminal.clone()).into_any_element(),
            Err(err) => empty_state(err.clone(), cx).into_any_element(),
        }
    }
}
