use anyhow::{Context, Result};
use signal_hook::{SigId, flag, low_level};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(crate) struct StopSignals {
    stop: Arc<AtomicBool>,
    registrations: Vec<SigId>,
}

impl StopSignals {
    pub(crate) fn install(numbers: &[i32]) -> Result<Self> {
        let mut signals = Self::default();
        for &number in numbers {
            signals.registrations.push(
                flag::register(number, Arc::clone(&signals.stop))
                    .with_context(|| format!("installing cleanup signal {number}"))?,
            );
        }
        Ok(signals)
    }

    pub(crate) fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
}

impl Drop for StopSignals {
    fn drop(&mut self) {
        for &registration in &self.registrations {
            low_level::unregister(registration);
        }
    }
}
