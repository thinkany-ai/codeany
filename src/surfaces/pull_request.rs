//! Pull request: the current branch's PR on GitHub, through the `gh` CLI —
//! state, checks, review, recent comments — and the repo's other open PRs.

use crate::style::AppButtonStyle as _;

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    spinner::Spinner,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::Value;

use super::{Extension, Host, Manifest, Request, Surface, SurfaceContext, SurfaceView, empty_state, shell_path};
use crate::i18n::{t, tf};
use crate::style::size::{ICON, TEXT};

const FIELDS: &str = "number,title,state,isDraft,url,headRefName,baseRefName,author,reviewDecision,\
statusCheckRollup,comments,additions,deletions,changedFiles";

pub struct PullRequest {
    manifest: Manifest,
}

impl Default for PullRequest {
    fn default() -> Self {
        Self {
            manifest: Manifest {
                id: "pull-request",
                name: "Pull request",
                icon: IconName::GitPullRequest,
                key: 'P',
            },
        }
    }
}

impl Extension for PullRequest {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn unavailable(&self, ctx: &SurfaceContext) -> Option<&'static str> {
        ctx.git_root.is_none().then_some("Not a git repository")
    }

    fn build(&self, ctx: &SurfaceContext, host: Host, _window: &mut Window, cx: &mut App) -> Box<dyn Surface> {
        let root = ctx.git_root.clone();
        Box::new(cx.new(|cx| {
            let mut surface = PrSurface {
                root: None,
                host,
                state: State::Loading,
                others: Vec::new(),
                loaded_at: None,
                _load: None,
            };
            surface.load(root, cx);
            surface
        }))
    }
}

enum State {
    Loading,
    /// The branch has no PR.
    None { branch: String },
    Found(Value),
    Failed(String),
}

pub struct PrSurface {
    root: Option<PathBuf>,
    host: Host,
    state: State,
    /// Other open PRs in the repository.
    others: Vec<Value>,
    loaded_at: Option<std::time::Instant>,
    _load: Option<Task<()>>,
}

