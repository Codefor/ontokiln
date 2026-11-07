//! Additional inclusive counters for opt-in execution profiles.
use std::cell::Cell;
pub type Meter = fn() -> (u64, u64);
thread_local! { static METER:Cell<Option<Meter>>=const {Cell::new(None)}; }

pub fn with_query_meter<T>(meter: Meter, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Meter>);
    impl Drop for Restore {
        fn drop(&mut self) {
            METER.set(self.0);
        }
    }
    let _restore = Restore(METER.replace(Some(meter)));
    f()
}
pub(crate) fn read() -> (u64, u64) {
    METER.get().map_or((0, 0), |f| f())
}
