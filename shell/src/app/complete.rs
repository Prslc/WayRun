use super::State;

/// The byte after the last separator: the component Tab replaces.
fn leaf_start(query: &str) -> usize {
    query
        .char_indices()
        .rev()
        .find(|(_, ch)| matches!(ch, '/' | ' '))
        .map_or(0, |(at, ch)| at + ch.len_utf8())
}

fn same_char(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

fn title_starts(title: &str, leaf: &[char]) -> bool {
    let mut chars = title.chars();
    leaf.iter()
        .all(|wanted| chars.next().is_some_and(|ch| same_char(ch, *wanted)))
}

fn candidates<'a>(rows: &'a [wayrun_core::wire::ResultItem], leaf: &[char]) -> Vec<&'a str> {
    rows.iter()
        .map(|row| row.title.as_str())
        .filter(|title| title_starts(title, leaf))
        .collect()
}

fn common_prefix(candidates: &[&str]) -> String {
    let first: Vec<char> = candidates[0].chars().collect();
    let mut end = first.len();
    for candidate in &candidates[1..] {
        let other: Vec<char> = candidate.chars().collect();
        end = end.min(other.len());
        let mut at = 0;
        while at < end && same_char(first[at], other[at]) {
            at += 1;
        }
        end = at;
        if end == 0 {
            break;
        }
    }
    first[..end].iter().collect()
}

impl State {
    /// Tab: grow the last component to the candidates' common prefix, else take
    /// the selected row; the query only changes when the completion grows it.
    pub fn complete(&mut self) -> bool {
        if self.rows.is_empty() || self.query.is_empty() {
            return false;
        }
        let start = leaf_start(&self.query);
        let leaf: Vec<char> = self.query[start..].chars().collect();
        let candidates = candidates(&self.rows, &leaf);
        if candidates.is_empty() {
            return false;
        }

        let prefix = common_prefix(&candidates);
        let replacement = if prefix.chars().count() > leaf.len() {
            prefix
        } else {
            let Some(selected) = self.rows.get(self.cursor.selected) else {
                return false;
            };
            if !title_starts(&selected.title, &leaf) {
                return false;
            }
            selected.title.clone()
        };
        if replacement == self.query[start..] {
            return false;
        }
        self.query.replace_range(start.., &replacement);
        self.caret = self.query.len();
        self.anchor = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{item, state};

    fn with(query: &str, titles: &[&str]) -> State {
        let mut state = state();
        state.query = query.to_string();
        state.caret = query.len();
        state.rows = titles
            .iter()
            .map(|title| item(title, None, None, None))
            .collect();
        state
    }

    #[test]
    fn tab_grows_the_last_component_to_the_common_prefix() {
        let mut state = with("Proj", &["Project/", "Projects/"]);
        assert!(state.complete());
        assert_eq!(state.query, "Project");
        assert_eq!(state.caret, state.query.len());
    }

    #[test]
    fn a_single_directory_takes_its_slash() {
        let mut state = with("Proj", &["Project/"]);
        assert!(state.complete());
        assert_eq!(state.query, "Project/");
    }

    #[test]
    fn a_full_prefix_takes_the_selected_row() {
        let mut state = with("Project", &["Project/", "Projects/"]);
        assert!(state.complete());
        assert_eq!(state.query, "Project/");

        let mut state = with("Project", &["Project/", "Projects/"]);
        state.cursor.selected = 1;
        assert!(state.complete());
        assert_eq!(state.query, "Projects/");
    }

    #[test]
    fn only_the_last_component_is_replaced() {
        let mut state = with("f proj", &["Project/"]);
        assert!(state.complete());
        assert_eq!(state.query, "f Project/");

        let mut state = with("~/Proj", &["Project/"]);
        assert!(state.complete());
        assert_eq!(state.query, "~/Project/");
    }

    #[test]
    fn a_non_matching_query_is_left_alone() {
        let mut state = with("Proe", &["Project/"]);
        assert!(!state.complete());
        assert_eq!(state.query, "Proe");

        let mut state = with("Project/", &[]);
        assert!(!state.complete());
    }
}
