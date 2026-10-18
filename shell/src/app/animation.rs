use std::time::Instant;

use super::State;

impl State {
    pub fn animating(&self, now: Instant) -> bool {
        self.presentation.animating(now)
            || self
                .row_motion
                .iter()
                .flatten()
                .any(|motion| motion.is_active(now))
    }

    pub fn advance(&mut self, now: Instant) {
        self.presentation.advance(now);
    }

    /// The first frame of a show: this is where the entrance fade begins.
    pub fn start_entrance(&mut self, now: Instant) {
        if self.presentation.entrance_started {
            return;
        }
        self.presentation.start_entrance(now);
        self.presentation.snap_to(self.content_height(), now);
    }

    pub fn retarget_height(&mut self, now: Instant) {
        self.presentation.retarget(self.content_height(), now);
    }
}
