//! Undo, redo and the History panel. As in Studio, the history keeps whole copies of the
//! project rather than asking each edit to describe its reverse: after each change it records
//! the project as it was before. Pixels are shared between copies (`Arc`), so a step costs
//! only the layers it changed. Each step has a name (the History panel lists them, as
//! Photoshop's does), and clicking one goes back or forward to it.
//!
//! The caller records only when no drag is in progress, so a whole brush stroke or move is
//! one step.

use canvas_core::Project;

/// A project as it was, its name in the History panel, and its version number.
struct Step {
    project: Project,
    name: String,
    version: u64,
}

pub struct History {
    /// The project as of the last recorded change: what the next change is compared against.
    baseline: Project,
    /// The name of the baseline's state (what made it).
    baseline_name: String,
    undo: Vec<Step>,
    redo: Vec<Step>,
    version: u64,
    next_version: u64,
    saved_version: u64,
    /// The most undo steps kept; older ones are dropped.
    pub limit: usize,
}

impl History {
    /// A fresh history for `project`, which counts as saved. `name` is its first state's name
    /// ("Open", "New").
    pub fn new(project: &Project, name: &str) -> Self {
        Self { baseline: project.clone(), baseline_name: name.to_owned(), undo: Vec::new(), redo: Vec::new(), version: 0, next_version: 1, saved_version: 0, limit: 50 }
    }

    /// Records `project` as a new step named `name` if it changed since the last one. Any redo
    /// steps are dropped, since they branched off an earlier state.
    pub fn record(&mut self, project: &Project, name: &str) -> bool {
        if *project == self.baseline {
            return false;
        }
        let before = std::mem::replace(&mut self.baseline, project.clone());
        let before_name = std::mem::replace(&mut self.baseline_name, name.to_owned());
        self.undo.push(Step { project: before, name: before_name, version: self.version });
        while self.undo.len() > self.limit.max(1) {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.version = self.next_version;
        self.next_version += 1;
        true
    }

    /// Puts `project` back one step. Returns whether there was anything to undo.
    pub fn undo(&mut self, project: &mut Project) -> bool {
        self.record(project, "Edit");
        let Some(step) = self.undo.pop() else { return false };
        let current_name = std::mem::replace(&mut self.baseline_name, step.name);
        self.redo.push(Step { project: std::mem::replace(project, step.project), name: current_name, version: self.version });
        self.baseline = project.clone();
        self.version = step.version;
        true
    }

    /// Re-applies the last undone step. Returns whether there was anything to redo.
    pub fn redo(&mut self, project: &mut Project) -> bool {
        self.record(project, "Edit");
        let Some(step) = self.redo.pop() else { return false };
        let current_name = std::mem::replace(&mut self.baseline_name, step.name);
        self.undo.push(Step { project: std::mem::replace(project, step.project), name: current_name, version: self.version });
        self.baseline = project.clone();
        self.version = step.version;
        true
    }

    pub fn can_undo(&self, project: &Project) -> bool {
        !self.undo.is_empty() || *project != self.baseline
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// The History panel's list, oldest first: each state's name, and which one is current.
    pub fn states(&self) -> (Vec<String>, usize) {
        let mut names: Vec<String> = self.undo.iter().map(|step| step.name.clone()).collect();
        names.push(self.baseline_name.clone());
        let current = names.len() - 1;
        names.extend(self.redo.iter().rev().map(|step| step.name.clone()));
        (names, current)
    }

    /// Goes back or forward to the state at `index` in `states`' list.
    pub fn go_to(&mut self, index: usize, project: &mut Project) -> bool {
        let (_, current) = self.states();
        let mut changed = false;
        if index < current {
            for _ in index..current {
                changed |= self.undo(project);
            }
        } else {
            for _ in current..index {
                changed |= self.redo(project);
            }
        }
        changed
    }

    /// Applies a change the app made by itself (fitting a text layer's box to its measured
    /// text) to the baseline too, so it isn't an undo step.
    pub fn absorb(&mut self, change: impl FnOnce(&mut Project)) {
        change(&mut self.baseline);
    }

    pub fn mark_saved(&mut self, project: &Project) {
        self.record(project, "Edit");
        self.saved_version = self.version;
    }

    /// Counts the project as not saved whatever is undone or redone, until it is next saved:
    /// for work brought back after Canvas closed without saving it.
    pub fn mark_unsaved(&mut self) {
        // No step ever has this version, so the project never matches the saved one.
        self.saved_version = u64::MAX;
    }

    pub fn has_unsaved_changes(&self, project: &Project) -> bool {
        self.version != self.saved_version || *project != self.baseline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_named_and_jumped_between() {
        let mut project = Project::default();
        let mut history = History::new(&project, "New");
        project.next_id = 2;
        history.record(&project, "First");
        project.next_id = 3;
        history.record(&project, "Second");
        assert_eq!(history.states(), (vec!["New".into(), "First".into(), "Second".into()], 2));
        assert!(history.go_to(0, &mut project));
        assert_eq!(project.next_id, 1);
        assert_eq!(history.states().1, 0);
        assert!(history.go_to(2, &mut project));
        assert_eq!(project.next_id, 3);
        assert!(history.has_unsaved_changes(&project));
        history.mark_saved(&project);
        assert!(!history.has_unsaved_changes(&project));
        history.undo(&mut project);
        assert!(history.has_unsaved_changes(&project));
    }

    #[test]
    fn old_steps_are_dropped_past_the_limit() {
        let mut project = Project::default();
        let mut history = History::new(&project, "New");
        history.limit = 3;
        for id in 2..10 {
            project.next_id = id;
            history.record(&project, "Step");
        }
        assert_eq!(history.states().0.len(), 4);
    }
}