fn gh(root: &PathBuf, args: &[&str]) -> Result<String, String> {
    let output = crate::command("gh")
        .args(args)
        .current_dir(root)
        .env("PATH", shell_path())
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|_| t("The GitHub CLI (gh) is not installed. Install it with `brew install gh`.").to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The branch's PR and the repo's open PRs. Blocks.
fn fetch(root: &PathBuf) -> (State, Vec<Value>) {
    let branch = crate::command("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(root)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let state = match gh(root, &["pr", "view", "--json", FIELDS]) {
        Ok(json) => serde_json::from_str(&json)
            .map(State::Found)
            .unwrap_or_else(|err| State::Failed(err.to_string())),
        Err(err) if err.contains("no pull requests found") => State::None { branch: branch.clone() },
        Err(err) => State::Failed(err),
    };
    let others = gh(
        root,
        &["pr", "list", "--limit", "20", "--json", "number,title,url,headRefName,isDraft,author"],
    )
    .ok()
    .and_then(|json| serde_json::from_str::<Vec<Value>>(&json).ok())
    .unwrap_or_default()
    .into_iter()
    .filter(|pr| pr["headRefName"].as_str() != Some(branch.as_str()))
    .collect();
    (state, others)
}

impl PrSurface {
    fn load(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.root = root.clone();
        self.loaded_at = Some(std::time::Instant::now());
        self.state = State::Loading;
        let Some(root) = root else {
            self.state = State::Failed(t("Not a git repository").into());
            return;
        };
        self._load = Some(cx.spawn(async move |this, cx| {
            let (state, others) = cx.background_executor().spawn(async move { fetch(&root) }).await;
            this.update(cx, |this, cx| {
                this.state = state;
                this.others = others;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn open(&self, url: &str, window: &mut Window, cx: &mut App) {
        self.host.request(Request::OpenUrl(url.to_string()), window, cx);
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else { return };
        // `--web` hands off to github.com, where the form already knows the branch.
        cx.background_executor()
            .spawn(async move {
                let _ = gh(&root, &["pr", "create", "--web"]);
            })
            .detach();
    }

    fn render_pr(&self, pr: &Value, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let s = |key: &str| pr[key].as_str().unwrap_or_default().to_string();
        let (badge, badge_color) = match (s("state").as_str(), pr["isDraft"].as_bool().unwrap_or(false)) {
            ("MERGED", _) => ("Merged", rgb(0x8957e5).into()),
            ("CLOSED", _) => ("Closed", theme.danger),
            (_, true) => ("Draft", theme.muted_foreground),
            _ => ("Open", theme.success),
        };
        let review = match s("reviewDecision").as_str() {
            "APPROVED" => Some(("Approved", theme.success)),
            "CHANGES_REQUESTED" => Some(("Changes requested", theme.danger)),
            "REVIEW_REQUIRED" => Some(("Review required", theme.warning)),
            _ => None,
        };
        let url = s("url");
        let checks: Vec<Value> = pr["statusCheckRollup"].as_array().cloned().unwrap_or_default();
        let comments: Vec<Value> = pr["comments"].as_array().cloned().unwrap_or_default();

        let check_row = |check: &Value| {
            let name = check["name"]
                .as_str()
                .or(check["context"].as_str())
                .unwrap_or("check")
                .to_string();
            let result = check["conclusion"]
                .as_str()
                .filter(|c| !c.is_empty())
                .or(check["state"].as_str())
                .unwrap_or("PENDING");
            let icon: AnyElement = match result {
                "SUCCESS" => Icon::new(IconName::CircleCheck).size(ICON).text_color(theme.success).into_any_element(),
                "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" => {
                    Icon::new(IconName::CircleX).size(ICON).text_color(theme.danger).into_any_element()
                }
                "SKIPPED" | "NEUTRAL" => Icon::new(IconName::CircleMinus)
                    .size(ICON)
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                _ => Spinner::new().xsmall().into_any_element(),
            };
            div()
                .h(px(28.))
                .flex()
                .items_center()
                .gap_2()
                .text_size(TEXT)
                .child(icon)
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
        };

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px(px(8.))
                                    .py(px(2.))
                                    .rounded_full()
                                    .bg(badge_color.opacity(0.15))
                                    .text_color(badge_color)
                                    .text_size(TEXT)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(state_label(badge)),
                            )
                            .child(
                                div()
                                    .text_size(TEXT)
                                    .text_color(theme.muted_foreground)
                                    .child(format!("#{}", pr["number"])),
                            )
                            .when_some(review, |this, (label, color)| {
                                this.child(div().text_size(TEXT).text_color(color).child(t(label)))
                            }),
                    )
                    .child(div().text_size(TEXT).font_weight(FontWeight::SEMIBOLD).child(s("title")))
                    .child(
                        div()
                            .text_size(TEXT)
                            .text_color(theme.muted_foreground)
                            .child(tf(
                                "{head} → {base} · {author} · +{added} −{removed} in {files} files",
                                &[
                                    ("head", s("headRefName")),
                                    ("base", s("baseRefName")),
                                    ("author", pr["author"]["login"].as_str().unwrap_or("").to_string()),
                                    ("added", pr["additions"].to_string()),
                                    ("removed", pr["deletions"].to_string()),
                                    ("files", pr["changedFiles"].to_string()),
                                ],
                            )),
                    )
                    .child(
                        div().flex().gap_2().child(
                            Button::new("open-pr")
                                .outline()
                                .small()
                                .app_icon(IconName::ExternalLink)
                                .app_label(t("Open on GitHub"))
                                .on_click(cx.listener(move |this, _, window, cx| this.open(&url, window, cx))),
                        ),
                    ),
            )
            .when(!checks.is_empty(), |this| {
                let failed = checks
                    .iter()
                    .filter(|c| matches!(c["conclusion"].as_str(), Some("FAILURE" | "ERROR" | "TIMED_OUT")))
                    .count();
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(section_title(
                            if failed > 0 {
                                tf("Checks · {count} failing", &[("count", failed.to_string())])
                            } else {
                                tf("Checks · {count}", &[("count", checks.len().to_string())])
                            },
                            cx,
                        ))
                        .children(checks.iter().map(check_row)),
                )
            })
            .when(!comments.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(section_title(tf("Comments · {count}", &[("count", comments.len().to_string())]), cx))
                        .children(comments.iter().rev().take(5).enumerate().map(|(ix, c)| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .p_3()
                                .rounded_lg()
                                .bg(theme.secondary)
                                .child(
                                    div()
                                        .text_size(TEXT)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(c["author"]["login"].as_str().unwrap_or("").to_string()),
                                )
                                .child(
                                    div().text_size(TEXT).child(crate::style::markdown(
                                        ("pr-comment", ix),
                                        c["body"].as_str().unwrap_or("").to_string(),
                                        cx,
                                    )),
                                )
                        })),
                )
            })
            .into_any_element()
    }
}

