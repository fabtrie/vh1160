//! Small widgets in the Manson look. Handlers are plain click callbacks, so a
//! view passes `cx.listener(..)`.

use gpui::{
    AnyElement, App, ClickEvent, Div, ElementId, Entity, FontWeight, Hsla, IntoElement,
    SharedString, Stateful, Window, div, prelude::*, px, svg,
};
use gpui_component::input::{Input, InputState};

use crate::theme::{MONO_FONT, Palette, RADIUS};

pub type OnClick = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

pub fn icon(name: &str, size: f32, color: Hsla) -> impl IntoElement {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// The default button: 34 px, 1 px border, input background.
pub fn button(id: impl Into<ElementId>, p: &Palette) -> Stateful<Div> {
    let hover = p.hover;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .h(px(34.0))
        .px(px(12.0))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(p.bd)
        .bg(p.input)
        .text_color(p.tx)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
}

/// A button with an icon and/or a label, disabled when `enabled` is false.
pub fn action(
    id: impl Into<ElementId>,
    icon_name: Option<&str>,
    text: impl Into<SharedString>,
    enabled: bool,
    p: &Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let text: SharedString = text.into();
    let b = button(id, p)
        .when_some(icon_name, |b, n| b.child(icon(n, 16.0, p.tx)))
        .when(!text.is_empty(), |b| b.child(text));
    enabled_or_dimmed(b, enabled, on_click)
}

pub fn enabled_or_dimmed(
    el: Stateful<Div>,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    if enabled {
        el.on_click(on_click)
    } else {
        el.opacity(0.45).cursor_default()
    }
}

/// The big green/red ECU power button.
pub fn power_button(
    id: impl Into<ElementId>,
    on: bool,
    enabled: bool,
    p: &Palette,
    on_click: OnClick,
) -> Stateful<Div> {
    let (bg, hover) = if on {
        (p.stop, p.stop_hover)
    } else {
        (p.go, p.go_hover)
    };
    let white: Hsla = gpui::white();
    let b = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .h(px(42.0))
        .rounded(px(RADIUS))
        .bg(bg)
        .text_color(white)
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon("power", 16.0, white))
        .child(if on { "ECU off" } else { "ECU on" });
    enabled_or_dimmed(b, enabled, on_click)
}

/// A segmented control: one entry per (label, icon), `selected` index highlighted.
pub fn segmented(
    id: &str,
    items: Vec<(Option<&'static str>, SharedString, OnClick)>,
    selected: usize,
    enabled: bool,
    p: &Palette,
) -> impl IntoElement {
    let hover = p.hover;
    div()
        .flex()
        .flex_none()
        .border_1()
        .border_color(p.bd)
        .rounded(px(RADIUS))
        .overflow_hidden()
        .when(!enabled, |d| d.opacity(0.45))
        .children(
            items
                .into_iter()
                .enumerate()
                .map(|(k, (icon_name, text, on_click))| {
                    let on = k == selected;
                    let b = div()
                        .id(SharedString::from(format!("{id}-{k}")))
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .h(px(32.0))
                        .px(px(10.0))
                        .text_size(px(12.0))
                        .text_color(p.tx)
                        .when(on, |d| d.bg(p.hover).font_weight(FontWeight::SEMIBOLD))
                        .when(!on && enabled, |d| {
                            d.cursor_pointer().hover(move |s| s.bg(hover))
                        })
                        .when_some(icon_name, |d, n| d.child(icon(n, 14.0, p.tx)))
                        .when(!text.is_empty(), |d| d.child(text));
                    if enabled { b.on_click(on_click) } else { b }
                }),
        )
}

/// An on/off switch (36 × 20).
pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    enabled: bool,
    p: &Palette,
    on_click: OnClick,
) -> Stateful<Div> {
    let s = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .when(on, |d| d.justify_end())
        .w(px(36.0))
        .h(px(20.0))
        .p(px(2.0))
        .rounded(px(10.0))
        .bg(if on { p.on } else { p.led_off })
        .cursor_pointer()
        .child(div().size(px(16.0)).rounded_full().bg(gpui::white()));
    enabled_or_dimmed(s, enabled, on_click)
}

/// A checkbox (`round` = radio button) with a label.
pub fn check(
    id: impl Into<ElementId>,
    checked: bool,
    round: bool,
    text: impl Into<SharedString>,
    enabled: bool,
    p: &Palette,
    on_click: OnClick,
) -> Stateful<Div> {
    let mark = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(16.0))
        .border_1()
        .border_color(if checked { p.series_v } else { p.tx3 })
        .bg(if checked && !round {
            p.series_v
        } else {
            p.input
        })
        .map(|d| {
            if round {
                d.rounded_full()
            } else {
                d.rounded(px(4.0))
            }
        })
        .when(checked && round, |d| {
            d.child(div().size(px(8.0)).rounded_full().bg(p.series_v))
        })
        .when(checked && !round, |d| {
            d.child(icon("check", 12.0, gpui::white()))
        });
    let el = div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(13.0))
        .cursor_pointer()
        .child(mark)
        .child(text.into());
    enabled_or_dimmed(el, enabled, on_click)
}

