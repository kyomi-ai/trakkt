// SPDX-License-Identifier: AGPL-3.0-or-later

//! Draft ownership for the issue description's single debounced save worker.

#[derive(Clone, Debug)]
pub(super) struct DescriptionDraft {
    pub text: String,
    pub generation: u64,
    pub dirty: bool,
    pub focused: bool,
    worker_running: bool,
    acknowledged: String,
    deferred_remote: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct DescriptionSave {
    pub text: String,
    pub generation: u64,
}

impl DescriptionDraft {
    pub fn new(text: String) -> Self {
        Self {
            acknowledged: text.clone(),
            text,
            generation: 0,
            dirty: false,
            focused: false,
            worker_running: false,
            deferred_remote: None,
        }
    }

    /// Returns whether the caller must start the sole save worker. Repeating a
    /// dirty value retries a failed save without inventing another edit.
    pub fn edit(&mut self, text: String) -> bool {
        if text != self.text {
            self.text = text;
            self.generation += 1;
            self.dirty = true;
            // A local edit supersedes an update deferred while clean/focused.
            self.deferred_remote = None;
        }
        if !self.dirty || self.worker_running {
            return false;
        }
        self.worker_running = true;
        true
    }

    pub fn snapshot(&self) -> DescriptionSave {
        DescriptionSave {
            text: self.text.clone(),
            generation: self.generation,
        }
    }

    /// Only the worker's current generation can become clean. Acknowledgements
    /// never change the document; a newer draft must be saved by the same worker.
    pub fn acknowledge(&mut self, saved: &DescriptionSave) -> bool {
        self.acknowledged.clone_from(&saved.text);
        if saved.generation != self.generation {
            return false;
        }
        self.dirty = false;
        self.worker_running = false;
        true
    }

    /// A failure leaves the draft dirty. Keep processing if a newer edit was
    /// queued during the failed request, otherwise wait for an explicit retry.
    pub fn fail(&mut self, saved: &DescriptionSave) -> bool {
        if saved.generation != self.generation {
            return false;
        }
        self.worker_running = false;
        true
    }

    /// Dirty local drafts win over arriving remote data. Clean focused drafts
    /// defer replacement to blur, because kode reparsing resets selection/history.
    pub fn receive_remote(&mut self, text: String) -> Option<String> {
        if self.dirty {
            return None;
        }
        if text == self.acknowledged || text == self.text {
            self.deferred_remote = None;
            return None;
        }
        if self.focused {
            self.deferred_remote = Some(text);
            return None;
        }
        self.accept_remote(text)
    }

    fn accept_remote(&mut self, text: String) -> Option<String> {
        self.acknowledged.clone_from(&text);
        self.text.clone_from(&text);
        Some(text)
    }

    pub fn blur(&mut self) -> Option<String> {
        self.focused = false;
        if self.dirty {
            return None;
        }
        match self.deferred_remote.take() {
            Some(text) => self.accept_remote(text),
            None => Some(self.text.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DescriptionDraft;

    #[test]
    fn an_old_acknowledgement_keeps_the_newer_draft_dirty_and_queued() {
        let mut draft = DescriptionDraft::new(String::new());
        assert!(draft.edit("first".into()));
        let first = draft.snapshot();
        assert!(!draft.edit("second".into()));
        assert!(!draft.acknowledge(&first));
        assert_eq!(draft.text, "second");
        assert!(draft.dirty);
        let second = draft.snapshot();
        assert!(draft.acknowledge(&second));
        assert!(!draft.dirty);
        assert!(draft.receive_remote("second".into()).is_none());
    }

    #[test]
    fn failed_saves_retain_the_draft_and_can_be_retried() {
        let mut draft = DescriptionDraft::new("initial".into());
        assert!(draft.edit("unsaved".into()));
        assert!(draft.fail(&draft.snapshot()));
        assert!(draft.dirty);
        assert_eq!(draft.text, "unsaved");
        assert!(draft.edit("unsaved".into()));
        assert!(draft.acknowledge(&draft.snapshot()));
        assert!(!draft.dirty);
    }

    #[test]
    fn focused_remote_updates_wait_for_blur_and_local_edits_supersede_them() {
        let mut draft = DescriptionDraft::new("initial".into());
        draft.focused = true;
        assert!(draft.receive_remote("remote".into()).is_none());
        assert_eq!(draft.text, "initial");
        assert_eq!(draft.blur().as_deref(), Some("remote"));
        draft.focused = true;
        assert!(draft.receive_remote("another remote".into()).is_none());
        assert!(draft.edit("local wins".into()));
        assert!(draft.blur().is_none());
        assert!(draft.acknowledge(&draft.snapshot()));
        assert_eq!(draft.blur().as_deref(), Some("local wins"));
    }

    #[test]
    fn a_remote_reversion_cancels_the_deferred_replacement() {
        let mut draft = DescriptionDraft::new("initial".into());
        draft.focused = true;
        assert!(draft.receive_remote("remote".into()).is_none());
        assert!(draft.receive_remote("initial".into()).is_none());
        assert_eq!(draft.blur().as_deref(), Some("initial"));
    }

    #[test]
    fn save_echoes_never_replace_the_focused_document() {
        let mut draft = DescriptionDraft::new("initial".into());
        draft.focused = true;
        assert!(draft.edit("TRA-123".into()));
        assert!(draft.receive_remote("TRA-123".into()).is_none());
        assert!(draft.acknowledge(&draft.snapshot()));
        assert!(draft.receive_remote("TRA-123".into()).is_none());
        assert_eq!(draft.text, "TRA-123");
    }
}
