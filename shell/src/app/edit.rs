use super::State;

impl State {
    /// The selected byte range, `None` when the caret is a bare point.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (start, end) = if anchor < self.caret {
            (anchor, self.caret)
        } else {
            (self.caret, anchor)
        };
        (start != end).then_some((start, end))
    }

    /// Ctrl+A.
    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.caret = self.query.len();
    }

    /// Remove the selected range, if any; whether it removed something. The
    /// anchor is always cleared: this is also what collapses a selection.
    pub fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            self.anchor = None;
            return false;
        };

        self.query.replace_range(start..end, "");
        self.caret = start;
        self.anchor = None;
        true
    }

    /// Typing over a selection replaces it, as it does in any text field.
    pub fn insert(&mut self, text: &str) {
        self.delete_selection();
        self.caret = self.caret.min(self.query.len());
        self.query.insert_str(self.caret, text);
        self.caret += text.len();
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if let Some(at) = prev_boundary(&self.query, self.caret) {
            self.query.remove(at);
            self.caret = at;
        }
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.caret < self.query.len() {
            self.query.remove(self.caret);
        }
    }

    /// `delete_surrounding_text` lengths are UTF-8 *bytes*: remove characters
    /// while they fit, stop at the ends, so a bogus length cannot spin.
    pub fn delete_surrounding(&mut self, before: u32, after: u32) {
        // The IME is about to replace whatever is selected.
        self.delete_selection();
        self.caret = self.caret.min(self.query.len());

        let mut budget = before as usize;
        while budget > 0 {
            let Some(at) = prev_boundary(&self.query, self.caret) else {
                break;
            };
            let len = self.caret - at;
            if len > budget {
                break;
            }
            budget -= len;
            self.backspace();
        }

        let mut budget = after as usize;
        while budget > 0 {
            let Some(len) = next_char_len(&self.query, self.caret) else {
                break;
            };
            if len > budget {
                break;
            }
            budget -= len;
            self.delete();
        }
    }

    /// Home/End are deliberately not intercepted as widget shortcuts: they
    /// belong to the caret.
    pub fn home(&mut self) {
        self.anchor = None;
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.anchor = None;
        self.caret = self.query.len();
    }

    pub fn left(&mut self) {
        // With a selection, a bare arrow collapses to the end it moves towards.
        if let Some((start, _)) = self.selection() {
            self.anchor = None;
            self.caret = start;
            return;
        }
        self.anchor = None;
        if let Some(at) = prev_boundary(&self.query, self.caret) {
            self.caret = at;
        }
    }

    pub fn right(&mut self) {
        if let Some((_, end)) = self.selection() {
            self.anchor = None;
            self.caret = end;
            return;
        }
        self.anchor = None;
        if let Some(len) = next_char_len(&self.query, self.caret) {
            self.caret += len;
        }
    }

    /// Shift+Left/Right/Home/End extend from wherever the caret sat when the
    /// shift was first held.
    pub fn extend_left(&mut self) {
        self.anchor.get_or_insert(self.caret);
        if let Some(at) = prev_boundary(&self.query, self.caret) {
            self.caret = at;
        }
    }

    pub fn extend_right(&mut self) {
        self.anchor.get_or_insert(self.caret);
        if let Some(len) = next_char_len(&self.query, self.caret) {
            self.caret += len;
        }
    }

    pub fn extend_home(&mut self) {
        self.anchor.get_or_insert(self.caret);
        self.caret = 0;
    }

    pub fn extend_end(&mut self) {
        self.anchor.get_or_insert(self.caret);
        self.caret = self.query.len();
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.caret = 0;
        self.anchor = None;
    }

    /// The text before the caret, which is what the IME wants as surrounding
    /// text (only its byte length is reported, since nothing tracks it).
    pub fn before_caret(&self) -> &str {
        &self.query[..self.caret]
    }

    /// The active keyword prefix (`b`, `h`, `f`, …): `^([a-zA-Z]{1,3})\s`.
    pub fn keyword_prefix(&self) -> Option<&str> {
        let end = self
            .query
            .as_bytes()
            .iter()
            .take(4)
            .position(|byte| *byte == b' ')?;
        if end == 0
            || !self.query.as_bytes()[..end]
                .iter()
                .all(u8::is_ascii_alphabetic)
        {
            return None;
        }

        Some(&self.query[..end])
    }
}

/// The byte index where the character before `at` starts; `at` must be a char
/// boundary, and the start of the text has nothing before it.
fn prev_boundary(text: &str, at: usize) -> Option<usize> {
    text[..at]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
}

