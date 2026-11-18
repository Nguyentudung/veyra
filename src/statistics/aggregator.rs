use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Aggregator {
    values: BTreeMap<String, Vec<f64>>,
}

impl Aggregator {
    pub fn add(&mut self, name: &str, value: f64) {
        self.values.entry(name.into()).or_default().push(value);
    }
    pub fn averages(&self) -> BTreeMap<String, f64> {
        self.values
            .iter()
            .filter_map(|(name, values)| {
                (!values.is_empty()).then_some((
                    name.clone(),
                    values.iter().sum::<f64>() / values.len() as f64,
                ))
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
}
