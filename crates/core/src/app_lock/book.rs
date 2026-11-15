//! The lock state of every locked program (`docs/specs/app-lock.md`). Portable and unit-tested:
//! the platform thread reports windows and process exits, the book says what to do.
//!
//! A program is **locked** (its windows are hidden as they appear, until the password is typed)
//! or **unlocked** (it runs freely; the book remembers its processes, and once the last one has
//! exited the program is locked again). Every process of the same exe shares the state.

use std::collections::{BTreeSet, HashMap};

/// A window as the platform knows it (an `HWND` value on Windows).
pub type WindowId = isize;

/// A window the book hid, with the process that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hidden {
    pub pid: u32,
    pub window: WindowId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// Windows hidden so far, in the order they were hidden.
    Locked(Vec<Hidden>),
    /// The processes seen since the program was unlocked.
    Unlocked(BTreeSet<u32>),
}

/// What to do with a window that appeared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Not a locked program: leave it alone.
    Ignore,
    /// Hide the window and show the prompt for this program.
    HideAndPrompt,
    /// The program is unlocked: leave the window alone (its process is now remembered).
    Allow,
}

/// The lock state of the programs in `app_lock.apps`. Programs are keyed by their normalized
/// exe path (`app_lock::normalize_path`).
#[derive(Debug, Default)]
pub struct Book {
    apps: HashMap<String, State>,
    /// A password is set: without one nothing can be unlocked, so nothing is locked.
    armed: bool,
    /// Tray Pause: nothing new is locked.
    paused: bool,
}

impl Book {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether windows of locked programs are hidden now.
    pub fn active(&self) -> bool {
        self.armed && !self.paused
    }

    /// Whether a password exists (set by [`Self::set_armed`]).
    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn is_listed(&self, app: &str) -> bool {
        self.apps.contains_key(app)
    }

    #[cfg(test)]
    pub fn is_locked(&self, app: &str) -> bool {
        matches!(self.apps.get(app), Some(State::Locked(_)))
    }

    /// Replaces the list of programs. Programs no longer listed are forgotten; the windows the
    /// book had hidden for them are returned, to be shown again. New programs start locked.
    pub fn set_apps(&mut self, apps: &[String]) -> Vec<Hidden> {
        let mut restore = Vec::new();
        self.apps.retain(|app, state| {
            let keep = apps.contains(app);
            if !keep && let State::Locked(hidden) = state {
                restore.append(hidden);
            }
            keep
        });
        for app in apps {
            self.apps
                .entry(app.clone())
                .or_insert_with(|| State::Locked(Vec::new()));
        }
        restore
    }

    /// A program already running when it became locked by the user (added, App lock turned on,
    /// password set): it stays unlocked until `pid` (and its other processes) exit.
    pub fn adopt_running(&mut self, app: &str, pid: u32) {
        match self.apps.get_mut(app) {
            Some(State::Unlocked(pids)) => {
                pids.insert(pid);
            }
            Some(state @ State::Locked(_)) => {
                if let State::Locked(hidden) = state
                    && !hidden.is_empty()
                {
                    // Hidden windows mean the book is already locking it: keep that.
                    return;
                }
                *state = State::Unlocked(BTreeSet::from([pid]));
            }
            None => {}
        }
    }

    /// A window of `app` (process `pid`) appeared.
    pub fn on_window(&mut self, app: &str, pid: u32, window: WindowId) -> Decision {
        let active = self.active();
        let Some(state) = self.apps.get_mut(app) else {
            return Decision::Ignore;
        };
        match state {
            State::Locked(hidden) if active => {
                let h = Hidden { pid, window };
                if !hidden.contains(&h) {
                    hidden.push(h);
                }
                Decision::HideAndPrompt
            }
            // Started while paused or without a password: unlocked until it exits.
            State::Locked(hidden) => {
                debug_assert!(hidden.is_empty(), "inactive books hide nothing");
                *state = State::Unlocked(BTreeSet::from([pid]));
                Decision::Allow
            }
            State::Unlocked(pids) => {
                pids.insert(pid);
                Decision::Allow
            }
        }
    }

    /// A window the book hid could not be hidden (an elevated program): forget it, and let the
    /// program run unlocked (there is nothing the prompt could protect).
    pub fn hide_failed(&mut self, app: &str, h: Hidden) {
        if let Some(state) = self.apps.get_mut(app)
            && let State::Locked(hidden) = state
        {
            hidden.retain(|x| *x != h);
            if hidden.is_empty() {
                *state = State::Unlocked(BTreeSet::from([h.pid]));
            }
        }
    }

