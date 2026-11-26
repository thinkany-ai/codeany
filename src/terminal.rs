//! An embedded terminal: `alacritty_terminal` runs the PTY and parses escape
//! sequences, and this view paints the visible grid in fixed-width cells.

use std::{borrow::Cow, path::PathBuf, sync::Arc};

use alacritty_terminal::{
    event::{Event as TermEvent, EventListener, Notify, OnResize, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        self, Term, TermMode,
        cell::Flags,
        color::Colors,
    },
    tty,
    vte::ansi::{Color as AnsiColor, CursorShape, NamedColor, Rgb},
};
use futures::{StreamExt as _, channel::mpsc};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const PADDING: f32 = 6.;

fn line_height(font_size: Pixels) -> Pixels {
    px((f32::from(font_size) * 1.4).ceil())
}

actions!(terminal, [Copy, Paste, SendTab, SendShiftTab, Clear]);

pub fn init(cx: &mut App) {
    // Copy, paste and clear are bound in `keymap`, so they can be changed.
    cx.bind_keys([
        // The window root binds tab for focus navigation; keep it for the shell.
        KeyBinding::new("tab", SendTab, Some("Terminal")),
        KeyBinding::new("shift-tab", SendShiftTab, Some("Terminal")),
    ]);
}

/// The default shell's name, the tab title until the shell sets one.
fn shell_name() -> String {
    if cfg!(windows) {
        return "PowerShell".into();
    }
    std::env::var("SHELL")
        .ok()
        .and_then(|shell| std::path::Path::new(&shell).file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "sh".into())
}

#[derive(Clone)]
struct Listener(mpsc::UnboundedSender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        self.0.unbounded_send(event).ok();
    }
}

#[derive(Clone, Copy, PartialEq)]
struct TermSize {
    columns: usize,
    lines: usize,
    cell_width: Pixels,
    cell_height: Pixels,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

impl TermSize {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.lines as u16,
            num_cols: self.columns as u16,
            cell_width: f32::from(self.cell_width) as u16,
            cell_height: f32::from(self.cell_height) as u16,
        }
    }
}

