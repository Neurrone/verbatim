//! The slot a child's writer keeps the latest list of what Core holds in
//! (an outpost's held nodes, the listener's held view): the one rule for
//! when such a list is written. A list left in the slot replaces one not
//! yet written and goes with the next command written once every command
//! queued before it was left has been written, after that command's own
//! frame, so it never costs a write of its own; a list written at once
//! supersedes the one in the slot. Pure, so the supervisor's writer and a
//! test's stand-in for a child keep the same rule.

/// The slot for the latest held list, `M`.
#[derive(Debug)]
pub struct HeldSlot<M> {
    /// The latest list not yet written.
    held: Option<M>,
    /// How many commands queued before it was left still wait: it goes
    /// with the last of them, or, with none, with the next one written.
    behind: usize,
}

impl<M> Default for HeldSlot<M> {
    fn default() -> Self {
        Self {
            held: None,
            behind: 0,
        }
    }
}

impl<M> HeldSlot<M> {
    /// Leaves `held` in the slot, replacing a list not yet written, behind
    /// the `waiting` commands queued now.
    pub fn leave(&mut self, held: M, waiting: usize) {
        self.held = Some(held);
        self.behind = waiting;
    }

    /// A list is written at once: it supersedes the one in the slot, which
    /// is dropped unwritten.
    pub fn superseded(&mut self) {
        self.held = None;
    }

    /// A command that waited is taken to be written.
    pub fn popped(&mut self) {
        self.behind = self.behind.saturating_sub(1);
    }

    /// The list a command being written carries after its own frame: the
    /// slot's, once nothing queued before it was left still waits, unless
    /// the command is the shutdown message, the child's last.
    pub fn carried(&mut self, shutdown: bool) -> Option<M> {
        if shutdown || self.behind > 0 {
            return None;
        }
        self.held.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list left behind two waiting commands goes with the second; a
    /// newer list replaces it; one written at once supersedes it; the
    /// shutdown message carries none.
    #[test]
    fn a_left_list_goes_with_the_next_command_after_those_before_it() {
        let mut slot = HeldSlot::default();
        slot.leave(1, 2);
        slot.popped();
        assert_eq!(slot.carried(false), None);
        slot.popped();
        assert_eq!(slot.carried(false), Some(1));
        assert_eq!(slot.carried(false), None);
        slot.leave(2, 0);
        slot.leave(3, 0);
        assert_eq!(slot.carried(true), None);
        assert_eq!(slot.carried(false), Some(3));
        slot.leave(4, 0);
        slot.superseded();
        assert_eq!(slot.carried(false), None);
    }
}
