//! Unified per-tab undo/redo (design doc §1.7/§4.3): one linear history
//! covering edits from every editing surface - the curve-point editor,
//! the noise/luminance table, and the opponent-contrast editor alike -
//! in the order they actually happened, rather than each surface
//! maintaining its own separate history.
//!
//! Generic over `T: Clone + PartialEq` (originally written against
//! `xenovision_core::CurveSet` directly for the single-page app; the
//! three-window redesign tracks `state::EditableSnapshot` instead, but
//! the gesture-detection logic itself is unchanged - see the type
//! parameter rather than two near-duplicate modules).
//!
//! Implementation: whole-value snapshots rather than a command/diff log
//! (the design doc explicitly allows either). A snapshot is pushed once
//! per *edit gesture*, not once per frame - detected generically by
//! watching the value settle (stop changing from one frame to the next)
//! while the *same* widget has held keyboard focus throughout, rather
//! than instrumenting every single widget with its own "gesture
//! start/end" plumbing. This is what makes several editing surfaces
//! share one history for free: none of them need to know the undo
//! system exists.
//!
//! Tracking *which* widget is focused (an `egui::Id`), not just whether
//! something is, matters: tabbing or clicking from one field straight to
//! another changes the focused id in the same frame the value itself may
//! not yet have, so that transition - not just a return to zero focus -
//! has to count as a gesture boundary too. An earlier version only
//! checked "is anything focused", which meant moving between fields
//! without ever fully blurring never closed out the previous edit:
//! everything merged into one giant uncommitted gesture that a single
//! Undo discarded in one shot, with nothing ever reaching the committed
//! stack for Redo to restore.

use egui::Id;

const MAX_HISTORY: usize = 100;

pub struct UndoManager<T: Clone + PartialEq> {
    undo_stack: Vec<T>,
    redo_stack: Vec<T>,
    /// The value as of the start of an edit gesture still in progress
    /// (not yet settled), if any.
    pending_before: Option<T>,
    /// The value as of the end of the previous frame, used to detect
    /// "did anything change this frame".
    last_tick: Option<T>,
    /// The focused widget as of the end of the previous frame.
    last_focused: Option<Id>,
}

impl<T: Clone + PartialEq> Default for UndoManager<T> {
    fn default() -> Self {
        UndoManager {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_before: None,
            last_tick: None,
            last_focused: None,
        }
    }
}

