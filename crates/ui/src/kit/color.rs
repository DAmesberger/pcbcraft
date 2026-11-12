//! Document RGB and optional separate opacity editing. Interface paint uses tokens; gradients are colour data.

/// Text drafts are independent of model values. Copy retains the last accepted representation,
/// even while an incomplete/invalid draft is visible. Never round-trip opacity through its byte.
#[derive(Clone)]
struct HexEdit {
    draft: String,
    accepted: String,
    rgb: [u8; 3],
    opacity: Option<f64>,
    alpha: bool,
}

impl HexEdit {
    fn new(rgb: [u8; 3], opacity: Option<f64>) -> Self {
        let mut edit = Self { draft: String::new(), accepted: String::new(), rgb, opacity, alpha: opacity.is_some_and(|a| a != 1.0) };
        edit.set(rgb, opacity);
        edit
    }

    fn set(&mut self, rgb: [u8; 3], opacity: Option<f64>) {
        let [r, g, b] = rgb;
        self.accepted = format!("#{r:02x}{g:02x}{b:02x}");
        if self.alpha {
            let a = (opacity.unwrap_or(1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
            self.accepted.push_str(&format!("{a:02x}"));
        }
        self.draft.clone_from(&self.accepted);
        self.rgb = rgb;
        self.opacity = opacity;
    }
}

/// How a channel number edits: its range `0..=max`, the step of its step buttons, the drag
/// speed and the decimals shown.
#[derive(Clone, Copy)]
struct Spin {
    max: f64,
    step: f64,
    speed: f64,
    decimals: usize,
}

/// One popover's part in the recent list while it stays open (drawn on consecutive passes).
#[derive(Clone, Copy, Default)]
struct RecentSession {
    /// The pass the popover was last drawn on; a gap means it was closed and opened again.
    pass: u64,
    /// The colour changed and the edit is not finished yet (a button is still down).
    pending: bool,
    /// The colour this opening added, replaced by the next finished edit.
    entry: Option<[u8; 3]>,
}
