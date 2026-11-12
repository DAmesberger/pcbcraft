//! Per-row animation memory keyed by row: the committed-value flash. Row
//! moves (FLIP: reorder, fold, insert) animate in the list itself through
//! `animate_value_with_time` on each row's y. Every duration comes from
//! the list tokens, so reduced motion makes all of them immediate.
use super::model::RowKey;
use super::tokens::ListTokens;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
struct Flashes(BTreeMap<RowKey, f64>);

/// Flash the row `key` of the list `id` once (a committed value).
pub fn start_flash(ctx: &egui::Context, id: egui::Id, key: RowKey) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| {
        let flashes = d.get_temp_mut_or_default::<Flashes>(id.with("flashes"));
        flashes.0.insert(key, now);
    });
    ctx.request_repaint();
}

/// The flash of row `key` now, from 1 down to 0; 0 with reduced motion.
pub fn flash(ctx: &egui::Context, id: egui::Id, key: RowKey) -> f32 {
    let l = ListTokens::get(ctx);
    let now = ctx.input(|i| i.time);
    let slot = id.with("flashes");
    let Some(start) = ctx.data(|d| {
        d.get_temp::<Flashes>(slot)
            .and_then(|f| f.0.get(&key).copied())
    }) else {
        return 0.0;
    };
    let t = (now - start) as f32;
    if l.flash_time <= 0.0 || t >= l.flash_time {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Flashes>(slot).0.remove(&key));
        return 0.0;
    }
    ctx.request_repaint();
    // Held for the first third, then fading out.
    let hold = l.flash_time * 0.3;
    if t <= hold {
        1.0
    } else {
        1.0 - (t - hold) / (l.flash_time - hold)
    }
}