/// A PR state in the current language. "Open" alone is the verb elsewhere
/// in the app, so the state has its own key.
fn state_label(label: &'static str) -> &'static str {
    match (label, crate::i18n::current()) {
        ("Open", crate::i18n::Language::En) => "Open",
        ("Open", _) => t("Open (pull request state)"),
        (other, _) => t(other),
    }
}

fn section_title(text: String, cx: &App) -> impl IntoElement {
    div()
        .mb_1()
        .text_size(TEXT)
        .font_weight(FontWeight::MEDIUM)
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

impl SurfaceView for PrSurface {
    fn set_context(&mut self, ctx: &SurfaceContext, _window: &mut Window, cx: &mut Context<Self>) {
        if ctx.git_root != self.root {
            self.load(ctx.git_root.clone(), cx);
        }
    }

    /// Refreshes come often (every git change, every turn); GitHub need
    /// not be asked more than twice a minute.
    fn refresh(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.loaded_at.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(30)) {
            self.load(self.root.clone(), cx);
        }
    }
}

impl Render for PrSurface {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let body: AnyElement = match &self.state {
            State::Loading => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
            State::Failed(err) => empty_state(err.clone(), cx).into_any_element(),
            State::None { branch } => div()
                .py_10()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .text_size(TEXT)
                .text_color(theme.muted_foreground)
                .child(tf("No pull request for {branch}", &[("branch", branch.clone())]))
                .child(
                    Button::new("create-pr")
                        .primary()
                        .small()
                        .app_label(t("Create pull request"))
                        .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                )
                .into_any_element(),
            State::Found(pr) => {
                let pr = pr.clone();
                self.render_pr(&pr, cx)
            }
        };
        let others = self.others.clone();
        div()
            .id("pull-request")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_6()
            .child(body)
            .when(!others.is_empty(), |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(section_title(tf("Open pull requests · {count}", &[("count", others.len().to_string())]), cx))
                        .children(others.into_iter().enumerate().map(|(ix, pr)| {
                            let url = pr["url"].as_str().unwrap_or("").to_string();
                            div()
                                .id(("other-pr", ix))
                                .h(px(30.))
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|this| this.bg(theme.list_hover))
                                .text_size(TEXT)
                                .child(
                                    Icon::new(IconName::GitPullRequest)
                                        .size(ICON)
                                        .text_color(if pr["isDraft"].as_bool().unwrap_or(false) {
                                            theme.muted_foreground
                                        } else {
                                            theme.success
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("#{}", pr["number"])),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(pr["title"].as_str().unwrap_or("").to_string()),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| this.open(&url, window, cx)))
                        })),
                )
            })
    }
}
