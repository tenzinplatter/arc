use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Context;
use chrono::{DateTime, Local, NaiveDate};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

const RETRY_BASE: chrono::Duration = chrono::Duration::minutes(1);
const RETRY_CAP: chrono::Duration = chrono::Duration::minutes(30);

pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> DateTime<Local>;
}

pub trait Notifier: Send + Sync + 'static {
    fn notify(&self, title: &str, body: &str, receipt: Receipt);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Local> {
        Local::now()
    }
}

pub struct DBusNotifier;

impl Notifier for DBusNotifier {
    fn notify(&self, title: &str, body: &str, receipt: Receipt) {
        let _ = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .show();
        receipt.delivered();
    }
}

pub struct NoNotifier;

impl Notifier for NoNotifier {
    fn notify(&self, _title: &str, _body: &str, receipt: Receipt) {
        receipt.delivered();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeliveryId(u64);

#[derive(Debug)]
pub struct Outcome {
    delivery: DeliveryId,
    delivered: bool,
}

pub struct Receipt {
    delivery: DeliveryId,
    outcomes: Option<UnboundedSender<Outcome>>,
}

impl Receipt {
    pub fn delivered(mut self) {
        self.settle(true);
    }

    pub fn failed(mut self) {
        self.settle(false);
    }

    fn settle(&mut self, delivered: bool) {
        if let Some(outcomes) = self.outcomes.take() {
            let _ = outcomes.send(Outcome {
                delivery: self.delivery,
                delivered,
            });
        }
    }
}

impl Drop for Receipt {
    fn drop(&mut self) {
        self.settle(false);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReminderKey {
    pub id: Uuid,
    pub date: NaiveDate,
}

impl fmt::Display for ReminderKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.id, self.date)
    }
}

impl FromStr for ReminderKey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let (id, date) = s.split_once(' ').context("expected \"<id> <date>\"")?;
        Ok(Self {
            id: id.parse()?,
            date: date.parse()?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ScheduledTodo {
    pub id: Uuid,
    pub title: String,
    pub body: String,
    pub fire_at: DateTime<Local>,
}

impl ScheduledTodo {
    pub fn key(&self) -> ReminderKey {
        ReminderKey {
            id: self.id,
            date: self.fire_at.date_naive(),
        }
    }
}

#[derive(Debug)]
struct Message {
    title: String,
    body: String,
    keys: Vec<ReminderKey>,
}

impl Message {
    fn single(todo: ScheduledTodo) -> Self {
        Self {
            keys: vec![todo.key()],
            title: todo.title,
            body: todo.body,
        }
    }

    fn overdue_batch(todos: &[ScheduledTodo]) -> Self {
        Self {
            title: "arc — overdue todos".into(),
            body: format!("{} overdue todos. Check the arc TUI.", todos.len()),
            keys: todos.iter().map(ScheduledTodo::key).collect(),
        }
    }
}

#[derive(Debug)]
struct Pending {
    message: Message,
    failed_attempts: u32,
}

impl Pending {
    fn retry_delay(&self) -> chrono::Duration {
        let factor = 2i32.saturating_pow(self.failed_attempts);
        (RETRY_BASE * factor).min(RETRY_CAP)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    AwaitingOverdueBatch,
    Steady,
}

pub struct Scheduler {
    clock: Arc<dyn Clock>,
    notifier: Arc<dyn Notifier>,
    outcomes: UnboundedSender<Outcome>,
    entries: HashMap<Uuid, ScheduledTodo>,
    fired: HashSet<ReminderKey>,
    in_flight: HashMap<DeliveryId, Pending>,
    retries: Vec<(DateTime<Local>, Pending)>,
    next_delivery: u64,
    phase: Phase,
}

impl Scheduler {
    pub fn new(
        clock: Arc<dyn Clock>,
        notifier: Arc<dyn Notifier>,
        outcomes: UnboundedSender<Outcome>,
        previously_fired: HashSet<ReminderKey>,
    ) -> Self {
        Self {
            clock,
            notifier,
            outcomes,
            entries: HashMap::new(),
            fired: previously_fired,
            in_flight: HashMap::new(),
            retries: Vec::new(),
            next_delivery: 0,
            phase: Phase::AwaitingOverdueBatch,
        }
    }

    pub fn upsert(&mut self, todo: ScheduledTodo) {
        let key = todo.key();
        if self.fired.contains(&key) || self.is_pending(&key) {
            return;
        }
        self.entries.insert(todo.id, todo);
    }

    fn is_pending(&self, key: &ReminderKey) -> bool {
        self.in_flight
            .values()
            .chain(self.retries.iter().map(|(_, p)| p))
            .any(|p| p.message.keys.contains(key))
    }

    pub fn cancel(&mut self, id: Uuid) {
        self.entries.remove(&id);
        self.fired.retain(|k| k.id != id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn tick(&mut self) {
        let now = self.clock.now();
        match self.phase {
            Phase::AwaitingOverdueBatch => {
                let overdue = self.take_due(|t| t.fire_at < now);
                if !overdue.is_empty() {
                    self.send(Message::overdue_batch(&overdue), 0);
                }
                self.phase = Phase::Steady;
            }
            Phase::Steady => {
                for t in self.take_due(|t| t.fire_at <= now) {
                    self.send(Message::single(t), 0);
                }
            }
        }
        let (due, waiting) = std::mem::take(&mut self.retries)
            .into_iter()
            .partition::<Vec<_>, _>(|(at, _)| *at <= now);
        self.retries = waiting;
        for (_, pending) in due {
            self.send(pending.message, pending.failed_attempts);
        }
    }

    #[must_use = "delivered reminders must be persisted to the fired log"]
    pub fn settle(&mut self, outcome: Outcome) -> Vec<ReminderKey> {
        let Some(pending) = self.in_flight.remove(&outcome.delivery) else {
            return Vec::new();
        };
        if outcome.delivered {
            self.fired.extend(pending.message.keys.iter().copied());
            return pending.message.keys;
        }
        let retry_at = self.clock.now() + pending.retry_delay();
        self.retries.push((
            retry_at,
            Pending {
                failed_attempts: pending.failed_attempts + 1,
                message: pending.message,
            },
        ));
        Vec::new()
    }

    fn send(&mut self, message: Message, failed_attempts: u32) {
        let delivery = DeliveryId(self.next_delivery);
        self.next_delivery += 1;
        let receipt = Receipt {
            delivery,
            outcomes: Some(self.outcomes.clone()),
        };
        self.notifier.notify(&message.title, &message.body, receipt);
        self.in_flight.insert(
            delivery,
            Pending {
                message,
                failed_attempts,
            },
        );
    }

    fn take_due(&mut self, is_due: impl Fn(&ScheduledTodo) -> bool) -> Vec<ScheduledTodo> {
        self.entries
            .extract_if(|_, t| is_due(t))
            .map(|(_, t)| t)
            .collect()
    }

    pub fn next_fire(&self) -> Option<DateTime<Local>> {
        self.entries
            .values()
            .map(|t| t.fire_at)
            .chain(self.retries.iter().map(|(at, _)| *at))
            .min()
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    pub struct FakeClock(pub Mutex<DateTime<Local>>);

    impl FakeClock {
        pub fn new(at: DateTime<Local>) -> Self {
            Self(Mutex::new(at))
        }
        pub fn advance(&self, by: chrono::Duration) {
            let mut g = self.0.lock().unwrap();
            *g += by;
        }
        pub fn set(&self, at: DateTime<Local>) {
            *self.0.lock().unwrap() = at;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Local> {
            *self.0.lock().unwrap()
        }
    }

    #[derive(Default)]
    pub struct CapturingNotifier {
        pub sent: Mutex<Vec<(String, String)>>,
        failures: Mutex<VecDeque<()>>,
    }

    impl CapturingNotifier {
        pub fn failing_first(n: usize) -> Self {
            Self {
                failures: Mutex::new(std::iter::repeat_n((), n).collect()),
                ..Self::default()
            }
        }

        pub fn sent_count(&self) -> usize {
            self.sent.lock().unwrap().len()
        }
    }

    impl Notifier for CapturingNotifier {
        fn notify(&self, title: &str, body: &str, receipt: Receipt) {
            self.sent.lock().unwrap().push((title.into(), body.into()));
            match self.failures.lock().unwrap().pop_front() {
                Some(()) => receipt.failed(),
                None => receipt.delivered(),
            }
        }
    }

    pub fn scheduler(
        clock: Arc<FakeClock>,
        notifier: Arc<dyn Notifier>,
        previously_fired: HashSet<ReminderKey>,
    ) -> (Scheduler, UnboundedReceiver<Outcome>) {
        let (tx, rx) = unbounded_channel();
        (Scheduler::new(clock, notifier, tx, previously_fired), rx)
    }

    pub fn settle_all(
        sched: &mut Scheduler,
        outcomes: &mut UnboundedReceiver<Outcome>,
    ) -> Vec<ReminderKey> {
        std::iter::from_fn(|| outcomes.try_recv().ok())
            .flat_map(|o| sched.settle(o))
            .collect()
    }
}

#[cfg(test)]
mod heap_tests {
    use super::test_support::{CapturingNotifier, FakeClock, scheduler, settle_all};
    use super::*;
    use chrono::TimeZone;

    fn at_9am(year: i32, month: u32, day: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(year, month, day, 9, 0, 0).unwrap()
    }

    fn todo(n: u128, fire_at: DateTime<Local>) -> ScheduledTodo {
        ScheduledTodo {
            id: Uuid::from_u128(n),
            title: format!("t{n}"),
            body: format!("b{n}"),
            fire_at,
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

    #[test]
    fn dated_todo_fires_at_9am_on_its_date() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 5, 31, 8, 59, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let (mut sched, _outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());

        sched.upsert(ScheduledTodo {
            id: Uuid::nil(),
            title: "Story sc-1".into(),
            body: "ship it".into(),
            fire_at: at_9am(2026, 5, 31),
        });

        sched.tick();
        assert_eq!(notifier.sent_count(), 0, "not 9am yet");

        clock.advance(chrono::Duration::minutes(2));
        sched.tick();
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "Story sc-1");
        assert_eq!(sent[0].1, "ship it");
    }

    #[test]
    fn overdue_batched_on_first_tick() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 12, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let (mut sched, _outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());

        sched.upsert(todo(1, at_9am(2026, 6, 1)));
        sched.upsert(todo(2, at_9am(2026, 6, 2)));
        sched.upsert(todo(3, at_9am(2026, 6, 10)));

        sched.tick();
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.contains("2 overdue todos"));
    }

    #[test]
    fn upsert_replaces_existing_fire_time() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 5, 31, 8, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let (mut sched, _outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());

        sched.upsert(todo(1, at_9am(2026, 5, 31)));
        sched.upsert(todo(1, at_9am(2026, 6, 30)));

        clock.advance(chrono::Duration::hours(2));
        sched.tick();
        assert_eq!(notifier.sent_count(), 0, "old fire time was cancelled");
    }

    #[test]
    fn cancel_removes_entry() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 5, 31, 8, 59, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let (mut sched, _outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());

        sched.upsert(todo(1, at_9am(2026, 5, 31)));
        sched.cancel(Uuid::from_u128(1));
        clock.advance(chrono::Duration::hours(2));
        sched.tick();
        assert_eq!(notifier.sent_count(), 0);
    }

    #[test]
    fn fired_overdue_does_not_refire_after_clear() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 12, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let (mut sched, mut outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());

        sched.upsert(todo(1, at_9am(2026, 6, 1)));
        sched.tick();
        let recorded = settle_all(&mut sched, &mut outcomes);
        assert_eq!(recorded, vec![todo(1, at_9am(2026, 6, 1)).key()]);

        sched.clear();
        sched.upsert(todo(1, at_9am(2026, 6, 1)));
        sched.tick();
        assert_eq!(notifier.sent_count(), 1, "did not re-fire");
    }

    #[test]
    fn restart_does_not_refire_reminders_already_sent() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 8, 59, 0).unwrap(),
        ));
        let reminder = todo(7, at_9am(2026, 6, 5));
        let notifier = Arc::new(CapturingNotifier::default());
        let (mut before_restart, mut outcomes) =
            scheduler(clock.clone(), notifier.clone(), HashSet::new());
        before_restart.upsert(reminder.clone());
        before_restart.tick();
        clock.advance(chrono::Duration::minutes(2));
        before_restart.tick();
        let recorded = settle_all(&mut before_restart, &mut outcomes);
        assert_eq!(recorded, vec![reminder.key()]);

