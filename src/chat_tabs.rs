//! The agent window's tabs, like a browser's: each shows one thread, or a
//! new one not sent yet. Every project keeps its own row of tabs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabSet {
    pub project: Option<PathBuf>,
    /// Each tab's thread; `None` is a new thread.
    pub tabs: Vec<Option<u64>>,
    pub active: usize,
}

impl TabSet {
    pub fn new(project: Option<PathBuf>) -> Self {
        Self {
            project,
            tabs: vec![None],
            active: 0,
        }
    }

    /// The active tab's thread.
    pub fn current(&self) -> Option<u64> {
        self.tabs.get(self.active).copied().flatten()
    }

    /// Show `id` in the active tab, as following a link does, or switch to
    /// the tab that already shows it.
    pub fn navigate(&mut self, id: u64) {
        self.normalize();
        if let Some(ix) = self.position(id) {
            self.active = ix;
        } else {
            self.tabs[self.active] = Some(id);
        }
    }

    /// Show `id` in a tab of its own, unless one already shows it. An empty
    /// active tab is used rather than opening another.
    pub fn open(&mut self, id: u64) {
        self.normalize();
        if let Some(ix) = self.position(id) {
            self.active = ix;
        } else if self.tabs[self.active].is_none() {
            self.tabs[self.active] = Some(id);
        } else {
            self.active += 1;
            self.tabs.insert(self.active, Some(id));
        }
    }

    /// Open a tab for a new thread after the active one; an empty tab
    /// already open is reused.
    pub fn open_new(&mut self) {
        self.normalize();
        if let Some(ix) = self.tabs.iter().position(Option::is_none) {
            self.active = ix;
        } else {
            self.active += 1;
            self.tabs.insert(self.active, None);
        }
    }

    /// Close tab `ix`; the thread it showed. The last tab closed leaves an
    /// empty one, as a window always shows something.
    pub fn close(&mut self, ix: usize) -> Option<u64> {
        if ix >= self.tabs.len() {
            return None;
        }
        let closed = self.tabs.remove(ix);
        if self.active > ix || (self.active == ix && ix == self.tabs.len()) {
            self.active = self.active.saturating_sub(1);
        }
        self.normalize();
        closed
    }

    pub fn select(&mut self, ix: usize) {
        if ix < self.tabs.len() {
            self.active = ix;
        }
    }

    /// Move `delta` tabs along, wrapping around.
    pub fn cycle(&mut self, delta: isize) {
        self.normalize();
        let len = self.tabs.len() as isize;
        self.active = (self.active as isize + delta).rem_euclid(len) as usize;
    }

    /// Close every tab showing a thread for which `keep` is false.
    pub fn retain_threads(&mut self, keep: impl Fn(u64) -> bool) {
        let mut ix = 0;
        while ix < self.tabs.len() {
            match self.tabs[ix] {
                Some(id) if !keep(id) => {
                    self.close(ix);
                }
                _ => ix += 1,
            }
        }
        self.normalize();
    }

    fn position(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| *t == Some(id))
    }

    fn normalize(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(None);
        }
        self.active = self.active.min(self.tabs.len() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behaves_like_browser_tabs() {
        let mut set = TabSet::new(None);
        set.navigate(1);
        assert_eq!(set.tabs, [Some(1)]);
        set.open_new();
        assert_eq!((set.tabs.clone(), set.active), (vec![Some(1), None], 1));
        // A second new tab reuses the empty one.
        set.open_new();
        assert_eq!(set.tabs.len(), 2);
        set.navigate(2);
        set.open(3);
        assert_eq!((set.tabs.clone(), set.active), (vec![Some(1), Some(2), Some(3)], 2));
        // An open thread is switched to, not opened twice.
        set.navigate(1);
        assert_eq!(set.active, 0);
        set.cycle(-1);
        assert_eq!(set.active, 2);
        set.cycle(1);
        assert_eq!(set.active, 0);

        assert_eq!(set.close(0), Some(1));
        assert_eq!((set.tabs.clone(), set.active), (vec![Some(2), Some(3)], 0));
        set.select(1);
        set.close(1);
        assert_eq!((set.tabs.clone(), set.active), (vec![Some(2)], 0));
        set.close(0);
        assert_eq!((set.tabs.clone(), set.active), (vec![None], 0));
    }

    #[test]
    fn drops_tabs_of_gone_threads() {
        let mut set = TabSet {
            project: None,
            tabs: vec![Some(1), Some(2), None, Some(3)],
            active: 3,
        };
        set.retain_threads(|id| id != 2 && id != 3);
        assert_eq!((set.tabs.clone(), set.active), (vec![Some(1), None], 1));
        set.retain_threads(|_| false);
        assert_eq!(set.tabs, [None]);
    }
}
