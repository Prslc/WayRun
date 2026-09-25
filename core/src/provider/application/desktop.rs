use super::model::{DesktopAction, DesktopMeta, Field};
use crate::system::desktop_action::Details;

/// The scoring surfaces of one app's `.desktop` extras. `desktop_action` owns the
/// file, its locale policy and the trimming; this only turns the text into the
/// forms a query is scored against.
pub(super) fn meta(details: &Details) -> DesktopMeta {
    DesktopMeta {
        generic: details.generic.as_deref().map(Field::new),
        keywords: details
            .keywords
            .iter()
            .map(|word| Field::new(word))
            .collect(),
        actions: details
            .actions
            .iter()
            .map(|(id, name)| DesktopAction {
                id: id.clone(),
                name_lower: name.to_lowercase(),
                name: name.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two case rules differ and are easy to conflate: a `Field` lowercases
    /// for matching, while a `DesktopAction` keeps its name for the panel row and
    /// carries `name_lower` beside it.
    #[test]
    fn the_scoring_surfaces_lowercase_and_keep_the_display_name() {
        let details = Details {
            generic: Some("Virtualization Software".to_string()),
            keywords: vec!["VirtualBox".to_string()],
            actions: vec![("Manager".to_string(), "Open VM Manager".to_string())],
            exec: None,
            terminal: false,
        };

        let meta = meta(&details);
        assert_eq!(meta.generic.unwrap().lower, "virtualization software");
        assert_eq!(meta.keywords[0].lower, "virtualbox");
        assert_eq!(meta.actions[0].name, "Open VM Manager");
        assert_eq!(meta.actions[0].name_lower, "open vm manager");
    }
}