        clock.advance(chrono::Duration::hours(3));
        let (mut after_restart, _outcomes) =
            scheduler(clock.clone(), notifier.clone(), recorded.into_iter().collect());
        after_restart.upsert(reminder);
        after_restart.tick();

        assert_eq!(notifier.sent_count(), 1, "fired once across the restart");
    }

    #[test]
    fn redated_todo_fires_again_on_its_new_date() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 8, 59, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::default());
        let (mut sched, mut outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());
        sched.upsert(todo(1, at_9am(2026, 6, 5)));
        sched.tick();
        clock.advance(chrono::Duration::minutes(2));
        sched.tick();
        let _ = settle_all(&mut sched, &mut outcomes);

        sched.clear();
        sched.upsert(todo(1, at_9am(2026, 6, 5)));
        clock.advance(chrono::Duration::hours(1));
        sched.tick();
        assert_eq!(notifier.sent_count(), 1, "re-completing on the same date does not refire");

        sched.clear();
        sched.upsert(todo(1, at_9am(2026, 6, 6)));
        clock.advance(chrono::Duration::days(1));
        sched.tick();
        assert_eq!(notifier.sent_count(), 2, "fires again on the new date");
    }

    #[test]
    fn failed_delivery_retries_with_doubling_backoff_and_records_only_on_success() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 9, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::failing_first(3));
        let (mut sched, mut outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());
        sched.tick();
        let reminder = todo(1, at_9am(2026, 6, 5));
        sched.upsert(reminder.clone());

        for delay in [1, 2, 4] {
            sched.tick();
            assert!(settle_all(&mut sched, &mut outcomes).is_empty());
            let wait = chrono::Duration::minutes(delay);
            assert_eq!(sched.next_fire(), Some(clock.now() + wait));
            sched.clear();
            sched.upsert(reminder.clone());
            assert_eq!(sched.next_fire(), Some(clock.now() + wait), "rebuild does not duplicate");
            clock.advance(wait);
        }

        sched.tick();
        assert_eq!(settle_all(&mut sched, &mut outcomes), vec![reminder.key()]);
        assert_eq!(notifier.sent_count(), 4);
        assert_eq!(sched.next_fire(), None);
    }

    #[test]
    fn retry_backoff_caps_at_thirty_minutes() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 9, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::failing_first(10));
        let (mut sched, mut outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());
        sched.tick();
        sched.upsert(todo(1, at_9am(2026, 6, 5)));

        for _ in 0..8 {
            sched.tick();
            let _ = settle_all(&mut sched, &mut outcomes);
            let next = sched.next_fire().unwrap();
            assert!(next > clock.now(), "retries are never scheduled in the past");
            clock.set(next);
        }
        sched.tick();
        let _ = settle_all(&mut sched, &mut outcomes);
        assert_eq!(sched.next_fire(), Some(clock.now() + chrono::Duration::minutes(30)));
    }

    #[test]
    fn failed_overdue_batch_records_nothing_until_its_retry_succeeds() {
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 12, 0, 0).unwrap(),
        ));
        let notifier = Arc::new(CapturingNotifier::failing_first(1));
        let (mut sched, mut outcomes) = scheduler(clock.clone(), notifier.clone(), HashSet::new());
        sched.upsert(todo(1, at_9am(2026, 6, 1)));
        sched.upsert(todo(2, at_9am(2026, 6, 2)));

        sched.tick();
        assert!(settle_all(&mut sched, &mut outcomes).is_empty());

        clock.advance(chrono::Duration::minutes(1));
        sched.tick();
        let recorded: HashSet<_> = settle_all(&mut sched, &mut outcomes).into_iter().collect();
        assert_eq!(
            recorded,
            HashSet::from([todo(1, at_9am(2026, 6, 1)).key(), todo(2, at_9am(2026, 6, 2)).key()])
        );
        let sent = notifier.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|(_, body)| body.contains("2 overdue todos")));
    }

    #[test]
    fn dropped_receipt_counts_as_failure() {
        struct Forgetful;
        impl Notifier for Forgetful {
            fn notify(&self, _title: &str, _body: &str, _receipt: Receipt) {}
        }
        let clock = Arc::new(FakeClock::new(
            Local.with_ymd_and_hms(2026, 6, 5, 9, 0, 0).unwrap(),
        ));
        let (mut sched, mut outcomes) = scheduler(clock.clone(), Arc::new(Forgetful), HashSet::new());
        sched.tick();
        sched.upsert(todo(1, at_9am(2026, 6, 5)));
        sched.tick();

        assert!(settle_all(&mut sched, &mut outcomes).is_empty());
        assert_eq!(sched.next_fire(), Some(clock.now() + chrono::Duration::minutes(1)));
    }

    #[test]
    fn reminder_key_round_trips_through_its_log_line() {
        let key = todo(9, at_9am(2026, 6, 5)).key();
        assert_eq!(key.to_string().parse::<ReminderKey>().unwrap(), key);
        assert!(Uuid::from_u128(9).to_string().parse::<ReminderKey>().is_err());
    }
}
