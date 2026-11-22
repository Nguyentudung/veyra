use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Aggregator {
    values: BTreeMap<String, RunningAverage>,
}

#[derive(Clone, Debug, Default)]
struct RunningAverage {
    total: f64,
    count: u64,
}

impl Aggregator {
    pub fn add(&mut self, name: &str, value: f64) {
        let average = self.values.entry(name.into()).or_default();
        average.total += value;
        average.count = average.count.saturating_add(1);
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn averages(&self) -> BTreeMap<String, f64> {
        self.values
            .iter()
            .filter_map(|(name, average)| {
                (average.count > 0).then_some((name.clone(), average.total / average.count as f64))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Aggregator;
    #[test]
    fn average_is_mean_of_samples() {
        let mut a = Aggregator::default();
        a.add("frame_processing_ms", 2.0);
        a.add("frame_processing_ms", 4.0);
        assert_eq!(a.averages()["frame_processing_ms"], 3.0);
    }

    #[test]
    fn averages_do_not_retain_each_sample() {
        let mut a = Aggregator::default();
        for _ in 0..10_000 {
            a.add("position", 1.0);
        }
        assert_eq!(a.values.len(), 1);
        assert_eq!(a.averages()["position"], 1.0);
    }
}
