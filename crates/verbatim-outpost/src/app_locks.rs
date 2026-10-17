//! The focus listener's table of application locks
//! (`docs/design/focus-pipeline.md`, section 20.4; `verbatim_uia::lock`
//! says which calls take a lock and why).
//!
//! The supervisor sends the listener each application's lock as it creates
//! it ([`SupervisorToOutpost::AppLock`](crate::protocol::SupervisorToOutpost::AppLock)),
//! a handle to the mutex duplicated into the listener's process, and tells
//! it when the application's process has exited (`AppLockClosed`). The
//! table keeps each lock by the application's process id and creation time,
//! as a reference-counted handle: a reading thread holds a clone of its
//! own, so closing the table's reference never closes a handle a thread may
//! still wait on. For a process the supervisor gives no outpost, because it
//! has exited or cannot be opened, the supervisor answers with no lock,
//! which the table keeps as leave to read without one.
//!
//! A reading thread takes its application's lock before each read
//! ([`AppLocks::lock_for`]), waiting for it when it has not arrived yet:
//! the lock normally arrives while the thread makes its UIA client, since
//! the supervisor creates it as it routes the event's marker, which the
//! listener sends before it queues the event for the thread.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use verbatim_model::Pid;
use verbatim_uia::lock::AppLock;

/// What a reading thread is given for its application.
#[derive(Clone, Debug)]
pub(crate) enum Granted {
    /// The application's lock.
    Lock(AppLock),
    /// Leave to read without one: the supervisor gives the application no
    /// outpost, so no outpost calls into it.
    Without,
    /// Nothing: the listener is shutting down.
    Stopped,
}

/// The table's contents.
#[derive(Default)]
struct Table {
    /// Each application's lock, by process id and creation time.
    locks: HashMap<(Pid, u64), AppLock>,
    /// The supervisor's answers of no lock: the creation time it read for
    /// each process, or `None` when it could not open the process. The
    /// latest answer for each process id.
    unlocked: HashMap<Pid, Option<u64>>,
    /// Set at the listener's shutdown: nothing more is waited for.
    stopping: bool,
    /// How many times a waiting thread has looked at the table and found
    /// nothing for it: the evidence a test waits for that a thread waits.
    found_nothing: u64,
}

impl Table {
    /// What the table gives the process `pid` created at `created` now, if
    /// anything: its lock, or leave to read without one when the
    /// supervisor's answer of no lock names that creation time or none.
    fn grant(&self, pid: Pid, created: u64) -> Option<Granted> {
        if let Some(lock) = self.locks.get(&(pid, created)) {
            return Some(Granted::Lock(lock.clone()));
        }
        match self.unlocked.get(&pid) {
            Some(Some(answered)) if *answered == created => Some(Granted::Without),
            Some(None) => Some(Granted::Without),
            _ => None,
        }
    }
}

/// Which lock a reading thread reads with: the one it holds, when that
/// names its application's process, else what the table gives it, else
/// `None`, when it must wait for the table.
pub(crate) fn choose(
    held: Option<&AppLock>,
    (pid, created): (Pid, u64),
    table: impl FnOnce() -> Option<Granted>,
) -> Option<Granted> {
    match held {
        Some(lock) if lock.names(pid.0, created) => Some(Granted::Lock(lock.clone())),
        _ => table(),
    }
}

/// The listener's locks, and the condition its reading threads wait on for
/// one.
#[derive(Default)]
pub(crate) struct AppLocks {
    table: Mutex<Table>,
    /// Woken by every lock or answer added, and by the shutdown.
    changed: Condvar,
    /// Woken each time a waiting thread finds nothing for it, for a test
    /// that waits until a thread waits ([`AppLocks::found_nothing`]).
    looked: Condvar,
}

impl AppLocks {
    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds `lock`, its application's, and wakes the threads that wait.
    pub(crate) fn add(&self, lock: AppLock) {
        let mut table = self.table();
        let pid = Pid(lock.pid());
        table.unlocked.remove(&pid);
        table.locks.insert((pid, lock.created()), lock);
        drop(table);
        self.changed.notify_all();
    }

