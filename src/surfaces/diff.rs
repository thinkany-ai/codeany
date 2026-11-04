//! Diff: the working tree's changes against HEAD.

use gpui_kit::assets::IconName;
use gpui_kit::*;

use super::{Extension, Host, Manifest, Surface, SurfaceContext, SurfaceView};
use crate::git_panel::GitPanel;

pub struct Diff {
    manifest: Manifest,
}

impl Default for Diff {
    fn default() -> Self {
        Self {
            manifest: Manifest {
                id: "diff",
                name: "Diff",
                icon: IconName::FileDiff,
                key: 'D',
            },
        }
    }
}

impl Extension for Diff {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn unavailable(&self, ctx: &SurfaceContext) -> Option<&'static str> {
        ctx.git_root.is_none().then_some("Not a git repository")
    }

    fn build(&self, ctx: &SurfaceContext, _host: Host, _window: &mut Window, cx: &mut App) -> Box<dyn Surface> {
        let project = ctx.project.clone();
        Box::new(cx.new(|cx| {
            let panel = cx.new(GitPanel::new);
            panel.update(cx, |panel, cx| panel.set_root(project.clone(), cx));
            DiffSurface { panel, project }
        }))
    }
}

pub struct DiffSurface {
    panel: Entity<GitPanel>,
    project: Option<std::path::PathBuf>,
}

impl SurfaceView for DiffSurface {
    fn set_context(&mut self, ctx: &SurfaceContext, _window: &mut Window, cx: &mut Context<Self>) {
        if self.project != ctx.project {
            self.project = ctx.project.clone();
            let root = ctx.project.clone();
            self.panel.update(cx, |panel, cx| panel.set_root(root, cx));
        }
    }

    fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.panel.update(cx, |panel, cx| panel.refresh(cx));
    }
}

impl Render for DiffSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.panel.clone())
    }
}
