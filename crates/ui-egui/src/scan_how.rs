//! The keys that say how a click, a box or a brush stroke changes what is chosen, the same in every
//! Scan-tab tool: Shift adds to the choice, Ctrl (Command on macOS) takes from it, Alt takes away
//! too. Free Form's handles (Edit Surface) read Ctrl as a toggle, which is a remove for what is
//! chosen and an add for what is not; Select on Scan has no toggle, so for it Ctrl is a remove.
//! Ctrl and Shift together are the Smoothing Brush's and the brush circle's wheel, so they choose
//! nothing special. The wheel does the same everywhere too: Shift sizes the soft selection (Free
//! Form's falloff), Ctrl+Shift sizes the brush (Free Form's Smoothing Brush, Select on Scan's brush).

/// How a click or a box changes what is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum How {
    /// The new choice replaces the old.
    Replace,
    /// The new choice joins it.
    Add,
    /// What is new and not chosen is chosen, what is chosen is let go.
    Toggle,
    /// The new choice is let go.
    Remove,
}

impl How {
    /// By the keys held: Shift adds, Ctrl (Command on macOS) toggles (takes from what is chosen),
    /// Alt takes away. Ctrl and Shift together are the brush's, so they choose nothing special.
    pub(crate) fn of(mods: egui::Modifiers) -> How {
        let ctrl = mods.ctrl || mods.command;
        match (mods.alt, mods.shift, ctrl) {
            (true, _, _) => How::Remove,
            (false, true, false) => How::Add,
            (false, false, true) => How::Toggle,
            _ => How::Replace,
        }
    }

    /// Whether the keys held keep what is chosen (a click then does not start again).
    pub(crate) fn keeps(self) -> bool {
        self != How::Replace
    }

    /// The mode of Select on Scan this stands for, an index into its replace, add, remove: a toggle
    /// takes away, there being no toggle to make on a region or a brush.
    pub(crate) fn select_mode(self) -> Option<usize> {
        match self {
            How::Replace => None,
            How::Add => Some(1),
            How::Toggle | How::Remove => Some(2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(shift: bool, ctrl: bool, alt: bool) -> egui::Modifiers {
        egui::Modifiers { shift, ctrl, alt, ..Default::default() }
    }

    /// Shift adds, Ctrl and Alt take from the choice, nothing replaces, and Ctrl with Shift is the
    /// brush's, so it replaces; Select on Scan has modes for what each stands for.
    #[test]
    fn the_keys_choose_the_same_way_in_every_tool() {
        assert_eq!(How::of(keys(false, false, false)), How::Replace);
        assert_eq!(How::of(keys(true, false, false)), How::Add);
        assert_eq!(How::of(keys(false, true, false)), How::Toggle);
        assert_eq!(How::of(keys(false, false, true)), How::Remove);
        assert_eq!(How::of(keys(true, true, false)), How::Replace);
        assert_eq!([How::Replace, How::Add, How::Toggle, How::Remove].map(How::select_mode), [None, Some(1), Some(2), Some(2)]);
        assert!(How::Add.keeps() && !How::Replace.keeps());
    }
}
