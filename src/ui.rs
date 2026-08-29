use std::cell::{RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use gtk::cairo::{Context, FontSlant, FontWeight};
use gtk::gdk;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::core::{Color, KeyboardGeometry, KeyboardLayout, Modifiers, Palette, Rect, actions_for};
use crate::system::{RuntimeSnapshot, load_runtime};

#[derive(Clone, Debug)]
struct HitRegion {
    rect: Rect,
    key: String,
    legend: String,
}

#[derive(Clone, Debug, Default)]
struct RenderMetrics {
    panel: Rect,
    hits: Vec<HitRegion>,
}

struct OverlayState {
    snapshot: RuntimeSnapshot,
    geometry: KeyboardGeometry,
    modifiers: Modifiers,
    hovered: Option<String>,
    metrics: RenderMetrics,
}

#[derive(Clone, Copy)]
struct ObservedModifierMasks {
    event: gdk::ModifierType,
    device: Option<gdk::ModifierType>,
}

impl ObservedModifierMasks {
    fn capture(controller: &gtk::EventControllerKey, event: gdk::ModifierType) -> Self {
        Self {
            event,
            device: controller
                .current_event_device()
                .map(|device| device.modifier_state()),
        }
    }
}

pub fn run(layout_override: Option<KeyboardLayout>) -> u8 {
    let application = gtk::Application::builder()
        .application_id("dev.keywrld.Keywrld")
        .build();
    let current_window = Rc::new(RefCell::new(None::<gtk::ApplicationWindow>));

    application.connect_activate({
        let current_window = current_window.clone();
        move |application| {
            let existing = current_window.borrow_mut().take();
            if let Some(window) = existing {
                window.close();
                return;
            }

            let snapshot = load_runtime(layout_override);
            let window = build_window(application, snapshot);
            window.connect_close_request({
                let current_window = current_window.clone();
                move |_| {
                    current_window.borrow_mut().take();
                    gtk::glib::Propagation::Proceed
                }
            });
            current_window.borrow_mut().replace(window.clone());
            window.present();
        }
    });

    application.run_with_args(&["keywrld"]).get()
}

fn build_window(
    application: &gtk::Application,
    snapshot: RuntimeSnapshot,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Keywrld")
        .decorated(false)
        .build();
    window.add_css_class("keywrld-overlay");
    window.init_layer_shell();
    window.set_namespace(Some("keywrld"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_exclusive_zone(0);
    for edge in [
        gtk4_layer_shell::Edge::Top,
        gtk4_layer_shell::Edge::Right,
        gtk4_layer_shell::Edge::Bottom,
        gtk4_layer_shell::Edge::Left,
    ] {
        window.set_anchor(edge, true);
    }
    select_monitor(&window, snapshot.monitor_connector.as_deref());

    let provider = gtk::CssProvider::new();
    provider.load_from_data("window.keywrld-overlay { background: transparent; }");
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let state = Rc::new(RefCell::new(OverlayState {
        geometry: KeyboardGeometry::new(snapshot.layout),
        snapshot,
        modifiers: Modifiers::super_only(),
        hovered: None,
        metrics: RenderMetrics::default(),
    }));

    let drawing_area = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .focusable(true)
        .build();
    drawing_area.set_draw_func({
        let state = state.clone();
        move |_, context, width, height| {
            draw_overlay(
                context,
                width as f64,
                height as f64,
                &mut state.borrow_mut(),
            );
        }
    });

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |_, x, y| {
            let hovered = state
                .borrow()
                .metrics
                .hits
                .iter()
                .find(|hit| hit.rect.contains(x, y))
                .map(|hit| hit.key.clone());
            let mut state = state.borrow_mut();
            if state.hovered != hovered {
                state.hovered = hovered;
                drawing_area.queue_draw();
            }
        }
    });
    motion.connect_leave({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |_| {
            if state.borrow().hovered.is_some() {
                state.borrow_mut().hovered = None;
                drawing_area.queue_draw();
            }
        }
    });
    drawing_area.add_controller(motion);

    let click = gtk::GestureClick::new();
    click.connect_released({
        let state = state.clone();
        let window = window.downgrade();
        move |_, _, x, y| {
            if !state.borrow().metrics.panel.contains(x, y)
                && let Some(window) = window.upgrade()
            {
                window.close();
            }
        }
    });
    drawing_area.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        let window = window.downgrade();
        move |_, key, _, _| {
            let before = state.borrow().modifiers;
            let candidate = modifiers_for_key_event(before, key, true);
            if key == gdk::Key::Escape {
                if let Some(window) = window.upgrade() {
                    window.close();
                }
                return gtk::glib::Propagation::Stop;
            }
            if let Some(modifiers) = candidate {
                update_modifiers(&state, &drawing_area, modifiers);
            }
            gtk::glib::Propagation::Proceed
        }
    });
    keys.connect_key_released({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |_, key, _, _| {
            let before = state.borrow().modifiers;
            let candidate = modifiers_for_key_event(before, key, false);
            if let Some(modifiers) = candidate {
                update_modifiers(&state, &drawing_area, modifiers);
            }
        }
    });
    keys.connect_modifiers({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |controller, modifier_state| {
            let masks = ObservedModifierMasks::capture(controller, modifier_state);
            let modifiers = modifiers_from_state_change(masks.event, masks.device);
            update_modifiers(&state, &drawing_area, modifiers);
            gtk::glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    window.set_child(Some(&drawing_area));
    gtk::prelude::GtkWindowExt::set_focus(&window, Some(&drawing_area));
    window
}

fn select_monitor(window: &gtk::ApplicationWindow, connector: Option<&str>) {
    let (Some(connector), Some(display)) = (connector, gdk::Display::default()) else {
        return;
    };
    let monitors = display.monitors();
    for index in 0..monitors.n_items() {
        let Some(object) = monitors.item(index) else {
            continue;
        };
        let Ok(monitor) = object.downcast::<gdk::Monitor>() else {
            continue;
        };
        if monitor.connector().as_deref() == Some(connector) {
            window.set_monitor(Some(&monitor));
            return;
        }
    }
}

fn modifier_for_key(key: gdk::Key) -> Option<Modifiers> {
    match key {
        gdk::Key::Shift_L | gdk::Key::Shift_R => Some(Modifiers::SHIFT),
        gdk::Key::Control_L | gdk::Key::Control_R => Some(Modifiers::CTRL),
        gdk::Key::Alt_L | gdk::Key::Alt_R => Some(Modifiers::ALT),
        _ => None,
    }
}

fn modifiers_from_gdk(state: gdk::ModifierType) -> Modifiers {
    let mut modifiers = Modifiers::super_only();
    if state.contains(gdk::ModifierType::SHIFT_MASK) {
        modifiers = modifiers.with(Modifiers::SHIFT);
    }
    if state.contains(gdk::ModifierType::CONTROL_MASK) {
        modifiers = modifiers.with(Modifiers::CTRL);
    }
    if state.contains(gdk::ModifierType::ALT_MASK) {
        modifiers = modifiers.with(Modifiers::ALT);
    }
    modifiers
}

fn modifiers_from_state_change(
    signal_state: gdk::ModifierType,
    device_state: Option<gdk::ModifierType>,
) -> Modifiers {
    modifiers_from_gdk(device_state.unwrap_or(signal_state))
}

fn modifiers_for_key_event(current: Modifiers, key: gdk::Key, pressed: bool) -> Option<Modifiers> {
    match (modifier_for_key(key), pressed) {
        (Some(modifier), true) => Some(current.with(modifier)),
        (Some(modifier), false) => Some(current.without(modifier)),
        (None, _) => None,
    }
}

fn update_modifiers(
    state: &Rc<RefCell<OverlayState>>,
    drawing_area: &gtk::DrawingArea,
    modifiers: Modifiers,
) {
    let mut state = state.borrow_mut();
    if state.modifiers != modifiers {
        state.modifiers = modifiers;
        state.hovered = None;
        drawing_area.queue_draw();
    }
}

fn draw_overlay(context: &Context, width: f64, height: f64, state: &mut OverlayState) {
    let palette = state.snapshot.palette;
    set_source(context, palette.darker_background.with_alpha(0.82));
    let _ = context.paint();

    let actions = actions_for(&state.snapshot.bindings, state.modifiers);
    let geometry_ids = state.geometry.key_ids();
    let overflow: Vec<_> = actions
        .iter()
        .filter(|(key, _)| !geometry_ids.contains(key.as_str()))
        .collect();

    let outer_padding = 24.0;
    let header_height = 82.0;
    let detail_height = 66.0;
    let chip_height = 46.0;
    let chip_gap = 8.0;
    let chip_columns = usize::min(4, usize::max(1, overflow.len()));
    let chip_rows = overflow.len().div_ceil(chip_columns);
    let overflow_height = if chip_rows == 0 {
        0.0
    } else {
        chip_rows as f64 * chip_height + 12.0
    };

    let available_width = (width * 0.92 - outer_padding * 2.0).max(320.0);
    let available_height =
        (height * 0.9 - header_height - detail_height - overflow_height - outer_padding * 2.0)
            .max(180.0);
    let unit = (available_width / state.geometry.width)
        .min(available_height / state.geometry.height)
        .min(78.0);
    let board_width = state.geometry.width * unit;
    let board_height = state.geometry.height * unit;
    let panel_width = board_width + outer_padding * 2.0;
    let panel_height =
        header_height + board_height + overflow_height + detail_height + outer_padding * 2.0;
    let panel = Rect {
        x: (width - panel_width) / 2.0,
        y: (height - panel_height) / 2.0,
        width: panel_width,
        height: panel_height,
    };
    state.metrics.panel = panel;
    state.metrics.hits.clear();

    rounded_rectangle(context, panel, 18.0);
    set_source(context, palette.background.with_alpha(0.985));
    let _ = context.fill_preserve();
    context.set_line_width(1.0);
    set_source(context, palette.muted.with_alpha(0.9));
    let _ = context.stroke();

    let content_x = panel.x + outer_padding;
    let board_y = panel.y + outer_padding + header_height;
    draw_header(
        context,
        state,
        content_x,
        panel.y + outer_padding,
        board_width,
        palette,
    );

    let key_gap = (unit * 0.075).clamp(3.0, 7.0);
    for key in &state.geometry.keys {
        let rect = Rect {
            x: content_x + key.rect.x * unit + key_gap / 2.0,
            y: board_y + key.rect.y * unit + key_gap / 2.0,
            width: key.rect.width * unit - key_gap,
            height: key.rect.height * unit - key_gap,
        };
        let key_actions = actions.get(&key.id);
        let hovered = state.hovered.as_deref() == Some(key.id.as_str());
        let modifier_active = modifier_key_active(&key.id, state.modifiers);
        draw_key(
            context,
            rect,
            &key.legend,
            key_actions,
            hovered,
            modifier_active,
            palette,
        );
        state.metrics.hits.push(HitRegion {
            rect,
            key: key.id.clone(),
            legend: key.legend.clone(),
        });
    }

    let overflow_y = board_y + board_height + 12.0;
    if !overflow.is_empty() {
        let chip_width = (board_width - chip_gap * (chip_columns.saturating_sub(1)) as f64)
            / chip_columns as f64;
        for (index, (key, key_actions)) in overflow.iter().enumerate() {
            let column = index % chip_columns;
            let row = index / chip_columns;
            let rect = Rect {
                x: content_x + column as f64 * (chip_width + chip_gap),
                y: overflow_y + row as f64 * chip_height,
                width: chip_width,
                height: chip_height - chip_gap,
            };
            let hovered = state.hovered.as_deref() == Some(key.as_str());
            draw_overflow_chip(context, rect, key, key_actions, hovered, palette);
            state.metrics.hits.push(HitRegion {
                rect,
                key: (*key).clone(),
                legend: friendly_key_name(key),
            });
        }
    }

    let detail_y = board_y + board_height + overflow_height + 10.0;
    draw_details(
        context,
        state,
        &actions,
        Rect {
            x: content_x,
            y: detail_y,
            width: board_width,
            height: detail_height - 10.0,
        },
        palette,
    );
}

fn draw_header(
    context: &Context,
    state: &OverlayState,
    x: f64,
    y: f64,
    width: f64,
    palette: Palette,
) {
    draw_text(
        context,
        x,
        y + 24.0,
        &state.modifiers.label(),
        22.0,
        FontWeight::Bold,
        palette.foreground,
    );
    draw_text_right(
        context,
        x + width,
        y + 22.0,
        &state.snapshot.device_name,
        13.0,
        FontWeight::Normal,
        palette.dark_foreground,
    );

    let message = state
        .snapshot
        .binding_error
        .as_deref()
        .unwrap_or("Hold Shift, Ctrl, or Alt to inspect an exact chord · Hover for details");
    draw_text(
        context,
        x,
        y + 53.0,
        &elide(message, 112),
        13.0,
        FontWeight::Normal,
        if state.snapshot.binding_error.is_some() {
            palette.red
        } else {
            palette.dark_foreground
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_key(
    context: &Context,
    rect: Rect,
    legend: &str,
    actions: Option<&Vec<String>>,
    hovered: bool,
    modifier_active: bool,
    palette: Palette,
) {
    rounded_rectangle(context, rect, (rect.height * 0.1).clamp(4.0, 8.0));
    let fill = if hovered {
        palette.accent.with_alpha(0.3)
    } else if actions.is_some() {
        palette.selection
    } else {
        palette.lighter_background.with_alpha(0.34)
    };
    set_source(context, fill);
    let _ = context.fill_preserve();
    context.set_line_width(if hovered || modifier_active { 2.0 } else { 1.0 });
    set_source(
        context,
        if hovered || modifier_active {
            palette.accent
        } else if actions.is_some() {
            palette.muted
        } else {
            palette.muted.with_alpha(0.42)
        },
    );
    let _ = context.stroke();

    let legend_color = if actions.is_some() || modifier_active {
        palette.foreground
    } else {
        palette.dark_foreground.with_alpha(0.66)
    };
    draw_text(
        context,
        rect.x + 7.0,
        rect.y + 14.0,
        legend,
        (rect.height * 0.16).clamp(8.0, 11.0),
        FontWeight::Bold,
        legend_color,
    );

    if let Some(actions) = actions {
        let summary = actions.join(" · ");
        let max_chars = ((rect.width - 10.0) / 6.1).floor().max(4.0) as usize;
        let lines = wrap_two_lines(&summary, max_chars);
        let font_size = (rect.height * 0.155).clamp(8.0, 11.5);
        let start_y = rect.y + rect.height * 0.52 - (lines.len().saturating_sub(1) as f64 * 6.0);
        for (index, line) in lines.iter().enumerate() {
            draw_text_centered(
                context,
                rect.x + rect.width / 2.0,
                start_y + index as f64 * (font_size + 3.0),
                line,
                font_size,
                FontWeight::Normal,
                palette.foreground,
            );
        }
    }
}

fn draw_overflow_chip(
    context: &Context,
    rect: Rect,
    key: &str,
    actions: &[String],
    hovered: bool,
    palette: Palette,
) {
    rounded_rectangle(context, rect, 7.0);
    set_source(
        context,
        if hovered {
            palette.accent.with_alpha(0.28)
        } else {
            palette.selection
        },
    );
    let _ = context.fill_preserve();
    context.set_line_width(if hovered { 2.0 } else { 1.0 });
    set_source(
        context,
        if hovered {
            palette.accent
        } else {
            palette.muted
        },
    );
    let _ = context.stroke();
    draw_text(
        context,
        rect.x + 9.0,
        rect.y + 15.0,
        &friendly_key_name(key),
        10.0,
        FontWeight::Bold,
        palette.accent,
    );
    draw_text(
        context,
        rect.x + 9.0,
        rect.y + 31.0,
        &elide(&actions.join(" · "), ((rect.width - 18.0) / 6.0) as usize),
        10.0,
        FontWeight::Normal,
        palette.foreground,
    );
}

fn draw_details(
    context: &Context,
    state: &OverlayState,
    actions: &BTreeMap<String, Vec<String>>,
    rect: Rect,
    palette: Palette,
) {
    rounded_rectangle(context, rect, 9.0);
    set_source(context, palette.darker_background.with_alpha(0.72));
    let _ = context.fill();

    let hovered = state.hovered.as_ref().and_then(|key| {
        state
            .metrics
            .hits
            .iter()
            .find(|hit| &hit.key == key)
            .map(|hit| (hit, actions.get(key)))
    });

    match hovered {
        Some((hit, Some(key_actions))) => {
            let chord = format!("{} + {}", state.modifiers.label(), hit.legend);
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + 21.0,
                &chord,
                12.0,
                FontWeight::Bold,
                palette.accent,
            );
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + 42.0,
                &elide(
                    &key_actions.join("  •  "),
                    ((rect.width - 28.0) / 7.0) as usize,
                ),
                14.0,
                FontWeight::Normal,
                palette.foreground,
            );
        }
        Some((hit, None)) => {
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + 33.0,
                &format!(
                    "{} + {}  ·  No binding",
                    state.modifiers.label(),
                    hit.legend
                ),
                13.0,
                FontWeight::Normal,
                palette.dark_foreground,
            );
        }
        None => {
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + 33.0,
                "Hover a key for its complete shortcut and action",
                13.0,
                FontWeight::Normal,
                palette.dark_foreground,
            );
            draw_text_right(
                context,
                rect.x + rect.width - 14.0,
                rect.y + 33.0,
                "Esc or backdrop to close",
                12.0,
                FontWeight::Normal,
                palette.dark_foreground,
            );
        }
    }
}

fn modifier_key_active(key: &str, modifiers: Modifiers) -> bool {
    match key {
        "SHIFTLEFT" | "SHIFTRIGHT" => modifiers.contains(Modifiers::SHIFT),
        "CTRLLEFT" | "CTRLRIGHT" => modifiers.contains(Modifiers::CTRL),
        "ALTLEFT" | "ALTRIGHT" => modifiers.contains(Modifiers::ALT),
        "SUPERLEFT" | "SUPERRIGHT" => true,
        _ => false,
    }
}

fn friendly_key_name(key: &str) -> String {
    match key {
        "MOUSE_DOWN" => "Mouse wheel down".into(),
        "MOUSE_UP" => "Mouse wheel up".into(),
        "LEFT MOUSE BUTTON" => "Left mouse button".into(),
        "RIGHT MOUSE BUTTON" => "Right mouse button".into(),
        value if value.starts_with("XF86") => value.trim_start_matches("XF86").into(),
        value => value.replace('_', " ").to_ascii_lowercase(),
    }
}

fn wrap_two_lines(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let words: Vec<_> = text.split_whitespace().collect();
    let mut first = String::new();
    let mut second = String::new();
    for word in words {
        let target = if first.is_empty() || first.len() + 1 + word.len() <= max_chars {
            &mut first
        } else {
            &mut second
        };
        if !target.is_empty() {
            target.push(' ');
        }
        target.push_str(word);
    }
    if second.chars().count() > max_chars {
        second = elide(&second, max_chars);
    }
    if first.chars().count() > max_chars {
        first = elide(&first, max_chars);
    }
    vec![first, second]
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect()
}

fn elide(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", text.chars().take(keep).collect::<String>())
}

fn rounded_rectangle(context: &Context, rect: Rect, radius: f64) {
    let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0);
    context.new_sub_path();
    context.arc(
        rect.x + rect.width - radius,
        rect.y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    context.arc(
        rect.x + rect.width - radius,
        rect.y + rect.height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    context.arc(
        rect.x + radius,
        rect.y + rect.height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    context.arc(
        rect.x + radius,
        rect.y + radius,
        radius,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    context.close_path();
}

fn set_source(context: &Context, color: Color) {
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
}

fn draw_text(
    context: &Context,
    x: f64,
    baseline_y: f64,
    text: &str,
    size: f64,
    weight: FontWeight,
    color: Color,
) {
    context.select_font_face("sans", FontSlant::Normal, weight);
    context.set_font_size(size);
    set_source(context, color);
    context.move_to(x, baseline_y);
    let _ = context.show_text(text);
}

fn draw_text_centered(
    context: &Context,
    center_x: f64,
    baseline_y: f64,
    text: &str,
    size: f64,
    weight: FontWeight,
    color: Color,
) {
    context.select_font_face("sans", FontSlant::Normal, weight);
    context.set_font_size(size);
    let width = context
        .text_extents(text)
        .map(|extents| extents.width())
        .unwrap_or(0.0);
    draw_text(
        context,
        center_x - width / 2.0,
        baseline_y,
        text,
        size,
        weight,
        color,
    );
}

fn draw_text_right(
    context: &Context,
    right_x: f64,
    baseline_y: f64,
    text: &str,
    size: f64,
    weight: FontWeight,
    color: Color,
) {
    context.select_font_face("sans", FontSlant::Normal, weight);
    context.set_font_size(size);
    let width = context
        .text_extents(text)
        .map(|extents| extents.width())
        .unwrap_or(0.0);
    draw_text(
        context,
        right_x - width,
        baseline_y,
        text,
        size,
        weight,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_current_gdk_modifier_state_and_keeps_super_implicit() {
        assert_eq!(
            modifiers_from_gdk(gdk::ModifierType::empty()),
            Modifiers::SUPER
        );
        assert_eq!(
            modifiers_from_gdk(
                gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK,
            ),
            Modifiers::SUPER
                .with(Modifiers::SHIFT)
                .with(Modifiers::CTRL)
                .with(Modifiers::ALT)
        );
    }

    #[test]
    fn state_changes_prefer_post_event_device_mask_over_stale_signal_mask() {
        assert_eq!(
            modifiers_from_state_change(
                gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK,
                Some(gdk::ModifierType::ALT_MASK),
            ),
            Modifiers::SUPER.with(Modifiers::ALT)
        );
        assert_eq!(
            modifiers_from_state_change(
                gdk::ModifierType::ALT_MASK,
                Some(gdk::ModifierType::empty()),
            ),
            Modifiers::SUPER
        );
        assert_eq!(
            modifiers_from_state_change(gdk::ModifierType::ALT_MASK, None),
            Modifiers::SUPER.with(Modifiers::ALT)
        );
    }

    #[test]
    fn modifier_key_events_only_change_their_mapped_modifier() {
        for (key, modifier) in [
            (gdk::Key::Shift_L, Modifiers::SHIFT),
            (gdk::Key::Shift_R, Modifiers::SHIFT),
            (gdk::Key::Control_L, Modifiers::CTRL),
            (gdk::Key::Control_R, Modifiers::CTRL),
            (gdk::Key::Alt_L, Modifiers::ALT),
            (gdk::Key::Alt_R, Modifiers::ALT),
        ] {
            assert_eq!(
                modifiers_for_key_event(Modifiers::SUPER, key, true),
                Some(Modifiers::SUPER.with(modifier))
            );
            assert_eq!(
                modifiers_for_key_event(Modifiers::SUPER.with(modifier), key, false),
                Some(Modifiers::SUPER)
            );
        }

        assert_eq!(
            modifiers_for_key_event(
                Modifiers::SUPER.with(Modifiers::ALT),
                gdk::Key::VoidSymbol,
                false,
            ),
            None
        );
    }
}
