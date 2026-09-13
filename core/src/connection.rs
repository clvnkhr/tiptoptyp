//! Process readiness and an endpoint are accepted together with their owner.
#[derive(Debug)]
enum Phase<G> {
    Offline,
    Starting(G),
    Ready(G),
}

/// The endpoint may survive a restart for display, but never implies readiness.
pub struct Connection<G, E> {
    phase: Phase<G>,
    endpoint: Option<(G, E)>,
}
impl<G, E> Default for Connection<G, E> {
    fn default() -> Self {
        Self {
            phase: Phase::Offline,
            endpoint: None,
        }
    }
}
impl<G: Copy + Eq, E> Connection<G, E> {
    pub fn stop(&mut self) {
        self.phase = Phase::Offline;
        self.endpoint = None;
    }
    pub fn suspend(&mut self, retain_display: bool) {
        self.phase = Phase::Offline;
        if !retain_display {
            self.endpoint = None;
        }
    }
    pub fn start(&mut self, generation: G) {
        self.phase = Phase::Starting(generation);
    }
    pub fn initialized(&mut self, generation: G) -> bool {
        if !matches!(self.phase, Phase::Starting(g) if g == generation) {
            return false;
        }
        self.phase = Phase::Ready(generation);
        true
    }
    pub fn connect(&mut self, generation: G, endpoint: E) -> bool {
        if !matches!(self.phase, Phase::Ready(g) if g == generation) {
            return false;
        }
        self.endpoint = Some((generation, endpoint));
        true
    }
    pub fn clear_endpoint(&mut self) {
        self.endpoint = None;
    }
    pub fn endpoint(&self) -> Option<&E> {
        self.endpoint.as_ref().map(|(_, e)| e)
    }
    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_endpoint_does_not_make_replacement_process_ready() {
        let mut connection = Connection::default();
        connection.start(1);
        assert!(!connection.connect(1, "url"));
        assert!(connection.initialized(1));
        assert!(connection.connect(1, "url"));
        connection.suspend(true);
        assert_eq!(connection.endpoint(), Some(&"url"));
        assert!(!connection.is_ready());
        connection.start(2);
        assert!(!connection.initialized(1));
        assert!(!connection.connect(1, "obsolete"));
        assert!(connection.initialized(2));
        assert!(connection.connect(2, "url"));
        connection.stop();
        assert!(!connection.connect(2, "late"));
        assert_eq!(connection.endpoint(), None);
    }
}
