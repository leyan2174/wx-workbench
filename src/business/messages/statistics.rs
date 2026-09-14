//! Metadata-only counts; database encodings and timezone conversion stay in adapters.
use super::{Error, Kind, Result};
use std::collections::HashMap;

#[derive(Default)]
pub struct Statistics {
    pub total: u64,
    pub by_kind: HashMap<Kind, u64>,
    pub by_sender: HashMap<String, u64>,
    pub by_hour: [u64; 24],
}
impl Statistics {
    pub fn add(&mut self, kind: Kind, hour: usize, sender: Option<&str>, count: u64) -> Result<()> {
        if hour >= 24 || count == 0 {
            return Err(Error::InvalidData);
        }
        self.total = self.total.checked_add(count).ok_or(Error::Limit)?;
        let total = self.by_kind.entry(kind).or_default();
        *total = total.checked_add(count).ok_or(Error::Limit)?;
        self.by_hour[hour] = self.by_hour[hour].checked_add(count).ok_or(Error::Limit)?;
        if let Some(sender) = sender {
            let total = self.by_sender.entry(sender.to_owned()).or_default();
            *total = total.checked_add(count).ok_or(Error::Limit)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_are_semantic_and_overflow_is_not_zero() {
        let mut result = Statistics::default();
        result.add(Kind::Voice, 3, Some("sender"), 2).unwrap();
        result.add(Kind::Call, 3, None, 1).unwrap();
        assert_eq!(result.total, 3);
        assert_eq!(result.by_hour[3], 3);
        assert_eq!(result.by_sender["sender"], 2);
        assert_eq!(result.add(Kind::Text, 24, None, 1), Err(Error::InvalidData));
        assert_eq!(result.add(Kind::Text, 0, None, u64::MAX), Err(Error::Limit));
    }
}