    /// The right password was typed for `app`: it becomes unlocked; returns the windows to show
    /// again (oldest first). `None` when `app` is not listed.
    pub fn unlock(&mut self, app: &str) -> Option<Vec<Hidden>> {
        let state = self.apps.get_mut(app)?;
        match state {
            State::Locked(hidden) => {
                let hidden = std::mem::take(hidden);
                *state = State::Unlocked(hidden.iter().map(|h| h.pid).collect());
                Some(hidden)
            }
            State::Unlocked(_) => Some(Vec::new()),
        }
    }

    /// Process `pid` exited: its hidden windows are forgotten, and a program left without any
    /// process is locked again. Returns the programs that were locked again.
    pub fn on_exit(&mut self, pid: u32) -> Vec<String> {
        let mut relocked = Vec::new();
        for (app, state) in &mut self.apps {
            match state {
                State::Locked(hidden) => hidden.retain(|h| h.pid != pid),
                State::Unlocked(pids) => {
                    if pids.remove(&pid) && pids.is_empty() {
                        *state = State::Locked(Vec::new());
                        relocked.push(app.clone());
                    }
                }
            }
        }
        relocked.sort();
        relocked
    }

    /// Sets whether a password exists. Disarming shows every hidden window again (returned) and
    /// leaves those programs unlocked until they exit.
    pub fn set_armed(&mut self, armed: bool) -> Vec<Hidden> {
        self.armed = armed;
        if armed {
            Vec::new()
        } else {
            self.release_all()
        }
    }

    /// Tray Pause. Pausing shows every hidden window again (returned) and leaves those programs
    /// unlocked until they exit.
    pub fn set_paused(&mut self, paused: bool) -> Vec<Hidden> {
        self.paused = paused;
        if paused {
            self.release_all()
        } else {
            Vec::new()
        }
    }

    fn release_all(&mut self) -> Vec<Hidden> {
        let mut restore = Vec::new();
        for state in self.apps.values_mut() {
            if let State::Locked(hidden) = state
                && !hidden.is_empty()
            {
                let pids = hidden.iter().map(|h| h.pid).collect();
                restore.append(hidden);
                *state = State::Unlocked(pids);
            }
        }
        restore
    }

    /// Every window hidden now.
    pub fn hidden(&self) -> Vec<Hidden> {
        let mut all: Vec<Hidden> = self
            .apps
            .values()
            .filter_map(|s| match s {
                State::Locked(h) => Some(h.iter().copied()),
                State::Unlocked(_) => None,
            })
            .flatten()
            .collect();
        all.sort_by_key(|h| (h.pid, h.window));
        all
    }

    /// Every process the book needs to hear about when it exits.
    pub fn processes(&self) -> BTreeSet<u32> {
        let mut all = BTreeSet::new();
        for state in self.apps.values() {
            match state {
                State::Locked(hidden) => all.extend(hidden.iter().map(|h| h.pid)),
                State::Unlocked(pids) => all.extend(pids.iter().copied()),
            }
        }
        all
    }