struct VisibleCell {
    row: usize,
    column: usize,
    width_in_cells: f32,
    text: SharedString,
    run: TextRun,
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    size: TermSize,
    bounds: Bounds<Pixels>,
    title: SharedString,
    exited: bool,
    selecting: bool,
    focus_handle: FocusHandle,
    _events_task: Task<()>,
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    pub fn new(cwd: Option<PathBuf>, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        let size = TermSize {
            columns: 80,
            lines: 24,
            cell_width: px(8.),
            cell_height: line_height(cx.theme().font_size),
        };
        let (tx, mut rx) = mpsc::unbounded();
        let listener = Listener(tx);

        let config = term::Config {
            scrolling_history: 10_000,
            ..Default::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));

        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "codeany".to_string());
        let options = tty::Options {
            shell: None,
            working_directory: cwd,
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&options, size.window_size(), 0)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();

        Ok(cx.new(|cx| {
            let events_task = cx.spawn(async move |this, cx| {
                while let Some(event) = rx.next().await {
                    // Coalesce bursts of output into a single repaint.
                    let mut events = vec![event];
                    while let Ok(event) = rx.try_recv() {
                        events.push(event);
                    }
                    let result = this.update(cx, |this: &mut Self, cx| {
                        for event in events {
                            this.handle_event(event, cx);
                        }
                        cx.notify();
                    });
                    if result.is_err() {
                        break;
                    }
                }
            });

            Self {
                term,
                notifier,
                size,
                bounds: Bounds::default(),
                title: shell_name().into(),
                exited: false,
                selecting: false,
                focus_handle: cx.focus_handle(),
                _events_task: events_task,
            }
        }))
    }

    fn handle_event(&mut self, event: TermEvent, cx: &mut Context<Self>) {
        match event {
            TermEvent::Title(title) => self.title = title.into(),
            TermEvent::ResetTitle => self.title = shell_name().into(),
            TermEvent::PtyWrite(text) => self.write(text.into_bytes()),
            TermEvent::ClipboardStore(_, text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            TermEvent::ClipboardLoad(_, format) => {
                let text = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text())
                    .unwrap_or_default();
                self.write(format(&text).into_bytes());
            }
            TermEvent::ColorRequest(index, format) => {
                let color = self.term.lock().colors()[index].unwrap_or_else(|| default_rgb(index));
                self.write(format(color).into_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                self.write(format(self.size.window_size()).into_bytes());
            }
            TermEvent::Exit | TermEvent::ChildExit(_) => self.exited = true,
            _ => {}
        }
    }

    fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    fn input(&mut self, bytes: impl Into<Cow<'static, [u8]>>) {
        let mut term = self.term.lock();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
        drop(term);
        self.write(bytes);
    }

    fn set_bounds(&mut self, bounds: Bounds<Pixels>, cell_width: Pixels, cell_height: Pixels, cx: &mut Context<Self>) {
        self.bounds = bounds;
        let columns = ((bounds.size.width / cell_width).floor() as usize).max(2);
        let lines = ((bounds.size.height / cell_height).floor() as usize).max(1);
        let size = TermSize {
            columns,
            lines,
            cell_width,
            cell_height,
        };
        if size != self.size {
            self.size = size;
            self.term.lock().resize(size);
            self.notifier.on_resize(size.window_size());
            cx.notify();
        }
    }

    fn grid_point(&self, position: Point<Pixels>) -> (GridPoint, Side) {
        let local = position - self.bounds.origin;
        let x = f32::from(local.x).max(0.) / f32::from(self.size.cell_width);
        let row = (local.y.max(px(0.)) / self.size.cell_height) as usize;
        let col = (x as usize).min(self.size.columns.saturating_sub(1));
        let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
        let offset = self.term.lock().grid().display_offset() as i32;
        let line = row.min(self.size.lines.saturating_sub(1)) as i32 - offset;
        (GridPoint::new(Line(line), Column(col)), side)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let (point, side) = self.grid_point(event.position);
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.term.lock().selection = Some(Selection::new(kind, point, side));
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (point, side) = self.grid_point(event.position);
        if let Some(selection) = self.term.lock().selection.as_mut() {
            selection.update(point, side);
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = false;
        let mut term = self.term.lock();
        if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            term.selection = None;
        }
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(self.size.cell_height);
        let lines = (delta.y / self.size.cell_height).round() as i32;
        if lines == 0 {
            return;
        }
        let mut term = self.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            // Full-screen programs like `less` scroll with the arrow keys.
            let key: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
            let bytes = key.repeat(lines.unsigned_abs() as usize);
            drop(term);
            self.write(bytes);
        } else {
            term.scroll_display(Scroll::Delta(lines));
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mode = *self.term.lock().mode();
        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, mode) {
            self.input(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.input(text.into_bytes());
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, _: &mut Context<Self>) {
        // Form feed makes the shell redraw its prompt on a cleared screen.
        self.input(b"\x0c".as_slice());
    }

    fn send_tab(&mut self, _: &SendTab, _: &mut Window, _: &mut Context<Self>) {
        self.input(b"\t".as_slice());
    }

    fn send_shift_tab(&mut self, _: &SendShiftTab, _: &mut Window, _: &mut Context<Self>) {
        self.input(b"\x1b[Z".as_slice());
    }

    fn visible_cells(&self, font: &Font, cx: &App) -> (Vec<VisibleCell>, Option<(usize, usize, CursorShape)>) {
        let term = self.term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset as i32;
        let selection = content.selection;
        let theme_fg = cx.theme().foreground;
        let selection_bg = cx.theme().selection;

        let mut cells = Vec::new();
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= self.size.lines {
                continue;
            }
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut fg = resolve_color(cell.fg, colors, true);
            let mut bg = match cell.bg {
                AnsiColor::Named(NamedColor::Background) => None,
                color => Some(resolve_color(color, colors, false)),
            };
            if cell.flags.contains(Flags::INVERSE) {
                let old_bg = bg.unwrap_or_else(|| resolve_color(cell.bg, colors, false));
                bg = Some(fg);
                fg = old_bg;
            }
            if cell.flags.contains(Flags::DIM) {
                fg = fg.opacity(0.66);
            }
            if fg == rgb_to_hsla(default_rgb(NamedColor::Foreground as usize)) {
                fg = theme_fg;
            }
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = Some(selection_bg);
            }

            let mut run_font = font.clone();
            if cell.flags.contains(Flags::BOLD) {
                run_font.weight = FontWeight::BOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                run_font.style = FontStyle::Italic;
            }
            let underline = cell
                .flags
                .intersects(Flags::ALL_UNDERLINES)
                .then(|| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(fg),
                    wavy: cell.flags.contains(Flags::UNDERCURL),
                });

            let ch = if cell.flags.contains(Flags::HIDDEN) || cell.c == '\0' {
                ' '
            } else {
                cell.c
            };
            let strike = cell.flags.contains(Flags::STRIKEOUT);
            if ch == ' ' && bg.is_none() && underline.is_none() && !strike && cell.zerowidth().is_none() {
                continue;
            }
            let mut text = String::new();
            text.push(ch);
            if let Some(zerowidth) = cell.zerowidth() {
                text.extend(zerowidth);
            }
            let run = TextRun {
                len: text.len(),
                font: run_font,
                color: fg,
                background_color: bg,
                underline,
                strikethrough: strike.then(|| {
                    StrikethroughStyle {
                        thickness: px(1.),
                        color: Some(fg),
                    }
                }),
            };
            cells.push(VisibleCell {
                row: row as usize,
                column: indexed.point.column.0,
                width_in_cells: if cell.flags.contains(Flags::WIDE_CHAR) { 2. } else { 1. },
                text: text.into(),
                run,
            });
        }

        let cursor = (content.cursor.shape != CursorShape::Hidden && !self.exited)
            .then(|| {
                let row = content.cursor.point.line.0 + offset;
                (row >= 0 && (row as usize) < self.size.lines).then(|| {
                    (row as usize, content.cursor.point.column.0, content.cursor.shape)
                })
            })
            .flatten();

        (cells, cursor)
    }
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        // A terminal lays text out in a grid, so it needs a monospace font.
        let font = font(theme.mono_font_family.clone());
        let font_size = theme.mono_font_size;
        let cell_height = line_height(font_size);
        let font_id = window.text_system().resolve_font(&font);
        let cell_width = ['M', 'W', 'm', '@'].into_iter()
            .filter_map(|ch| window.text_system().advance(font_id, font_size, ch).ok())
            .fold(font_size * 0.55, |width, advance| width.max(advance.width));

        let (cells, cursor) = self.visible_cells(&font, cx);
        let focused = self.focus_handle.is_focused(window);
        let view = cx.entity();

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .p(px(PADDING))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(Self::send_tab))
            .on_action(cx.listener(Self::send_shift_tab))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .cursor(CursorStyle::IBeam)
            .child(
                div()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .font_family(font.family.clone())
                    .text_size(font_size)
                    .line_height(cell_height)
                    .text_color(theme.foreground)
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                view.update(cx, |this, cx| this.set_bounds(bounds, cell_width, cell_height, cx))
                            },
                            move |bounds, _, window, cx| {
                                for cell in cells {
                                    let origin = bounds.origin + point(
                                        cell_width * cell.column as f32,
                                        cell_height * cell.row as f32,
                                    );
                                    let width = cell_width * cell.width_in_cells;
                                    let cell_bounds = Bounds::new(origin, size(width, cell_height));
                                    if let Some(background) = cell.run.background_color {
                                        window.paint_quad(fill(cell_bounds, background));
                                    }
                                    let line = window.text_system().shape_line(cell.text, font_size, &[cell.run], None);
                                    window.with_content_mask(Some(ContentMask { bounds: cell_bounds }), |window| {
                                        let _ = line.paint(origin, cell_height, TextAlign::Center, Some(width), window, cx);
                                    });
                                }
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .when_some(cursor, |this, (row, col, shape)| {
                        let left = cell_width * col as f32;
                        let top = cell_height * row as f32;
                        let cursor = div().absolute().left(left).top(top);
                        let cursor = match (shape, focused) {
                            (_, false) => cursor
                                .w(cell_width)
                                .h(cell_height)
                                .border_1()
                                .border_color(theme.caret),
                            (CursorShape::Beam, _) => {
                                cursor.w(px(2.)).h(cell_height).bg(theme.caret)
                            }
                            (CursorShape::Underline, _) => cursor
                                .top(top + cell_height - px(2.))
                                .w(cell_width)
                                .h(px(2.))
                                .bg(theme.caret),
                            _ => cursor
                                .w(cell_width)
                                .h(cell_height)
                                .bg(theme.caret.opacity(0.6)),
                        };
                        this.child(cursor)
                    })
                    .when(self.exited, |this| {
                        this.child(
                            div()
                                .absolute()
                                .bottom_0()
                                .right_0()
                                .px_2()
                                .text_color(theme.muted_foreground)
                                .child(crate::i18n::t("[process exited]")),
                        )
                    }),
            )
    }
}

