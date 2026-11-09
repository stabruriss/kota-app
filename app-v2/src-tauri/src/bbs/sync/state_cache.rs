//! Owned by ContentStore and its clones; callers hold the content flock.
use super::*;

#[derive(Default)]
pub(super) struct StateCache {
    value: Option<(Option<SyncState>, Deps)>,
    #[cfg(test)]
    pub(super) writes: u64,
}
impl StateCache {
    pub(super) fn matches(&mut self, store: &StateStore, next: &SyncState) -> Result<bool> {
        if let Some((value, deps)) = &self.value {
            if deps.unchanged() {
                return Ok(value.as_ref() == Some(next));
            }
        }
        Ok(self.read(store)?.as_ref() == Some(next))
    }
    pub(super) fn read(&mut self, store: &StateStore) -> Result<Option<SyncState>> {
        if let Some((value, deps)) = &self.value {
            if deps.unchanged() {
                return Ok(value.clone());
            }
        }
        self.clear();
        let mut deps = Deps::new();
        let value = store
            .load_state_tracked(&mut deps)
            .map_err(anyhow::Error::msg)?;
        if deps.unchanged() {
            self.value = Some((value.clone(), deps));
        }
        Ok(value)
    }
    pub(super) fn clear(&mut self) {
        self.value = None;
    }
    pub(super) fn saved(&mut self) {
        self.clear();
        #[cfg(test)]
        {
            self.writes += 1;
        }
    }
}
