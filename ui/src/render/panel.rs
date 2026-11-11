use wayrun_core::wire::IconShape;

use crate::view::Body;

use super::row;
use super::{DrawCtx, RowBody, RowWindow, draw_row};

/// The action panel (Shift+Enter): the row's secondary commands in the list
/// band. The parent row or group title is drawn by the footer.
pub(super) fn draw_actions(ctx: &mut DrawCtx) {
    let Body::Panel(panel) = &ctx.frame.body else {
        return;
    };
    let actions = panel.actions;
    let (first, selected, hovered, marked) =
        (panel.first, panel.selected, panel.hovered, panel.marked);
    let icon_tint = row::action_icon_tint(ctx.frame);
    let window = RowWindow {
        layout: ctx.frame.layout,
        surface: ctx.frame.surface,
        split: false,
        first,
        count: actions.len(),
        motion: &[],
        now: ctx.now,
    };

    // The dot marks the entry the row's Enter runs: the remembered default, or
    // the row's own command while none is remembered.
    for slot in window.slots() {
        let action = &actions[slot.index];
        let body = RowBody {
            rect: slot.rect,
            alpha: slot.alpha,
            selected: slot.index == selected,
            hovered: hovered == Some(slot.index),
            icon: action.icon.as_deref(),
            icon_tint: Some(icon_tint),
            icon_shape: IconShape::Square,
            title: &action.title,
            bold: false,
            summary: None,
            default_marker: marked == Some(slot.index),
            expand: !action.children.is_empty(),
        };
        draw_row(ctx, &body);
    }
}
