use postguy_model::NextRequest;

use crate::plan::PlannedRequest;

/// Most requests one iteration may run, so a `setNextRequest` loop ends.
pub const MAX_STEPS_PER_ITERATION: usize = 10_000;

/// Walks a plan, honouring `setNextRequest` directives.
#[derive(Debug, Clone)]
pub struct Sequencer {
    order: Vec<PlannedRequest>,
    position: Option<usize>,
    steps: usize,
    warnings: Vec<String>,
}

impl Sequencer {
    pub fn new(order: Vec<PlannedRequest>) -> Sequencer {
        Sequencer { order, position: None, steps: 0, warnings: Vec::new() }
    }

    pub fn order(&self) -> &[PlannedRequest] {
        &self.order
    }

    /// Begin an iteration; returns the first request's index.
    pub fn start(&mut self) -> Option<usize> {
        self.steps = 0;
        self.warnings.clear();
        self.position = if self.order.is_empty() { None } else { Some(0) };
        self.step()
    }

    /// The index of the request to run after the current one, given the script's directive.
    pub fn next(&mut self, directive: &NextRequest) -> Option<usize> {
        let current = self.position?;
        self.position = match directive {
            NextRequest::Unchanged => current.checked_add(1).filter(|next| *next < self.order.len()),
            NextRequest::Stop => None,
            NextRequest::Named(target) => {
                let found = self
                    .order
                    .iter()
                    .position(|planned| planned.name == *target)
                    .or_else(|| self.order.iter().position(|planned| planned.id.0 == *target));
                if found.is_none() {
                    self.warnings.push(format!("setNextRequest(\"{target}\") matched no request; the iteration ended"));
                }
                found
            }
        };
        self.step()
    }

    fn step(&mut self) -> Option<usize> {
        let position = self.position?;
        self.steps = self.steps.saturating_add(1);
        if self.steps > MAX_STEPS_PER_ITERATION {
            self.warnings.push(format!("setNextRequest looped more than {MAX_STEPS_PER_ITERATION} times; the iteration ended"));
            self.position = None;
            return None;
        }
        Some(position)
    }

    /// Warnings collected during this iteration.
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}
