//! Chip-independent Embassy network ownership across station associations.
//!
//! A network device is consumed exactly once to construct an IP stack. The
//! resulting stack and radio-side runner then survive Wi-Fi disconnect,
//! rescan and reassociation. This module owns that one-time/running transition;
//! DHCP versus static addressing and the concrete stack task remain caller
//! policy.

/// Network ownership before the first association or after a finite connected
/// epoch has returned its runner.
pub enum StationNetworkResources<D, R, S> {
    Unstarted { device: D, runner: R },
    Running(RunningStationNetwork<S, R>),
}

impl<D, R, S> StationNetworkResources<D, R, S> {
    pub const fn is_started(&self) -> bool {
        matches!(self, Self::Running(_))
    }

    /// Borrow the persistent physical radio-side network owner without
    /// changing the station-specific one-time start marker. A dual owner may
    /// retain distinct STA/AP RX endpoints while sharing one TX fabric.
    pub const fn radio_runner(&self) -> &R {
        match self {
            Self::Unstarted { runner, .. } => runner,
            Self::Running(network) => &network.runner,
        }
    }

    pub fn radio_runner_mut(&mut self) -> &mut R {
        match self {
            Self::Unstarted { runner, .. } => runner,
            Self::Running(network) => network.radio_runner_mut(),
        }
    }
}

/// Stack and radio-side network owner retained across association epochs.
pub struct RunningStationNetwork<S, R> {
    stack: S,
    runner: R,
}

impl<S, R> RunningStationNetwork<S, R> {
    pub const fn new(stack: S, runner: R) -> Self {
        Self { stack, runner }
    }

    pub fn into_parts(self) -> (S, R) {
        (self.stack, self.runner)
    }

    /// Borrow the radio-side endpoint while another role temporarily owns
    /// the shared Wi-Fi hardware. This does not restart or duplicate the
    /// network stack.
    pub const fn radio_runner(&self) -> &R {
        &self.runner
    }

    pub fn radio_runner_mut(&mut self) -> &mut R {
        &mut self.runner
    }
}

/// Uniform network frontier entering one connected station epoch.
///
/// `initial_task` is present only when this transition consumed the original
/// network device. A reconnect receives the existing stack and runner without
/// constructing or spawning another stack task.
pub struct StartedStationNetwork<S, R, T> {
    stack: S,
    runner: R,
    initial_task: Option<T>,
}

impl<S, R, T> StartedStationNetwork<S, R, T> {
    pub fn into_parts(self) -> (S, R, Option<T>) {
        (self.stack, self.runner, self.initial_task)
    }
}

/// Narrow capability needed by the ownership transition. It deliberately does
/// not expose RX publication or TX consumption to the network initializer.
#[doc(hidden)]
pub trait StationNetworkLink {
    fn publish_link_up(&self);
}

impl<N: StationNetworkLink + ?Sized> StationNetworkLink for &N {
    fn publish_link_up(&self) {
        N::publish_link_up(*self);
    }
}

impl<N: StationNetworkLink + ?Sized> StationNetworkLink for &mut N {
    fn publish_link_up(&self) {
        N::publish_link_up(*self);
    }
}

/// Enter a connected network epoch without rebuilding a running IP stack.
///
/// `start` owns only the one-time mapping from the network device to a stack
/// and its long-lived executor task. Address selection, random seed and stack
/// storage therefore remain explicit composition policy.
pub fn start_station_network<D, R, S, T>(
    resources: StationNetworkResources<D, R, S>,
    start: impl FnOnce(D) -> (S, T),
) -> StartedStationNetwork<S, R, T>
where
    R: StationNetworkLink,
{
    let (stack, runner, initial_task) = match resources {
        StationNetworkResources::Unstarted { device, runner } => {
            let (stack, task) = start(device);
            (stack, runner, Some(task))
        }
        StationNetworkResources::Running(network) => {
            let (stack, runner) = network.into_parts();
            (stack, runner, None)
        }
    };
    runner.publish_link_up();
    StartedStationNetwork {
        stack,
        runner,
        initial_task,
    }
}

#[cfg(test)]
mod tests;
