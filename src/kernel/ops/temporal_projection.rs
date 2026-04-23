use std::collections::HashMap;
use std::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeOfDay {
    Morning,    // 6-12
    Afternoon,  // 12-18
    Evening,    // 18-22
    Night,      // 22-6
}

impl TimeOfDay {
    pub fn from_hour(hour: u32) -> Self {
        match hour {
            6..=11 => TimeOfDay::Morning,
            12..=17 => TimeOfDay::Afternoon,
            18..=21 => TimeOfDay::Evening,
            _ => TimeOfDay::Night,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TemporalPattern {
    pub time_of_day: TimeOfDay,
    pub day_of_week: Option<u8>,  // 0=Mon, 6=Sun, None=any
    pub intents: Vec<String>,
    pub hit_count: usize,
}

pub struct TemporalProjectionEngine {
    patterns: RwLock<HashMap<TimeOfDay, TemporalPattern>>,
}

impl TemporalProjectionEngine {
    pub fn new() -> Self {
        Self {
            patterns: RwLock::new(HashMap::new()),
        }
    }

    pub fn record_intent(&self, intent: &str, timestamp_ms: u64) {
        let hour = (timestamp_ms / 3_600_000) % 24;
        let time_of_day = TimeOfDay::from_hour(hour as u32);

        let mut patterns = self.patterns.write().unwrap();
        let pattern = patterns.entry(time_of_day).or_insert_with(|| TemporalPattern {
            time_of_day,
            day_of_week: None,
            intents: Vec::new(),
            hit_count: 0,
        });

        if !pattern.intents.contains(&intent.to_string()) {
            pattern.intents.push(intent.to_string());
        }
        pattern.hit_count += 1;
    }

    pub fn project(&self, target_hour: u32) -> Vec<String> {
        let time_of_day = TimeOfDay::from_hour(target_hour);
        let patterns = self.patterns.read().unwrap();

        match patterns.get(&time_of_day) {
            Some(pattern) => {
                let mut intents = pattern.intents.clone();
                intents.sort_by(|a, b| {
                    let count_a = patterns.values()
                        .filter(|p| p.intents.contains(a))
                        .map(|p| p.hit_count)
                        .sum::<usize>();
                    let count_b = patterns.values()
                        .filter(|p| p.intents.contains(b))
                        .map(|p| p.hit_count)
                        .sum::<usize>();
                    count_b.cmp(&count_a)
                });
                intents
            }
            None => Vec::new(),
        }
    }
}

impl Default for TemporalProjectionEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temporal_projection_records_intent() {
        let engine = TemporalProjectionEngine::new();

        // 6 AM UTC = 6 hours since midnight = Morning
        let six_am_ms = 6 * 3_600_000;
        engine.record_intent("backup_data", six_am_ms);
        engine.record_intent("check_mail", six_am_ms);

        let projected = engine.project(6); // 6 AM - Morning
        assert_eq!(projected.len(), 2);
        assert!(projected.contains(&"backup_data".to_string()));
        assert!(projected.contains(&"check_mail".to_string()));
    }

    #[test]
    fn test_temporal_projection_morning_intent() {
        let engine = TemporalProjectionEngine::new();

        // 6 AM UTC
        let six_am_ms = 6 * 3_600_000;
        engine.record_intent("morning_report", six_am_ms);

        let projected = engine.project(6); // 6 AM - Morning
        assert_eq!(projected.first(), Some(&"morning_report".to_string()));
    }

    #[test]
    fn test_temporal_projection_project_evening() {
        let engine = TemporalProjectionEngine::new();

        // 18:00 UTC (Evening) = 18 * 3_600_000
        let six_pm_ms = 18 * 3_600_000;
        engine.record_intent("review_logs", six_pm_ms);
        engine.record_intent("backup_data", six_pm_ms);
        engine.record_intent("review_logs", six_pm_ms);

        let projected = engine.project(19); // 7 PM - Evening
        assert!(!projected.is_empty());
        assert!(projected.contains(&"review_logs".to_string()));
    }
}