fn keystroke_to_bytes(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let mods = &keystroke.modifiers;
    if mods.platform || mods.function && keystroke.key.len() == 1 {
        // Leave cmd shortcuts to the app.
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let modifier_param = 1
        + (mods.shift as u8)
        + (mods.alt as u8) * 2
        + (mods.control as u8) * 4;

    let csi = |final_byte: char, app: bool| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[1;{modifier_param}{final_byte}").into_bytes()
        } else if app {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |code: u8| -> Vec<u8> {
        if modifier_param > 1 {
            format!("\x1b[{code};{modifier_param}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };

    let bytes = match keystroke.key.as_str() {
        "enter" => {
            if mods.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        "backspace" => {
            if mods.control {
                b"\x08".to_vec()
            } else if mods.alt {
                b"\x1b\x7f".to_vec()
            } else {
                b"\x7f".to_vec()
            }
        }
        "tab" if mods.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "up" => csi('A', app_cursor),
        "down" => csi('B', app_cursor),
        "right" => csi('C', app_cursor),
        "left" => csi('D', app_cursor),
        "home" => csi('H', app_cursor),
        "end" => csi('F', app_cursor),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => csi('P', true),
        "f2" => csi('Q', true),
        "f3" => csi('R', true),
        "f4" => csi('S', true),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if mods.control => vec![0],
        key if mods.control => {
            let ch = key.chars().next()?;
            if key.chars().count() != 1 {
                return None;
            }
            let byte = match ch.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '-' | '7' => 0x1f,
                '8' => 0x7f,
                _ => return None,
            };
            if mods.alt {
                vec![0x1b, byte]
            } else {
                vec![byte]
            }
        }
        key => {
            let text = keystroke
                .key_char
                .clone()
                .or_else(|| (key == "space").then(|| " ".to_string()))
                .or_else(|| (key.chars().count() == 1).then(|| key.to_string()))?;
            if mods.alt {
                // Treat option as meta, like most terminals configured for shells.
                let base = if key == "space" { " " } else { key };
                let mut bytes = vec![0x1b];
                bytes.extend_from_slice(base.as_bytes());
                bytes
            } else {
                text.into_bytes()
            }
        }
    };
    Some(bytes)
}

fn rgb_to_hsla(color: Rgb) -> Hsla {
    Rgba {
        r: color.r as f32 / 255.,
        g: color.g as f32 / 255.,
        b: color.b as f32 / 255.,
        a: 1.,
    }
    .into()
}

fn resolve_color(color: AnsiColor, colors: &Colors, foreground: bool) -> Hsla {
    let index = match color {
        AnsiColor::Spec(rgb) => return rgb_to_hsla(rgb),
        AnsiColor::Indexed(index) => index as usize,
        AnsiColor::Named(named) => {
            let named = match named {
                NamedColor::Foreground if !foreground => NamedColor::Background,
                other => other,
            };
            named as usize
        }
    };
    rgb_to_hsla(colors[index].unwrap_or_else(|| default_rgb(index)))
}

/// A VS Code–like dark palette, followed by the xterm 256-color cube.
fn default_rgb(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x31, 0x31),
        (0x0d, 0xbc, 0x79),
        (0xe5, 0xe5, 0x10),
        (0x24, 0x72, 0xc8),
        (0xbc, 0x3f, 0xbc),
        (0x11, 0xa8, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x66, 0x66, 0x66),
        (0xf1, 0x4c, 0x4c),
        (0x23, 0xd1, 0x8b),
        (0xf5, 0xf5, 0x43),
        (0x3b, 0x8e, 0xea),
        (0xd6, 0x70, 0xd6),
        (0x29, 0xb8, 0xdb),
        (0xff, 0xff, 0xff),
    ];
    let (r, g, b) = match index {
        0..=15 => crate::style::ansi(index).unwrap_or(ANSI[index]),
        16..=231 => {
            let i = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { (v * 40 + 55) as u8 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            (v, v, v)
        }
        i if i == NamedColor::Foreground as usize => (0xcc, 0xcc, 0xcc),
        i if i == NamedColor::Background as usize => (0x1e, 0x1e, 0x1e),
        i if i == NamedColor::Cursor as usize => (0xcc, 0xcc, 0xcc),
        // Dim and bright variants of the named colors.
        i if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&i) => {
            let (r, g, b) = ANSI[i - NamedColor::DimBlack as usize];
            ((r as f32 * 0.66) as u8, (g as f32 * 0.66) as u8, (b as f32 * 0.66) as u8)
        }
        i if i == NamedColor::BrightForeground as usize => (0xff, 0xff, 0xff),
        i if i == NamedColor::DimForeground as usize => (0x88, 0x88, 0x88),
        _ => (0xcc, 0xcc, 0xcc),
    };
    Rgb { r, g, b }
}
