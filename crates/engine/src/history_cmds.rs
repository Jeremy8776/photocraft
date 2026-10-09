//! History panel commands that are not Edit-menu items (`history.deleteState`).
//!
//! History is linear (`photocraft_ops::History`): past states, the current document, then the
//! redo states. "Delete current state" removes the step that produced the current document and
//! every step after it, as Photoshop's linear history does. It is an undo that also forgets the
//! redo states, so nothing can bring the deleted step back; the document returns to the state
//! before it. Both halves go through the session's own undo, so the selection and layer target are
//! restored the same way, and a running background job blocks it like it blocks Undo.

use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};

fn can_delete_state(s: &Session) -> std::result::Result<(), String> {
    s.active().filter(|d| d.history.can_undo()).map(|_| ()).ok_or_else(|| "no history state to delete".into())
}

/// `history.deleteState {}` → `{deleted}` (the label of the removed step).
fn delete_state(s: &mut Session, _: &Value) -> Result<Value> {
    let deleted = s.active().and_then(|d| d.history.undo_label().map(str::to_string)).ok_or_else(|| EngineError::Other("no history state to delete".into()))?;
    if !s.undo() {
        return Err(EngineError::Other("the state cannot be deleted while a background job is running".into()));
    }
    if let Some(st) = s.active_mut() {
        st.history.clear_redo();
    }
    Ok(json!({ "deleted": deleted }))
}

/// History panel command specs.
pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "history.deleteState",
        label: "Delete Current State",
        menu: &[],
        shortcut: None,
        params: r##"{} → {deleted} (undoes the current step and forgets it and every redo state; it cannot be redone)"##,
        enabled: can_delete_state,
        run: delete_state,
        journal: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 64, "height": 48})).unwrap();
        s
    }

    fn layers(s: &Session) -> usize {
        s.active().unwrap().doc.layers.len()
    }

    fn entries(s: &Session) -> Vec<String> {
        s.active().unwrap().history.entries()
    }

    #[test]
    fn deleting_the_current_state_goes_back_one_step_and_forgets_it() {
        let mut s = session();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        let (before_layers, before_entries) = (layers(&s), entries(&s).len());
        let r = s.execute("history.deleteState", json!({})).unwrap();
        assert!(r["deleted"].as_str().is_some_and(|l| !l.is_empty()), "{r}");
        assert_eq!(layers(&s), before_layers - 1, "the document is the state before the deleted step");
        assert_eq!(entries(&s).len(), before_entries - 1);
        assert!(!s.active().unwrap().history.can_redo(), "the deleted step cannot be redone");
        assert!(!s.redo());
    }

    #[test]
    fn deleting_after_an_undo_also_drops_the_redo_states() {
        let mut s = session();
        s.execute("layer.new.layer", json!({})).unwrap();
        s.execute("layer.new.layer", json!({})).unwrap();
        assert!(s.undo());
        assert!(s.active().unwrap().history.can_redo());
        let layers_now = layers(&s);
        s.execute("history.deleteState", json!({})).unwrap();
        assert_eq!(layers(&s), layers_now - 1);
        assert!(!s.active().unwrap().history.can_redo(), "the redo state after the deleted one is gone");
    }

    #[test]
    fn nothing_to_delete_is_a_refusal_not_a_panic() {
        let mut s = Session::new();
        assert!(s.execute("history.deleteState", json!({})).is_err(), "no document");
        s.execute("file.new", json!({"width": 8, "height": 8})).unwrap();
        assert_eq!(entries(&s).len(), 1);
        assert!(s.execute("history.deleteState", json!({})).is_err(), "only the opening state");
        assert_eq!(entries(&s).len(), 1, "a refused delete changes nothing");
    }

    #[test]
    fn params_are_ignored_and_a_non_object_is_refused() {
        for params in [json!("x"), json!([1, 2]), json!(7)] {
            let mut s = session();
            s.execute("layer.new.layer", json!({})).unwrap();
            let before = entries(&s).len();
            assert!(s.execute("history.deleteState", params).is_err());
            assert_eq!(entries(&s).len(), before, "a refused call changes nothing");
        }
        for params in [json!(null), json!({"layer": -1, "x": {"y": []}})] {
            let mut s = session();
            s.execute("layer.new.layer", json!({})).unwrap();
            let before = layers(&s);
            s.execute("history.deleteState", params).unwrap();
            assert_eq!(layers(&s), before - 1);
        }
    }
}
