use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Local};
use uuid::Uuid;

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Local>;
}

pub trait Notifier: Send + Sync + 'static {
    fn notify(&self, title: &str, body: &str);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Local> {
        Local::now()
    }
}

pub struct DBusNotifier;

impl Notifier for DBusNotifier {
    fn notify(&self, title: &str, body: &str) {
        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .show();
    }
}

#[derive(Clone, Debug)]
pub struct ScheduledTodo {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub fire_at: DateTime<Local>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingOverdueBatch,
    Steady,
}

pub struct Scheduler {
    clock: Arc<dyn Clock>,
    notifier: Arc<dyn Notifier>,
    entries: HashMap<Uuid, ScheduledTodo>,
    fired_ids: HashSet<Uuid>,
    phase: Phase,
}

impl Scheduler {
    pub fn new(clock: Arc<dyn Clock>, notifier: Arc<dyn Notifier>) -> Self {
        Self {
            clock,
            notifier,
            entries: HashMap::new(),
            fired_ids: HashSet::new(),
            phase: Phase::AwaitingOverdueBatch,
        }
    }

    pub fn upsert(&mut self, todo: ScheduledTodo) {
        if self.fired_ids.contains(&todo.id) {
            return;
        }
        self.entries.insert(todo.id, todo);
    }

    pub fn cancel(&mut self, id: Uuid) {
        self.entries.remove(&id);
        self.fired_ids.remove(&id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn tick(&mut self) {
        let now = self.clock.now();
        match self.phase {
            Phase::AwaitingOverdueBatch => {
                let overdue = self.take_fired(|t| t.fire_at < now);
                if !overdue.is_empty() {
                    self.notifier.notify(
                        "arc — overdue todos",
                        &format!("{} overdue todos. Check the arc TUI.", overdue.len()),
                    );
                }
                self.phase = Phase::Steady;
            }
            Phase::Steady => {
                for t in self.take_fired(|t| t.fire_at <= now) {
                    self.notifier.notify(&t.title, &t.body);
                }
            }
        }
    }

    fn take_fired(&mut self, is_due: impl Fn(&ScheduledTodo) -> bool) -> Vec<ScheduledTodo> {
        let fired: Vec<ScheduledTodo> = self
            .entries
            .extract_if(|_, t| is_due(t))
            .map(|(_, t)| t)
            .collect();
        self.fired_ids.extend(fired.iter().map(|t| t.id));
        fired
    }

    pub fn next_fire(&self) -> Option<DateTime<Local>> {
        self.entries.values().map(|t| t.fire_at).min()
    }
}

#[cfg(test)]
mod traits_tests {
    use super::*;
    use chrono::TimeZone;

    pub struct FakeClock(pub std::sync::Mutex<DateTime<Local>>);

    impl FakeClock {
        pub fn new(at: DateTime<Local>) -> Self {
            Self(std::sync::Mutex::new(at))
        }
        pub fn advance(&self, by: chrono::Duration) {
            let mut g = self.0.lock().unwrap();
            *g += by;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Local> {
            *self.0.lock().unwrap()
        }
    }

    #[derive(Default)]
    pub struct CapturingNotifier(pub std::sync::Mutex<Vec<(String, String)>>);

    impl Notifier for CapturingNotifier {
        fn notify(&self, title: &str, body: &str) {
            self.0.lock().unwrap().push((title.into(), body.into()));
        }
    }

    #[test]
    fn fake_clock_advances() {
        let c = FakeClock::new(Local.with_ymd_and_hms(2026, 5, 31, 8, 0, 0).unwrap());
        c.advance(chrono::Duration::hours(2));
        assert_eq!(
            c.now(),
            Local.with_ymd_and_hms(2026, 5, 31, 10, 0, 0).unwrap()
        );
    }
}

#[cfg(test)]
mod heap_tests {
    use super::traits_tests::{CapturingNotifier, FakeClock};
    use super::*;
    use chrono::TimeZone;
    use std::sync::Arc;
    use uuid::Uuid;

    fn at_9am(year: i32, month: u32, day: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(year, month, day, 9, 0, 0).unwrap()
    }

    #[test]
    fn dated_todo_fires_at_9am_on_its_date() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 5, 31, 8, 59, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let mut sched = Scheduler::new(clock.clone(), notifier.clone());

        sched.upsert(ScheduledTodo {
            id: Uuid::nil(),
            title: "Story sc-1".into(),
            body: "ship it".into(),
            fire_at: at_9am(2026, 5, 31),
        });

        sched.tick();
        assert!(notifier.0.lock().unwrap().is_empty(), "not 9am yet");

        clock.advance(chrono::Duration::minutes(2));
        sched.tick();
        let fired = notifier.0.lock().unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].0, "Story sc-1");
        assert_eq!(fired[0].1, "ship it");
    }

    #[test]
    fn overdue_batched_on_first_tick() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 12, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let mut sched = Scheduler::new(clock.clone(), notifier.clone());

        sched.upsert(ScheduledTodo {
            id: Uuid::from_u128(1),
            title: "t1".into(), body: "a".into(), fire_at: at_9am(2026, 6, 1),
        });
        sched.upsert(ScheduledTodo {
            id: Uuid::from_u128(2),
            title: "t2".into(), body: "b".into(), fire_at: at_9am(2026, 6, 2),
        });
        sched.upsert(ScheduledTodo {
            id: Uuid::from_u128(3),
            title: "t3".into(), body: "c".into(), fire_at: at_9am(2026, 6, 10),
        });

        sched.tick();
        let fired = notifier.0.lock().unwrap();
        assert_eq!(fired.len(), 1);
        assert!(fired[0].1.contains("2 overdue todos"));
    }

    #[test]
    fn upsert_replaces_existing_fire_time() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 5, 31, 8, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let mut sched = Scheduler::new(clock.clone(), notifier.clone());

        let id = Uuid::new_v4();
        sched.upsert(ScheduledTodo {
            id, title: "t".into(), body: "b".into(), fire_at: at_9am(2026, 5, 31),
        });
        sched.upsert(ScheduledTodo {
            id, title: "t".into(), body: "b".into(), fire_at: at_9am(2026, 6, 30),
        });

        clock.advance(chrono::Duration::hours(2));
        sched.tick();
        assert!(notifier.0.lock().unwrap().is_empty(), "old fire time was cancelled");
    }

    #[test]
    fn cancel_removes_entry() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 5, 31, 8, 59, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let mut sched = Scheduler::new(clock.clone(), notifier.clone());

        let id = Uuid::new_v4();
        sched.upsert(ScheduledTodo {
            id, title: "t".into(), body: "b".into(), fire_at: at_9am(2026, 5, 31),
        });
        sched.cancel(id);
        clock.advance(chrono::Duration::hours(2));
        sched.tick();
        assert!(notifier.0.lock().unwrap().is_empty());
    }

    #[test]
    fn fired_overdue_does_not_refire_after_clear() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 12, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let mut sched = Scheduler::new(clock.clone(), notifier.clone());

        let id = Uuid::from_u128(1);
        sched.upsert(ScheduledTodo {
            id,
            title: "t".into(),
            body: "b".into(),
            fire_at: at_9am(2026, 6, 1),
        });
        sched.tick();
        assert_eq!(notifier.0.lock().unwrap().len(), 1, "batched overdue");

        sched.clear();
        sched.upsert(ScheduledTodo {
            id,
            title: "t".into(),
            body: "b".into(),
            fire_at: at_9am(2026, 6, 1),
        });
        sched.tick();
        assert_eq!(notifier.0.lock().unwrap().len(), 1, "did not re-fire");
    }
}