impl<T: Clone + PartialEq> UndoManager<T> {
    /// Call once per frame, after every editing widget for `current` has
    /// had a chance to mutate it, with `focused` = `ctx.memory(|m|
    /// m.focused())`.
    pub fn observe(&mut self, current: &T, focused: Option<Id>) {
        // On the very first call there's no prior tick to compare
        // against - that's establishing the baseline, not a change (an
        // unconditional "changed" here would seed a no-op pending edit
        // that later commits as a false undo step).
        let changed_this_frame = match &self.last_tick {
            Some(last) => last != current,
            None => false,
        };
        if changed_this_frame && self.pending_before.is_none() {
            self.pending_before = self.last_tick.clone();
        }

        // A gesture ends when either: focus just moved *away* from a
        // prior target (covers tabbing/clicking straight from one
        // field to another, even if that same frame still carries the
        // outgoing field's last keystroke) - or focus is idle and the
        // value has stopped changing (covers a just-finished drag, which
        // never sets focus at all). Focus merely *arriving* (`None` ->
        // `Some`) must NOT itself close a gesture that started on this
        // same frame - that's the edit just beginning, not ending.
        let focus_left_a_real_target = self.last_focused.is_some() && focused != self.last_focused;
        let idle_and_settled = focused.is_none() && !changed_this_frame;
        if self.pending_before.is_some() && (focus_left_a_real_target || idle_and_settled) {
            let before = self.pending_before.take().unwrap();
            self.undo_stack.push(before);
            if self.undo_stack.len() > MAX_HISTORY {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
        }
        self.last_tick = Some(current.clone());
        self.last_focused = focused;
    }

    pub fn can_undo(&self) -> bool {
        self.pending_before.is_some() || !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Reverts `current` by one step: if an edit gesture is still
    /// in-progress (uncommitted), discards it back to its start;
    /// otherwise pops the most recent committed snapshot.
    pub fn undo(&mut self, current: &mut T) {
        if let Some(before) = self.pending_before.take() {
            *current = before;
            self.last_tick = Some(current.clone());
            return;
        }
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(current.clone());
            *current = prev;
            self.last_tick = Some(current.clone());
        }
    }

    pub fn redo(&mut self, current: &mut T) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(current.clone());
            *current = next;
            self.last_tick = Some(current.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xenovision_core::curve::{CurveType, SpectralCurve};
    use xenovision_core::curve_set::CurveSet;

    fn set_with_point(name: &str, wl: f64, v: f64) -> CurveSet {
        let mut s = CurveSet::new(name);
        s.colorspace_curves
            .push(SpectralCurve::new("c", CurveType::Sensitivity).with_points(vec![(wl, v)]));
        s
    }

    fn widget(n: u64) -> Option<Id> {
        Some(Id::new(n))
    }

    #[test]
    fn no_edit_means_nothing_to_undo() {
        let mut mgr: UndoManager<CurveSet> = UndoManager::default();
        let set = set_with_point("s", 500.0, 0.5);
        mgr.observe(&set, None);
        mgr.observe(&set, None);
        assert!(!mgr.can_undo());
    }

    #[test]
    fn single_settled_edit_is_one_undo_step() {
        let mut mgr: UndoManager<CurveSet> = UndoManager::default();
        let mut set = set_with_point("s", 500.0, 0.5);
        mgr.observe(&set, None); // frame 0: establishes baseline

        // Simulate a drag: value changes over several frames (one
        // gesture; point dragging never sets keyboard focus).
        set.colorspace_curves[0].points[0].1 = 0.6;
        mgr.observe(&set, None);
        set.colorspace_curves[0].points[0].1 = 0.7;
        mgr.observe(&set, None);
        set.colorspace_curves[0].points[0].1 = 0.8;
        mgr.observe(&set, None);
        // Settle: one more frame with no further change.
        mgr.observe(&set, None);

        assert!(mgr.can_undo());
        let before_undo = set.clone();
        mgr.undo(&mut set);
        assert_eq!(
            set.colorspace_curves[0].points[0].1, 0.5,
            "should revert to pre-gesture value"
        );
        assert!(mgr.can_redo());
        mgr.redo(&mut set);
        assert_eq!(set, before_undo);
    }

    #[test]
    fn focus_held_defers_commit_even_when_unchanged() {
        let mut mgr: UndoManager<CurveSet> = UndoManager::default();
        let mut set = set_with_point("s", 500.0, 0.5);
        mgr.observe(&set, None);

        set.colorspace_curves[0].points[0].1 = 0.9;
        mgr.observe(&set, widget(1)); // changed, and focused (typing)
        mgr.observe(&set, widget(1)); // unchanged, same field still focused -> not settled

        // Nothing has been committed to the undo stack yet - undoing
        // right now must discard the whole in-progress edit in one go
        // (back to 0.5), not step through intermediate frames.
        let mut probe = set.clone();
        mgr.undo(&mut probe);
        assert_eq!(probe.colorspace_curves[0].points[0].1, 0.5);
        assert!(!mgr.can_redo(), "a discarded pending edit isn't redoable");

        // Un-discard: re-establish the pending edit and let it settle.
        mgr.observe(&set, widget(1));
        mgr.observe(&set, None); // focus released -> settles now
        mgr.undo(&mut set);
        assert_eq!(
            set.colorspace_curves[0].points[0].1, 0.5,
            "now a real committed step"
        );
    }

    #[test]
    fn tabbing_between_fields_without_full_blur_still_closes_each_edit() {
        // Reproduces the reported bug: editing field A, then moving
        // straight to field B (never passing through "nothing focused"
        // in between) must still close out A's edit as its own
        // committed step - not merge into one giant pending blob that a
        // single Undo discards wholesale with nothing left for Redo.
        let mut mgr: UndoManager<CurveSet> = UndoManager::default();
        let mut set = set_with_point("s", 500.0, 0.5);
        mgr.observe(&set, None);

        // Edit field A (omega) - typing changes the value while focused on A.
        set.colorspace_curves[0].omega = Some(0.05);
        mgr.observe(&set, widget(1));

        // Tab to field B: focus moves this frame, but (as in real usage)
        // B's own value doesn't change until a later frame when the user
        // actually starts typing into it.
        mgr.observe(&set, widget(2));

        // A's edit must already be committed at this point - closed out
        // by the focus move itself, not by B's (still-to-come) edit.
        assert!(mgr.can_undo());

        // Now type into B.
        set.colorspace_curves[0].eta = Some(16.0);
        mgr.observe(&set, widget(2));

        // Settle B's edit too (e.g. click away entirely).
        mgr.observe(&set, None);

        assert_eq!(set.colorspace_curves[0].omega, Some(0.05));
        assert_eq!(set.colorspace_curves[0].eta, Some(16.0));

        mgr.undo(&mut set); // undoes B (eta)
        assert_eq!(set.colorspace_curves[0].eta, None);
        assert_eq!(
            set.colorspace_curves[0].omega,
            Some(0.05),
            "A's edit must survive"
        );

        mgr.undo(&mut set); // undoes A (omega)
        assert_eq!(set.colorspace_curves[0].omega, None);
        assert!(!mgr.can_undo());

        // And both should be redoable in order.
        mgr.redo(&mut set);
        assert_eq!(set.colorspace_curves[0].omega, Some(0.05));
        mgr.redo(&mut set);
        assert_eq!(set.colorspace_curves[0].eta, Some(16.0));
    }

    #[test]
    fn undo_discards_uncommitted_pending_edit() {
        let mut mgr: UndoManager<CurveSet> = UndoManager::default();
        let mut set = set_with_point("s", 500.0, 0.5);
        mgr.observe(&set, None);

        set.colorspace_curves[0].points[0].1 = 0.6; // mid-gesture, never settles
        mgr.observe(&set, None);

        mgr.undo(&mut set);
        assert_eq!(set.colorspace_curves[0].points[0].1, 0.5);
        assert!(!mgr.can_redo(), "a discarded pending edit isn't redoable");
    }

    #[test]
    fn interleaved_edits_across_surfaces_undo_in_order() {
        // Simulates: point edit, then a noise-field edit, then an
        // opponent-contrast edit, each its own settled gesture - the
        // three "surfaces" are just different fields on the same value
        // from this module's point of view.
        let mut mgr: UndoManager<CurveSet> = UndoManager::default();
        let mut set = set_with_point("s", 500.0, 0.5);
        mgr.observe(&set, None);

        // 1. "Point edit" surface (a drag: no focus involved).
        set.colorspace_curves[0].points[0].1 = 0.6;
        mgr.observe(&set, None);
        mgr.observe(&set, None); // settle

        // 2. "Noise table" surface (a focused text field).
        set.colorspace_curves[0].omega = Some(0.05);
        mgr.observe(&set, widget(1));
        mgr.observe(&set, None); // blur -> settle

        // 3. "Opponent contrast" surface (another focused field).
        set.opponent_contrasts
            .push(xenovision_core::curve_set::OpponentContrast {
                name: "x".to_string(),
                weights: vec![1.0],
            });
        mgr.observe(&set, widget(2));
        mgr.observe(&set, None); // blur -> settle

        assert_eq!(set.colorspace_curves[0].points[0].1, 0.6);
        assert_eq!(set.colorspace_curves[0].omega, Some(0.05));
        assert_eq!(set.opponent_contrasts.len(), 1);

        mgr.undo(&mut set); // undoes #3
        assert_eq!(set.opponent_contrasts.len(), 0);
        assert_eq!(
            set.colorspace_curves[0].omega,
            Some(0.05),
            "earlier edits remain"
        );

        mgr.undo(&mut set); // undoes #2
        assert_eq!(set.colorspace_curves[0].omega, None);
        assert_eq!(
            set.colorspace_curves[0].points[0].1, 0.6,
            "earlier edit remains"
        );

        mgr.undo(&mut set); // undoes #1
        assert_eq!(set.colorspace_curves[0].points[0].1, 0.5);

        assert!(!mgr.can_undo());
    }
}
