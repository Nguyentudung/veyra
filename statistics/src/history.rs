use std::collections::VecDeque;

#[derive(Debug)]
pub struct History {
    values: VecDeque<f64>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            values: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }
    pub fn push(&mut self, value: f64) {
        if self.values.len() == self.capacity {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }
    pub fn values(&self) -> Vec<f64> {
        self.values.iter().copied().collect()
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::History;
    #[test]
    fn history_is_bounded() {
        let mut h = History::new(2);
        h.push(1.0);
        h.push(2.0);
        h.push(3.0);
        assert_eq!(h.values(), vec![2.0, 3.0]);
    }
}