/// The UTF-8 length of the character at `at`, or `None` at the end.
fn next_char_len(text: &str, at: usize) -> Option<usize> {
    text[at..].chars().next().map(char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::state;

    #[test]
    fn boundaries_are_char_steps_not_bytes() {
        let text = "aé中";
        assert_eq!(prev_boundary(text, text.len()), Some(3));
        assert_eq!(prev_boundary(text, 3), Some(1));
        assert_eq!(prev_boundary(text, 1), Some(0));
        assert_eq!(prev_boundary(text, 0), None);

        assert_eq!(next_char_len(text, 0), Some(1));
        assert_eq!(next_char_len(text, 1), Some(2));
        assert_eq!(next_char_len(text, 3), Some(3));
        assert_eq!(next_char_len(text, text.len()), None);
    }

    #[test]
    fn editing_is_utf8_safe() {
        let mut state = state();
        state.insert("中文");
        state.insert("ab");
        assert_eq!(state.query, "中文ab");
        assert_eq!(state.caret, "中文ab".len());

        // backspace removes a whole character, not a byte
        state.backspace();
        state.backspace();
        assert_eq!(state.query, "中文");

        state.home();
        assert_eq!(state.caret, 0);
        state.delete();
        assert_eq!(state.query, "文");
        state.end();
        assert_eq!(state.caret, "文".len());

        // the caret never leaves the string
        state.left();
        state.left();
        assert_eq!(state.caret, 0);
        state.right();
        state.right();
        assert_eq!(state.caret, "文".len());

        state.clear_query();
        assert!(state.query.is_empty() && state.caret == 0);
    }

    #[test]
    fn deleting_surrounding_text_counts_bytes_not_characters() {
        let mut state = state();

        // one CJK character is three bytes: a three-byte request before the
        // caret removes exactly that character
        state.insert("中文");
        state.delete_surrounding(3, 0);
        assert_eq!(state.query, "中");
        assert_eq!(state.caret, "中".len());

        // ASCII is one byte a character
        state.insert("ab");
        state.delete_surrounding(1, 0);
        assert_eq!(state.query, "中a");
        state.delete_surrounding(1, 0);
        assert_eq!(state.query, "中");

        // a character that does not fit the budget is left alone: the request is
        // never exceeded, so no user text disappears for a one-byte ask
        state.delete_surrounding(1, 0);
        assert_eq!(state.query, "中");

        // forward deletion counts bytes the same way
        state.caret = 0;
        state.delete_surrounding(0, 3);
        assert_eq!(state.query, "");

        // a bogus length is clamped by the ends of the query, not looped
        state.insert("x");
        state.delete_surrounding(u32::MAX, u32::MAX);
        assert_eq!(state.query, "");
        assert_eq!(state.caret, 0);
    }

    #[test]
    fn ctrl_a_selects_the_query_and_editing_replaces_it() {
        let mut state = state();
        state.insert("firefox");
        state.select_all();
        assert_eq!(state.selection(), Some((0, 7)));

        // typing over a selection replaces it
        state.insert("kit");
        assert_eq!(state.query, "kit");
        assert_eq!(state.caret, 3);
        assert_eq!(state.selection(), None);

        // backspace over a selection removes all of it, not one character
        state.insert("ty");
        state.select_all();
        state.backspace();
        assert!(state.query.is_empty(), "{:?}", state.query);

        // a bare arrow collapses the selection instead of moving the caret
        state.insert("abcdef");
        state.select_all();
        state.left();
        assert_eq!((state.caret, state.selection()), (0, None));
        state.select_all();
        state.right();
        assert_eq!((state.caret, state.selection()), (6, None));
    }

    #[test]
    fn shift_arrows_extend_a_selection_from_the_anchor() {
        let mut state = state();
        state.insert("abc");
        state.home();

        state.extend_right();
        state.extend_right();
        assert_eq!(state.selection(), Some((0, 2)));
        // walking back to the anchor leaves no selection at all
        state.extend_left();
        assert_eq!(state.selection(), Some((0, 1)));
        state.extend_left();
        assert_eq!(state.selection(), None);

        // anchored in the middle, extending the other way flips the ends
        state.clear_query();
        state.insert("abc");
        state.home();
        state.right();
        state.right();
        state.extend_home();
        assert_eq!(state.selection(), Some((0, 2)));
        state.extend_right();
        assert_eq!(state.selection(), Some((1, 2)));
        state.extend_right();
        assert_eq!(state.selection(), None, "back at the anchor");
    }

    #[test]
    fn a_keyword_prefix_is_one_to_three_letters_and_a_space() {
        let mut state = state();
        state.query = "b firefox".into();
        assert_eq!(state.keyword_prefix(), Some("b"));
        state.query = "tr 你好".into();
        assert_eq!(state.keyword_prefix(), Some("tr"));
        state.query = "abcd ".into();
        assert_eq!(state.keyword_prefix(), None);
        state.query = "1 x".into();
        assert_eq!(state.keyword_prefix(), None);
        state.query = "b".into();
        assert_eq!(state.keyword_prefix(), None);
    }
}
