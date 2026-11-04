//! Files: the project's file tree; opening a file shows it in the editor.

use gpui_kit::assets::IconName;
use gpui_kit::*;

use super::{Extension, Host, Manifest, Request, Surface, SurfaceContext, SurfaceView};
use crate::{
    file_tree::{FileTree, FileTreeEvent},
    git,
};

pub struct Files {
    manifest: Manifest,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            manifest: Manifest {
                id: "files",
                name: "Files",
                icon: IconName::Files,
                key: 'F',
            },
        }
    }
}

impl Extension for Files {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn unavailable(&self, ctx: &SurfaceContext) -> Option<&'static str> {
        ctx.project.is_none().then_some("Open a project first")
    }

    fn build(&self, ctx: &SurfaceContext, host: Host, window: &mut Window, cx: &mut App) -> Box<dyn Surface> {
        let ctx = ctx.clone();
        Box::new(cx.new(|cx| {
            let tree = cx.new(FileTree::new);
            let subscription = cx.subscribe_in(&tree, window, move |_, _, event: &FileTreeEvent, window, cx| {
                let FileTreeEvent::OpenFile(path) = event;
                host.request(Request::OpenFile(path.clone()), window, cx);
            });
            let mut surface = FilesSurface {
                tree,
                ctx: SurfaceContext::default(),
                _subscription: subscription,
            };
            surface.set_context(&ctx, window, cx);
            surface
        }))
    }
}

pub struct FilesSurface {
    tree: Entity<FileTree>,
    ctx: SurfaceContext,
    _subscription: Subscription,
}

impl FilesSurface {
    /// Color files by their git status.
    fn load_status(&self, cx: &mut Context<Self>) {
        let (Some(project), tree) = (self.ctx.project.clone(), self.tree.clone()) else { return };
        cx.spawn(async move |_, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move { git::snapshot(&project) })
                .await
                .unwrap_or_default();
            tree.update(cx, |tree, cx| tree.set_git(snapshot, cx));
        })
        .detach();
    }
}

impl SurfaceView for FilesSurface {
    fn set_context(&mut self, ctx: &SurfaceContext, _window: &mut Window, cx: &mut Context<Self>) {
        if self.ctx.project != ctx.project {
            let root = ctx.project.clone();
            self.tree.update(cx, |tree, cx| tree.set_root(root, cx));
        }
        self.ctx = ctx.clone();
        self.load_status(cx);
    }

    fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.tree.update(cx, |tree, cx| tree.refresh(cx));
        self.load_status(cx);
    }
}

impl Render for FilesSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.tree.clone())
    }
}
