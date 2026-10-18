use std::cell::RefCell;
use std::rc::Rc;

use super::kv::Active;

/// The journal sink for a script's `print` and `wayrun.log`: stdout is the wire,
/// so everything goes to stderr, bounded per call so a loop cannot flood it.
#[derive(Clone)]
pub(super) struct Log {
    inner: Rc<Inner>,
}

struct Inner {
    script: String,
    active: Active,
    state: RefCell<State>,
    sink: Rc<dyn Fn(&str)>,
}

#[derive(Default)]
struct State {
    lines: usize,
    bytes: usize,
    dropped: bool,
}

const MAX_LINES: usize = 200;
const MAX_BYTES: usize = 64 * 1024;

impl Log {
    pub(super) fn new(script: &str, active: Active) -> Self {
        Self::with_sink(script, active, Rc::new(|line| eprintln!("{line}")))
    }

    pub(super) fn with_sink(script: &str, active: Active, sink: Rc<dyn Fn(&str)>) -> Self {
        Self {
            inner: Rc::new(Inner {
                script: script.to_string(),
                active,
                state: RefCell::new(State::default()),
                sink,
            }),
        }
    }

    /// A new plugin call gets a fresh budget.
    pub(super) fn reset(&self) {
        *self.inner.state.borrow_mut() = State::default();
    }

    pub(super) fn write(&self, message: &str) {
        let mut state = self.inner.state.borrow_mut();
        if state.dropped {
            return;
        }
        let lines = message.lines().count().max(1);
        if state.lines + lines > MAX_LINES || state.bytes + message.len() > MAX_BYTES {
            state.dropped = true;
            let line = self.prefix("log limit reached; further output in this call is dropped");
            (self.inner.sink)(&line);
            return;
        }
        state.lines += lines;
        state.bytes += message.len();
        let line = self.prefix(message);
        (self.inner.sink)(&line);
    }

    fn prefix(&self, message: &str) -> String {
        match self.inner.active.borrow().as_deref() {
            Some(id) => format!(
                "wayrun-core: lua plugin {} ({id}): {message}",
                self.inner.script
            ),
            None => format!("wayrun-core: lua plugin {}: {message}", self.inner.script),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collecting() -> (Log, Rc<RefCell<Vec<String>>>) {
        let active: Active = Rc::new(RefCell::new(Some("bench".to_string())));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = {
            let seen = Rc::clone(&seen);
            Rc::new(move |line: &str| seen.borrow_mut().push(line.to_string()))
        };
        (Log::with_sink("script.lua", active, sink), seen)
    }

    #[test]
    fn a_log_line_names_the_script_and_plugin() {
        let (log, seen) = collecting();
        log.write("hello");
        assert_eq!(
            seen.borrow().as_slice(),
            ["wayrun-core: lua plugin script.lua (bench): hello"]
        );
    }

    #[test]
    fn a_call_has_a_bounded_log_budget() {
        let (log, seen) = collecting();
        for _ in 0..MAX_LINES + 10 {
            log.write("x");
        }
        assert_eq!(seen.borrow().len(), MAX_LINES + 1, "the drop notice once");

        log.reset();
        log.write("again");
        assert_eq!(
            seen.borrow().last().unwrap(),
            "wayrun-core: lua plugin script.lua (bench): again"
        );
    }
}