/// Small caption above a field.
pub fn caption(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(p.tx2)
        .mb(px(4.0))
        .child(text.into())
}

pub fn section(text: &'static str, p: &Palette) -> Div {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.tx3)
        .mt(px(4.0))
        .child(text.to_uppercase())
}

pub fn muted(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .text_size(px(12.0))
        .text_color(p.tx3)
        .child(text.into())
}

/// A text input with a unit after it, in the Manson field look.
pub fn field(
    state: &Entity<InputState>,
    unit: &'static str,
    label: &'static str,
    p: &Palette,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            gpui::Styled::h(
                Input::new(state)
                    .aria_label(label)
                    .text_size(px(14.0))
                    .px(px(10.0)),
                px(34.0),
            )
            .flex_1()
            .rounded(px(RADIUS))
            .bg(p.input),
        )
        .when(!unit.is_empty(), |d| d.child(div().flex_none().child(unit)))
}

/// One LCD readout: caption, then the value in the LCD font.
pub fn lcd(title: &'static str, value: String, unit: &'static str, p: &Palette) -> Div {
    div()
        .flex_1()
        .min_w(px(0.0))
        .child(caption(title, p))
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_end()
                .gap(px(6.0))
                .px(px(14.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .bg(p.lcd)
                .text_color(p.seg)
                .font_family(MONO_FONT)
                .overflow_hidden()
                .child(div().text_size(px(44.0)).line_height(px(50.0)).child(value))
                .child(div().text_size(px(18.0)).child(unit)),
        )
}

#[derive(Clone, Copy)]
pub enum Led {
    On,
    Fault,
    Input,
}

/// A bordered indicator with an LED.
pub fn indicator(text: impl Into<SharedString>, active: bool, kind: Led, p: &Palette) -> Div {
    let color = match kind {
        Led::On => p.on,
        Led::Fault => p.fault,
        Led::Input => p.din,
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .px(px(12.0))
        .py(px(4.0))
        .border_1()
        .border_color(p.bd)
        .rounded(px(RADIUS))
        .text_size(px(13.0))
        .text_color(if active { p.tx } else { p.tx3 })
        .child(
            div()
                .size(px(10.0))
                .rounded_full()
                .bg(if active { color } else { p.led_off }),
        )
        .child(text.into())
}

/// A bordered panel with a title row.
pub fn card(
    icon_name: &'static str,
    title: &'static str,
    note: Option<String>,
    p: &Palette,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .flex_1()
        .min_w(px(260.0))
        .px(px(14.0))
        .py(px(12.0))
        .border_1()
        .border_color(p.bd)
        .rounded(px(RADIUS))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon(icon_name, 16.0, p.tx))
                .child(title)
                .child(div().flex_1())
                .when_some(note, |d, n| {
                    d.child(div().font_weight(FontWeight::NORMAL).child(muted(n, p)))
                }),
        )
}

/// A key/value row: label left, content right.
pub fn row(label: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .text_size(px(13.0))
        // min_w(0) lets a long label wrap instead of pushing the control out.
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_color(p.tx2)
                .child(label.into()),
        )
}

pub fn result_text(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(MONO_FONT)
        .text_size(px(17.0))
        .child(text.into())
}

/// A coloured message bar (error or warning).
pub fn banner(bg: Hsla, fg: Hsla) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .mx(px(16.0))
        .mt(px(12.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(RADIUS))
        .bg(bg)
        .text_color(fg)
}

pub fn tab(
    id: &'static str,
    icon_name: &'static str,
    text: &'static str,
    selected: bool,
    badge: Option<&'static str>,
    p: &Palette,
    on_click: OnClick,
) -> AnyElement {
    let hover = p.hover;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(34.0))
        .px(px(14.0))
        .border_b_2()
        .border_color(if selected {
            p.series_v
        } else {
            gpui::transparent_black()
        })
        .text_color(if selected { p.tx } else { p.tx2 })
        .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon(icon_name, 15.0, if selected { p.tx } else { p.tx2 }))
        .child(text)
        .when_some(badge, |d, b| {
            d.child(
                div()
                    .text_size(px(11.0))
                    .px(px(6.0))
                    .rounded(px(6.0))
                    .bg(p.err_bg)
                    .text_color(p.err_tx)
                    .child(b),
            )
        })
        .on_click(on_click)
        .into_any_element()
}
