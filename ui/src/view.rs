use wayrun_core::wire::{ActionItem, ResultItem};

use crate::config::FontConfig;
use crate::geom::{Layout, PreviewKind};
use crate::present::RowMotion;
use crate::theme::{Surfaces, Theme};

/// What the field draws: the query, its caret and composition state.
pub struct FieldView<'a> {
    pub query: &'a str,
    /// A byte index into `query`, always on a char boundary.
    pub caret: usize,
    pub selection: Option<(usize, usize)>,
    pub preedit: Option<&'a str>,
    pub caret_visible: bool,
}

/// The result list, in its fixed window.
pub struct ListView<'a> {
    pub rows: &'a [ResultItem],
    pub motion: &'a [Option<RowMotion>],
    pub first: usize,
    pub selected: usize,
    pub hovered: Option<usize>,
    /// The selected row's split-view payload, when there is one.
    pub preview: Option<PreviewView<'a>>,
}

/// What the split view shows beside the list.
pub enum PreviewView<'a> {
    Image { path: &'a str },
    Text { text: &'a str },
}

impl PreviewView<'_> {
    /// The column shape this preview asks the layout for.
    pub fn kind(&self) -> PreviewKind {
        match self {
            Self::Image { .. } => PreviewKind::Image,
            Self::Text { .. } => PreviewKind::Text,
        }
    }
}

/// The action panel that replaces the list.
pub struct PanelView<'a> {
    pub parent_title: Option<&'a str>,
    pub actions: &'a [ActionItem],
    pub first: usize,
    pub selected: usize,
    pub hovered: Option<usize>,
    /// The entry the row's `Enter` runs, dotted.
    pub marked: Option<usize>,
}

pub enum Body<'a> {
    List(ListView<'a>),
    Panel(PanelView<'a>),
}

/// The footer's content and the hints its state selects.
#[derive(Default)]
pub struct FooterView<'a> {
    pub rows: usize,
    pub action_count: usize,
    pub query_empty: bool,
    pub panel: bool,
    /// A search has been in flight past the delay; the footer says so.
    pub loading: bool,
    pub has_actions: bool,
    pub can_default: bool,
    pub is_default: bool,
    pub effective_label: Option<&'a str>,
}

/// One frame, fully resolved: the renderer reads nothing else.
pub struct Frame<'a> {
    pub surface: (u32, u32),
    pub scale: f32,
    pub timing: bool,
    /// The layout a frame draws with, entrance offset included.
    pub layout: Layout,
    pub card_h: f32,
    /// The card's resting height, for the reflow band below a growing card.
    pub content_h: f32,
    pub last_card_bottom: f32,
    /// The entrance fade, already eased.
    pub opacity: f32,
    pub theme: Theme,
    pub surfaces: Surfaces,
    pub font: FontConfig,
    pub field: FieldView<'a>,
    pub body: Body<'a>,
    pub footer: FooterView<'a>,
}

impl Frame<'_> {
    /// A card-content colour at `alpha`, scaled by the entrance fade: the whole
    /// card arrives as one unit and the first frame really is invisible.
    pub fn fade(&self, color: [u8; 3], alpha: f32) -> [u8; 4] {
        [
            color[0],
            color[1],
            color[2],
            ((alpha * self.opacity).clamp(0.0, 1.0) * 255.0).round() as u8,
        ]
    }

    /// A resolved surface colour, whose own inline alpha rides the entrance.
    pub fn fade_rgba(&self, color: [u8; 4]) -> [u8; 4] {
        self.fade([color[0], color[1], color[2]], color[3] as f32 / 255.0)
    }

    /// [`Self::fade`] with a per-row alpha on top of the entrance.
    pub fn fade_scaled(&self, color: [u8; 3], alpha: f32, scale: f32) -> [u8; 4] {
        self.fade(color, alpha * scale)
    }

    /// [`Self::fade_rgba`] with a per-row alpha on top of the entrance.
    pub fn fade_rgba_scaled(&self, color: [u8; 4], scale: f32) -> [u8; 4] {
        self.fade(
            [color[0], color[1], color[2]],
            color[3] as f32 / 255.0 * scale,
        )
    }

    /// The backdrop dim's alpha (0 → the surface's alpha), not a progress fraction.
    pub fn dim_alpha(&self) -> f32 {
        (self.surfaces.dim[3] as f32 / 255.0) * self.opacity
    }

    /// The backdrop dim's colour, its alpha handled by [`Self::dim_alpha`].
    pub fn dim_color(&self) -> [u8; 3] {
        [
            self.surfaces.dim[0],
            self.surfaces.dim[1],
            self.surfaces.dim[2],
        ]
    }

    /// The muted text alpha, shared by the `fg`-tinted hints and the `primary`
    /// `⏎` markers, which take the same alpha over a different role.
    pub fn muted_alpha(&self) -> f32 {
        self.surfaces.muted[3] as f32 / 255.0
    }
}

#[cfg(test)]
pub(crate) fn test_frame(surface: (u32, u32)) -> Frame<'static> {
    let layout = Layout::default();
    let theme = Theme::default();
    Frame {
        surface,
        scale: 1.0,
        timing: false,
        layout,
        card_h: layout.empty_h(),
        content_h: layout.empty_h(),
        last_card_bottom: 0.0,
        opacity: 1.0,
        theme,
        surfaces: Surfaces::resolve(
            theme,
            &Default::default(),
            &Default::default(),
            crate::config::Mode::Dark,
        ),
        font: FontConfig::default(),
        field: FieldView {
            query: "",
            caret: 0,
            selection: None,
            preedit: None,
            caret_visible: true,
        },
        body: Body::List(ListView {
            rows: &[],
            motion: &[],
            first: 0,
            selected: 0,
            hovered: None,
            preview: None,
        }),
        footer: FooterView::default(),
    }
}