    /// Forgets everything; returns the windows to show again.
    pub fn clear(&mut self) -> Vec<Hidden> {
        let hidden = self.hidden();
        self.apps.clear();
        hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRAVE: &str = "c:\\b\\brave.exe";
    const NOTES: &str = "c:\\n\\notes.exe";

    fn book() -> Book {
        let mut b = Book::new();
        b.set_apps(&[BRAVE.into(), NOTES.into()]);
        b.set_armed(true);
        b
    }

    fn h(pid: u32, window: WindowId) -> Hidden {
        Hidden { pid, window }
    }

    #[test]
    fn a_locked_program_is_hidden_until_unlocked() {
        let mut b = book();
        assert_eq!(b.on_window("c:\\other.exe", 1, 10), Decision::Ignore);
        assert_eq!(b.on_window(BRAVE, 5, 50), Decision::HideAndPrompt);
        assert_eq!(b.on_window(BRAVE, 5, 51), Decision::HideAndPrompt);
        assert_eq!(
            b.on_window(BRAVE, 5, 50),
            Decision::HideAndPrompt,
            "shown again"
        );
        assert_eq!(b.hidden(), vec![h(5, 50), h(5, 51)]);
        assert_eq!(b.unlock(BRAVE), Some(vec![h(5, 50), h(5, 51)]));
        assert!(b.hidden().is_empty());
        // Unlocked: new windows and new processes run freely.
        assert_eq!(b.on_window(BRAVE, 5, 52), Decision::Allow);
        assert_eq!(b.on_window(BRAVE, 6, 60), Decision::Allow);
        assert_eq!(b.processes(), BTreeSet::from([5, 6]));
        // Other programs are still locked.
        assert_eq!(b.on_window(NOTES, 7, 70), Decision::HideAndPrompt);
        assert_eq!(b.unlock("c:\\unknown.exe"), None);
    }

    #[test]
    fn the_last_exit_locks_again() {
        let mut b = book();
        b.on_window(BRAVE, 5, 50);
        b.unlock(BRAVE);
        b.on_window(BRAVE, 6, 60);
        assert!(b.on_exit(5).is_empty(), "6 still runs");
        assert!(!b.is_locked(BRAVE));
        assert_eq!(b.on_exit(6), vec![BRAVE.to_string()]);
        assert!(b.is_locked(BRAVE));
        assert_eq!(b.on_window(BRAVE, 8, 80), Decision::HideAndPrompt);
        // A locked program whose process exits forgets its hidden windows.
        assert!(b.on_exit(8).is_empty());
        assert!(b.hidden().is_empty());
        assert!(b.processes().is_empty());
    }

    #[test]
    fn running_programs_adopted_by_the_user_stay_unlocked() {
        let mut b = book();
        b.adopt_running(BRAVE, 5);
        assert_eq!(b.on_window(BRAVE, 5, 50), Decision::Allow);
        assert_eq!(b.on_exit(5), vec![BRAVE.to_string()]);
        assert_eq!(b.on_window(BRAVE, 9, 90), Decision::HideAndPrompt);
        // Already hiding windows: adopting does not undo the lock.
        b.adopt_running(BRAVE, 9);
        assert!(b.is_locked(BRAVE));
        b.adopt_running("c:\\unknown.exe", 1);
        assert!(!b.is_listed("c:\\unknown.exe"));
    }

    #[test]
    fn without_password_or_while_paused_nothing_is_hidden() {
        let mut b = Book::new();
        b.set_apps(&[BRAVE.into()]);
        assert!(!b.active());
        assert_eq!(b.on_window(BRAVE, 5, 50), Decision::Allow);
        b.set_armed(true);
        assert_eq!(b.on_window(BRAVE, 5, 51), Decision::Allow, "started before");
        b.on_exit(5);
        assert_eq!(b.on_window(BRAVE, 6, 60), Decision::HideAndPrompt);
        // Pause shows the hidden windows and leaves the program unlocked.
        assert_eq!(b.set_paused(true), vec![h(6, 60)]);
        assert!(!b.is_locked(BRAVE));
        assert_eq!(b.on_window(BRAVE, 6, 61), Decision::Allow);
        assert!(b.set_paused(false).is_empty());
        assert_eq!(b.on_window(BRAVE, 6, 62), Decision::Allow, "until it exits");
        b.on_exit(6);
        assert_eq!(b.on_window(BRAVE, 7, 70), Decision::HideAndPrompt);
        assert_eq!(b.set_armed(false), vec![h(7, 70)]);
    }

    #[test]
    fn removed_programs_show_their_windows_again() {
        let mut b = book();
        b.on_window(BRAVE, 5, 50);
        b.on_window(NOTES, 7, 70);
        assert_eq!(b.set_apps(&[NOTES.into()]), vec![h(5, 50)]);
        assert!(!b.is_listed(BRAVE));
        assert_eq!(b.on_window(BRAVE, 5, 51), Decision::Ignore);
        assert_eq!(b.hidden(), vec![h(7, 70)]);
        assert_eq!(b.clear(), vec![h(7, 70)]);
        assert!(b.processes().is_empty());
    }

    #[test]
    fn a_window_that_cannot_be_hidden_leaves_the_program_alone() {
        let mut b = book();
        b.on_window(BRAVE, 5, 50);
        b.hide_failed(BRAVE, h(5, 50));
        assert!(!b.is_locked(BRAVE));
        assert_eq!(b.on_window(BRAVE, 5, 51), Decision::Allow);
    }
}
