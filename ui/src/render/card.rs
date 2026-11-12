use crate::geom::Rect;
use crate::view::Frame;

use super::DrawCtx;

/// The card's own tokens: a translucent fill and the hairline ring around it.
struct Style {
    fill: [u8; 4],
    hairline: [u8; 4],
    hairline_width: f32,
}

impl Style {
    fn of(frame: &Frame<'_>) -> Self {
        Self {
            fill: frame.surfaces.card,
            hairline: frame.surfaces.hairline,
            hairline_width: frame.surfaces.hairline_width,
        }
    }
}

/// The card: a translucent fill with a hairline ring. The ring is stroked, never
/// a base fill, which would raise the interior's alpha.
pub(super) fn draw(ctx: &mut DrawCtx) {
    let frame = ctx.frame;
    let style = Style::of(frame);
    let card = frame.layout.card_rect(frame.surface, frame.card_h);
    let border = style.hairline_width;
    let radius = frame.layout.radius;

    ctx.canvas.fill_round(
        ctx.pixmap,
        Rect {
            x: card.x + border,
            y: card.y + border,
            w: card.w - 2.0 * border,
            h: card.h - 2.0 * border,
        },
        radius - border,
        frame.fade_rgba(style.fill),
    );
    if border > 0.0 {
        ctx.canvas.stroke_round(
            ctx.pixmap,
            Rect {
                x: card.x + border / 2.0,
                y: card.y + border / 2.0,
                w: card.w - border,
                h: card.h - border,
            },
            frame.layout.hairline_radius(border),
            border,
            frame.fade_rgba(style.hairline),
        );
    }
}
