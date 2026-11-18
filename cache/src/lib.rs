//! Small, dependency-free memory controls shared by Veyra subsystems.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// A process-local byte budget. Reservations are released automatically when
/// their guard is dropped, including on early returns and error paths.
#[derive(Debug, Clone)]
pub struct MemoryBudget {
    inner: Arc<BudgetInner>,
}

#[derive(Debug)]
struct BudgetInner {
    limit: usize,
    used: AtomicUsize,
}

impl MemoryBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            inner: Arc::new(BudgetInner {
                limit,
                used: AtomicUsize::new(0),
            }),
        }
    }

    pub fn limit(&self) -> usize {
        self.inner.limit
    }
    pub fn used(&self) -> usize {
        self.inner.used.load(Ordering::Relaxed)
    }
    pub fn available(&self) -> usize {
        self.limit().saturating_sub(self.used())
    }

    pub fn try_reserve(&self, bytes: usize) -> Option<Reservation> {
        let mut current = self.inner.used.load(Ordering::Relaxed);
        loop {
            let next = current.checked_add(bytes)?;
            if next > self.limit() {
                return None;
            }
            match self.inner.used.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(Reservation {
                        budget: self.clone(),
                        bytes,
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
}

pub struct Reservation {
    budget: MemoryBudget,
    bytes: usize,
}

impl Reservation {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget
            .inner
            .used
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::MemoryBudget;

    #[test]
    fn reservations_never_exceed_budget_and_release() {
        let budget = MemoryBudget::new(10);
        let reservation = budget.try_reserve(7).unwrap();
        assert_eq!(budget.used(), 7);
        assert!(budget.try_reserve(4).is_none());
        drop(reservation);
        assert_eq!(budget.available(), 10);
    }
}