    /// Notes the supervisor's answer of no lock for `pid`, whose creation
    /// time it read as `created`, and wakes the threads that wait.
    pub(crate) fn add_unlocked(&self, pid: Pid, created: Option<u64>) {
        self.table().unlocked.insert(pid, created);
        self.changed.notify_all();
    }

    /// Lets the table's reference to the lock of `pid` created at
    /// `created` go: its process has exited. A thread that holds a clone
    /// keeps it until it lets it go.
    pub(crate) fn close(&self, pid: Pid, created: u64) {
        self.table().locks.remove(&(pid, created));
    }

    /// The lock of `pid` created at `created`, if the table holds it, for
    /// a call into another application than the thread's own, which waits
    /// for none (section 20.3).
    pub(crate) fn find(&self, pid: Pid, created: u64) -> Option<AppLock> {
        self.table().locks.get(&(pid, created)).cloned()
    }

    /// What the reading thread of `pid` created at `created`, holding
    /// `held`, reads with: the lock it holds when that names its process,
    /// else the table's, waiting for one to arrive.
    pub(crate) fn lock_for(&self, held: Option<&AppLock>, pid: Pid, created: u64) -> Granted {
        let mut table = self.table();
        loop {
            if table.stopping {
                return Granted::Stopped;
            }
            if let Some(granted) = choose(held, (pid, created), || table.grant(pid, created)) {
                return granted;
            }
            table.found_nothing += 1;
            self.looked.notify_all();
            table = self
                .changed
                .wait(table)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Waits until waiting threads have looked at the table and found
    /// nothing `times` times in all, for a test.
    #[cfg(test)]
    pub(crate) fn found_nothing(&self, times: u64) {
        let table = self.table();
        let _table = self
            .looked
            .wait_while(table, |table| table.found_nothing < times)
            .unwrap_or_else(PoisonError::into_inner);
    }

    /// The listener's shutdown: every thread that waits is woken and given
    /// nothing.
    pub(crate) fn stop(&self) {
        self.table().stopping = true;
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::OwnedHandle;
    use std::sync::Arc;

    use super::*;

    fn lock(pid: u32, created: u64) -> AppLock {
        let mutex: OwnedHandle = verbatim_uia::lock::new_mutex().expect("a mutex");
        AppLock::new(pid, created, Arc::new(mutex))
    }

    fn mutex_of(granted: Option<Granted>) -> Option<usize> {
        match granted {
            Some(Granted::Lock(lock)) => Some(Arc::as_ptr(lock.mutex()) as usize),
            _ => None,
        }
    }

    #[test]
    fn a_held_lock_whose_identity_no_longer_matches_is_replaced_by_the_tables() {
        let stale = lock(7, 100);
        let current = lock(7, 200);
        let table = || Some(Granted::Lock(current.clone()));
        assert_eq!(
            mutex_of(choose(Some(&stale), (Pid(7), 200), table)),
            mutex_of(Some(Granted::Lock(current.clone())))
        );
        // A held lock that names the process is kept, whatever the table
        // holds.
        assert_eq!(
            mutex_of(choose(Some(&current), (Pid(7), 200), || None)),
            mutex_of(Some(Granted::Lock(current.clone())))
        );
    }

    #[test]
    fn an_answer_of_no_lock_lets_a_process_read_when_its_creation_time_matches_or_is_unknown() {
        let locks = AppLocks::default();
        locks.add_unlocked(Pid(7), Some(200));
        assert!(locks.table().grant(Pid(7), 100).is_none());
        assert!(matches!(
            locks.table().grant(Pid(7), 200),
            Some(Granted::Without)
        ));
        locks.add_unlocked(Pid(7), None);
        assert!(matches!(
            locks.table().grant(Pid(7), 100),
            Some(Granted::Without)
        ));
    }
}
